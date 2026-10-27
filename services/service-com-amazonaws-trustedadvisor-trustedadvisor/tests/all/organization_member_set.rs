// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE rule every organization-level view of a
//! member account's recommendation obeys, driven over HTTP through the same
//! `MultiServiceRouter` the emulator serves with:
//!
//! * **a member recommendation the organization recommendation aggregates is
//!   ONE member, however many times the store names it** — it is one
//!   `AccountRecommendationLifecycleSummary` of
//!   `ListOrganizationRecommendationAccounts`, one copy of each of its
//!   resources in `ListOrganizationRecommendationResources`, and one summand of
//!   `GetOrganizationRecommendation`'s `resourcesAggregates` and
//!   `pillarSpecificAggregates`;
//! * so `nextToken` still walks every member exactly once at any page size: the
//!   paged walk and the single-page read report the same sequence.
//!
//! The rows below differ along the number of member accounts the organization
//! has, which member is named more than once (the first, a middle one, the last
//! in ARN order, every one of them) and how many times it is named.
//!
//! Organization membership comes from AWS Organizations, outside this service,
//! and no modelled operation of this API enrols an account or produces its
//! check results, so a member — and a repeated naming of one — is written in
//! process, exactly as `tests/all/organization_recommendation_resources.rs`
//! writes a member account's resources.
//!
//! The per-member expectations are written out by hand; their source is the
//! `SEED` table in `src/state.rs`.

use serde_json::Value;
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{service_state, SupportPlan};
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_recommendation_lifecycle::{organization_recommendation_arn, read};
use crate::recommendation_loop::{
    call, emulator_on_plan, field_of_each, label, recommendation_arn, resource_arn,
};

/// The check the rows are built on: its recommendation optimizes the cost
/// pillar and carries three resources spanning two `ResourceStatus` values, so
/// one member contributes to every counter and to the cost aggregate.
const CHECK: &str = "low-utilization-ec2-instances";

/// The resource ids of `CHECK`'s seeded recommendation, in the order the `SEED`
/// table in `src/state.rs` lists them.
const RESOURCE_IDS: &[&str] = &["res-ec2-0001", "res-ec2-0002", "res-ec2-0003"];

/// What ONE member recommendation of `CHECK` contributes to
/// `resourcesAggregates`, as `(okCount, warningCount, errorCount,
/// excludedCount)`: its three seeded resources are one `ok` and two `warning`,
/// none excluded. Source: the `SEED` table in `src/state.rs`.
const PER_MEMBER_COUNTS: (u64, u64, u64, u64) = (1, 2, 0, 0);

/// What ONE member recommendation of `CHECK` contributes to the cost
/// aggregate: the sum of its resources' "Estimated Monthly Savings" column
/// (72.00 + 0.00 + 148.00) out of the sum of what they cost per month
/// (144.00 + 36.00 + 260.00). Source: the `SEED` table in `src/state.rs`.
const PER_MEMBER_SAVINGS: f64 = 220.00;
const PER_MEMBER_SPEND: f64 = 440.00;

/// One row of the table: the member accounts of the organization, and how many
/// times the store names each one's member recommendation.
struct Row {
    case: &'static str,
    /// `(account id, number of times the stored member list names that
    /// account's recommendation)`. The emulator's own account is the one the
    /// seed enrolled; every other account is enrolled by this suite.
    named: &'static [(&'static str, usize)],
}

/// The emulator's own account, which the seed enrolled as `CHECK`'s single
/// member. Its ARN sorts FIRST of the accounts below, so a row naming it twice
/// duplicates the head of the sequence.
const OWN: &str = engine::EMULATOR_ACCOUNT_ID;

const ROWS: &[Row] = &[
    Row {
        case: "the only member named twice",
        named: &[(OWN, 2)],
    },
    Row {
        case: "the only member named three times",
        named: &[(OWN, 3)],
    },
    Row {
        case: "the first member of three named twice",
        named: &[(OWN, 2), ("111122223333", 1), ("999988887777", 1)],
    },
    Row {
        case: "a middle member of three named twice",
        named: &[(OWN, 1), ("111122223333", 2), ("999988887777", 1)],
    },
    Row {
        case: "the last member of three named twice",
        named: &[(OWN, 1), ("111122223333", 1), ("999988887777", 2)],
    },
    Row {
        case: "every member named more than once",
        named: &[(OWN, 2), ("111122223333", 3), ("999988887777", 2)],
    },
    Row {
        case: "four members, none named twice",
        named: &[
            (OWN, 1),
            ("111122223333", 1),
            ("444455556666", 1),
            ("999988887777", 1),
        ],
    },
];

/// A mounted emulator on a plan that includes the Trusted Advisor API, with the
/// engine it was built on so a test can enrol members the way something outside
/// this API would.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    emulator_on_plan(SupportPlan::Business)
}

/// The modelled URI of `ListOrganizationRecommendationAccounts` for `CHECK`'s
/// organization recommendation.
fn accounts_path() -> String {
    format!(
        "/v1/organization-recommendations/{}/accounts",
        label(&organization_recommendation_arn(CHECK))
    )
}

/// The modelled URI of `ListOrganizationRecommendationResources` for `CHECK`'s
/// organization recommendation.
fn resources_path() -> String {
    format!(
        "/v1/organization-recommendations/{}/resources",
        label(&organization_recommendation_arn(CHECK))
    )
}

/// `arn:aws:trustedadvisor::<account>:recommendation/<check>-recommendation` —
/// the ARN pattern `AccountRecommendationArn` declares.
fn member_arn(account_id: &str) -> String {
    if account_id == OWN {
        return recommendation_arn(CHECK);
    }
    format!("arn:aws:trustedadvisor::{account_id}:recommendation/{CHECK}-recommendation")
}

/// `arn:aws:trustedadvisor::<account>:recommendation-resource/<rec>/<resource>`
/// — the ARN pattern `RecommendationResourceArn` declares.
fn member_resource_arn(account_id: &str, resource_id: &str) -> String {
    if account_id == OWN {
        return resource_arn(CHECK, resource_id);
    }
    format!(
        "arn:aws:trustedadvisor::{account_id}:recommendation-resource/\
         {CHECK}-recommendation/{resource_id}"
    )
}

/// Enrol the row's member accounts and name each one's recommendation the
/// number of times the row asks for, leaving the stored list in an order that
/// is NOT ARN order so the sequence under test is the handler's.
///
/// Every enrolled account gets a copy of `CHECK`'s seeded recommendation and of
/// each of its resources, so one member contributes the same counts and cost
/// totals whichever account owns it.
fn enrol(engine: &engine::InMemoryEngine, named: &[(&str, usize)]) {
    let seeded_arn = recommendation_arn(CHECK);
    let organization_arn = organization_recommendation_arn(CHECK);
    let state = service_state(engine).expect("the in-memory engine carries this service's state");
    let mut state = state.lock();

    // The stored list starts as the seed left it (the emulator's own account),
    // and this suite writes the whole membership of the row.
    let mut members: Vec<String> = Vec::new();
    for (account, times) in named {
        let arn = member_arn(account);
        if *account != OWN {
            let mut member = state
                .recommendations
                .get(&seeded_arn)
                .unwrap_or_else(|| panic!("no seeded recommendation for {CHECK}"))
                .clone();
            member.arn = arn.clone();
            let copies: Vec<_> = state
                .resources_of(&seeded_arn)
                .map(|(_, resource)| {
                    let mut copy = resource.clone();
                    copy.arn = member_resource_arn(account, &copy.id);
                    copy.recommendation_arn = arn.clone();
                    copy
                })
                .collect();
            state.recommendations.insert(arn.clone(), member);
            for copy in copies {
                state.recommendation_resources.insert(copy.arn.clone(), copy);
            }
        }
        for _ in 0..*times {
            members.push(arn.clone());
        }
    }
    // Reverse ARN order, so no handler can get the page order for free.
    members.reverse();
    state
        .organization_recommendations
        .get_mut(&organization_arn)
        .unwrap_or_else(|| panic!("no organization recommendation for {CHECK}"))
        .member_recommendation_arns = members;
}

/// The distinct member recommendations of a row, in the ARN order every
/// organization list pages in.
fn expected_members(named: &[(&str, usize)]) -> Vec<String> {
    let mut arns: Vec<String> = named.iter().map(|(account, _)| member_arn(account)).collect();
    arns.sort();
    arns.dedup();
    arns
}

/// The distinct resources of a row's members, in ARN order.
fn expected_resources(named: &[(&str, usize)]) -> Vec<String> {
    let mut arns: Vec<String> = named
        .iter()
        .flat_map(|(account, _)| {
            RESOURCE_IDS
                .iter()
                .map(move |id| member_resource_arn(account, id))
        })
        .collect();
    arns.sort();
    arns.dedup();
    arns
}

/// The body one read reports, asserting 200 first.
async fn ok_body(router: &MultiServiceRouter, case: &str, path: &str) -> Value {
    let (status, body) = call(router, "GET", path, None).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200, got {status} with {body}"
    );
    body
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
        let page = field_of_each(&body, list, field);
        assert!(
            page.len() <= page_size,
            "case {case}: expected at most {page_size} per page, got {}",
            page.len()
        );
        seen.extend(page);
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
async fn a_member_named_more_than_once_is_one_accounts_row_at_every_page_size() {
    for row in ROWS {
        let (router, engine) = emulator();
        enrol(&engine, row.named);
        let expected = expected_members(row.named);

        let body = ok_body(&router, row.case, &accounts_path()).await;
        let listed = field_of_each(
            &body,
            "accountRecommendationLifecycleSummaries",
            "accountRecommendationArn",
        );
        assert_eq!(
            listed, expected,
            "case {}: expected each member recommendation once in ARN order, got {listed:?}",
            row.case
        );
        let accounts = field_of_each(&body, "accountRecommendationLifecycleSummaries", "accountId");
        let mut distinct = accounts.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            accounts.len(),
            distinct.len(),
            "case {}: expected one row per account, got {accounts:?}",
            row.case
        );

        // The paged walk sees the same sequence as the single-page read — the
        // page boundary falls on a repeated member at one of these sizes.
        for page_size in [1usize, 2, 3, 4, 200] {
            let case = format!("{} at maxResults={page_size}", row.case);
            let seen = paged_walk(
                &router,
                &case,
                &accounts_path(),
                "accountRecommendationLifecycleSummaries",
                "accountRecommendationArn",
                page_size,
                expected.len(),
            )
            .await;
            assert_eq!(
                seen, expected,
                "case {case}: expected the paged walk to report every member exactly once, got \
                 {seen:?}"
            );
        }

        // And filtering to one account reports that account's one member, which
        // is where a repeated member would show up as two identical rows.
        for (account, _) in row.named {
            let case = format!("{} filtered to {account}", row.case);
            let body = ok_body(
                &router,
                &case,
                &format!("{}?affectedAccountId={account}", accounts_path()),
            )
            .await;
            let listed = field_of_each(
                &body,
                "accountRecommendationLifecycleSummaries",
                "accountRecommendationArn",
            );
            assert_eq!(
                listed,
                vec![member_arn(account)],
                "case {case}: expected that account's one member, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_member_named_more_than_once_contributes_its_resources_once() {
    for row in ROWS {
        let (router, engine) = emulator();
        enrol(&engine, row.named);
        let expected = expected_resources(row.named);

        let body = ok_body(&router, row.case, &resources_path()).await;
        let listed = field_of_each(&body, "organizationRecommendationResourceSummaries", "arn");
        assert_eq!(
            listed, expected,
            "case {}: expected each member resource once in ARN order, got {listed:?}",
            row.case
        );

        for page_size in [1usize, 2, 5, 200] {
            let case = format!("{} at maxResults={page_size}", row.case);
            let seen = paged_walk(
                &router,
                &case,
                &resources_path(),
                "organizationRecommendationResourceSummaries",
                "arn",
                page_size,
                expected.len(),
            )
            .await;
            assert_eq!(
                seen, expected,
                "case {case}: expected the paged walk to report every resource exactly once, got \
                 {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_member_named_more_than_once_is_summed_once_into_the_aggregates() {
    for row in ROWS {
        let (router, engine) = emulator();
        enrol(&engine, row.named);
        let members = expected_members(row.named).len() as u64;

        let organization = read(&router, row.case, CHECK).await;
        let aggregates = &organization["resourcesAggregates"];
        let (ok, warning, error, excluded) = PER_MEMBER_COUNTS;
        for (counter, expected) in [
            ("okCount", ok * members),
            ("warningCount", warning * members),
            ("errorCount", error * members),
            ("excludedCount", excluded * members),
        ] {
            assert_eq!(
                aggregates[counter].as_u64(),
                Some(expected),
                "case {}: expected resourcesAggregates.{counter} {expected} over {members} \
                 member recommendations, got {:?}",
                row.case,
                aggregates[counter]
            );
        }

        // The resource list and the counters are two views of one store, so
        // they agree on how many resources the organization has.
        let counted = ok * members + warning * members + error * members;
        let listed = field_of_each(
            &ok_body(&router, row.case, &resources_path()).await,
            "organizationRecommendationResourceSummaries",
            "arn",
        );
        assert_eq!(
            listed.len() as u64,
            counted,
            "case {}: expected resourcesAggregates to count the {} listed resources, got \
             {counted}",
            row.case,
            listed.len()
        );

        // The cost aggregate is summed over the same members: the savings grow
        // with the member count, and the percentage of spend does not move,
        // whatever the stored list repeats.
        let cost = &organization["pillarSpecificAggregates"]["costOptimizing"];
        let savings = cost["estimatedMonthlySavings"].as_f64();
        let expected_savings = PER_MEMBER_SAVINGS * members as f64;
        assert!(
            savings.is_some_and(|got| (got - expected_savings).abs() < 1e-9),
            "case {}: expected estimatedMonthlySavings {expected_savings} over {members} member \
             recommendations, got {savings:?}",
            row.case
        );
        let percent = cost["estimatedPercentMonthlySavings"].as_f64();
        let expected_percent = PER_MEMBER_SAVINGS / PER_MEMBER_SPEND * 100.0;
        assert!(
            percent.is_some_and(|got| (got - expected_percent).abs() < 1e-9),
            "case {}: expected estimatedPercentMonthlySavings {expected_percent}, got {percent:?}",
            row.case
        );
    }
}

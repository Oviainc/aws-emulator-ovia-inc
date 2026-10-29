// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ACCOUNT SCOPE of `ListRecommendations`, driven over
//! HTTP through the same `MultiServiceRouter` the emulator serves with.
//!
//! The rule: an account-level listing reports only the stored rows whose own ARN
//! names the calling account; a row owned by another account is not part of that
//! list, under any filter and at any page size, and the view that reports other
//! accounts' rows is the organization listing.
//!
//! `RecommendationSummary` is documented "Summary of Recommendation for an
//! Account" and its `arn` is an `AccountRecommendationArn`
//! (`^arn:[\w-]+:trustedadvisor::\d{12}:recommendation\/[\w-]+$`), whose account
//! segment names the owner, while `OrganizationRecommendationSummary` is
//! "Summary of recommendation for accounts within an Organization"
//! (`smithy_query(query="shape",shape="RecommendationSummary,OrganizationRecommendationSummary",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`).
//!
//! The store spans the organization's member accounts — that is what the
//! organization surface aggregates — so the rows below enrol member accounts in
//! process, "the way something outside this API would", and then assert that the
//! account-level list answers exactly as it did before they were stored, while
//! the organization-level view still reports them.
//!
//! Every call is made as an account whose support plan includes the Trusted
//! Advisor API; the gate ahead of every handler is pinned in
//! `tests/all/support_level_gate.rs`.

use serde_json::Value;
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{
    arn_account_id, service_state, SupportPlan,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_recommendation_lifecycle::organization_recommendation_arn;
use crate::owning_account_from_arn::{
    enrol_member_account, member_recommendation_arn, store_member_recommendation,
};
use crate::recommendation_loop::{
    call, check_arn, emulator_on_plan, field_of_each, label, recommendation_arn, CHECK_IDS,
    PRIORITIZED_CHECK_IDS,
};

/// The other accounts whose recommendations the store is made to hold: rows
/// differing along the account value itself — two unrelated 12-digit ids and one
/// with leading zeros. Every one of them sorts AFTER the emulator's canonical
/// account id in ARN order, so a leaked row would also be the TAIL of the
/// listing, which is where a page walk surfaces it. Shared with
/// `tests/all/owning_account_from_arn.rs`, which pins the organization views over
/// the same accounts.
const OTHER_ACCOUNTS: &[&str] = &["111122223333", "999988887777", "000000000012"];

/// Each row: a `ListRecommendations` query, and the check ids whose
/// recommendations the CALLING account's listing must report for it, in key
/// order. Rows differ along the member filtered on, the number of matches (all,
/// several, one, none) and a combination of two filters. Written out by hand;
/// their source is the `SEED` table in `src/state.rs`.
const CASES: &[(&str, &[&str])] = &[
    ("", CHECK_IDS),
    ("?type=priority", PRIORITIZED_CHECK_IDS),
    (
        "?type=standard",
        &["amazon-ebs-snapshots", "iam-access-key-rotation"],
    ),
    ("?status=error", &["amazon-ebs-snapshots"]),
    (
        "?status=warning",
        &["low-utilization-ec2-instances", "rds-idle-db-instances"],
    ),
    (
        "?pillar=cost_optimizing",
        &["low-utilization-ec2-instances", "rds-idle-db-instances"],
    ),
    ("?awsService=ec2", &["low-utilization-ec2-instances"]),
    ("?source=rds", &["rds-idle-db-instances"]),
    (
        "?afterLastUpdatedAt=2024-01-04T00:00:00Z",
        &["iam-access-key-rotation", "rds-idle-db-instances"],
    ),
    ("?type=standard&source=rds", &[]),
    // `language` selects which stored copy of the text is reported, so it must
    // change no row — and no row's owner.
    ("?language=ja", CHECK_IDS),
];

/// Read `path`, asserting 200 first, and answer the body.
async fn read(router: &MultiServiceRouter, case: &str, path: &str) -> Value {
    let (status, body) = call(router, "GET", path, None).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200, got {status} with {body}"
    );
    body
}

/// The recommendation ARNs one `ListRecommendations` query reports, with the
/// page size big enough for the whole result.
async fn listed(router: &MultiServiceRouter, query: &str) -> Vec<String> {
    let separator = if query.is_empty() { "?" } else { "&" };
    let body = read(
        router,
        query,
        &format!("/v1/recommendations{query}{separator}maxResults=200"),
    )
    .await;
    field_of_each(&body, "recommendationSummaries", "arn")
}

/// The CALLING account's recommendation ARNs for a list of check ids.
fn own_arns(check_ids: &[&str]) -> Vec<String> {
    check_ids.iter().map(|id| recommendation_arn(id)).collect()
}

#[tokio::test]
async fn the_account_listing_reports_only_the_calling_accounts_recommendations() {
    let (router, engine) = emulator_on_plan(SupportPlan::Business);

    // What each query answers while the store holds only the calling account's
    // rows, compared against the catalog written out above.
    let mut before: Vec<Vec<String>> = Vec::new();
    for (query, expected) in CASES {
        let arns = listed(&router, query).await;
        let expected = own_arns(expected);
        assert_eq!(
            arns, expected,
            "case {query:?} before enrolment: expected {expected:?}, got {arns:?}"
        );
        before.push(arns);
    }
    let unfiltered_before = read(&router, "unfiltered", "/v1/recommendations?maxResults=200").await;

    // Now the store holds every other account's copy of every seeded check's
    // recommendation and resources. A prioritized check's copy is also enrolled
    // into that check's organization recommendation — the aggregate the
    // organization surface exists for; a standard check has no organization
    // recommendation, so its copy is simply stored.
    for account in OTHER_ACCOUNTS {
        for check_id in CHECK_IDS {
            if PRIORITIZED_CHECK_IDS.contains(check_id) {
                enrol_member_account(&engine, check_id, account);
            } else {
                store_member_recommendation(&engine, check_id, account);
            }
            let stored = member_recommendation_arn(account, check_id);
            assert!(
                service_state(&*engine)
                    .expect("the in-memory engine carries this service's state")
                    .lock()
                    .recommendations
                    .contains_key(&stored),
                "case {account}/{check_id}: expected the store to hold {stored}"
            );
        }
    }

    // Every query answers exactly as it did: the other accounts' rows are not
    // part of this account's listing, and no filter reaches them.
    for ((query, expected), was) in CASES.iter().zip(&before) {
        let arns = listed(&router, query).await;
        let expected_own = own_arns(expected);
        assert_eq!(
            arns, expected_own,
            "case {query:?} after enrolment: expected {expected_own:?}, got {arns:?}"
        );
        assert_eq!(
            &arns, was,
            "case {query:?}: expected the listing to be unchanged by other accounts' rows, got \
             {arns:?} where it answered {was:?}"
        );
        for arn in &arns {
            let owner = arn_account_id(arn);
            assert_eq!(
                owner.as_deref(),
                Some(engine::EMULATOR_ACCOUNT_ID),
                "case {query:?}/{arn}: expected a row owned by the calling account, got {owner:?}"
            );
        }
    }

    // Not only the ARNs: the whole unfiltered response is what it was, so no
    // other account's resources moved an aggregate either.
    let unfiltered_after = read(&router, "unfiltered", "/v1/recommendations?maxResults=200").await;
    assert_eq!(
        unfiltered_after, unfiltered_before,
        "case unfiltered: expected the response to be unchanged by other accounts' rows, got \
         {unfiltered_after} where it answered {unfiltered_before}"
    );

    // And one filter aimed straight at a stored foreign row — the check it was
    // copied from — still reports only the calling account's recommendation for
    // that check.
    for check_id in CHECK_IDS {
        let query = format!("?checkIdentifier={}", label(&check_arn(check_id)));
        let arns = listed(&router, &query).await;
        assert_eq!(
            arns,
            own_arns(&[check_id]),
            "case checkIdentifier={check_id}: expected only this account's recommendation, got \
             {arns:?}"
        );
    }
}

#[tokio::test]
async fn paging_the_account_listing_walks_the_calling_accounts_rows_only() {
    let (router, engine) = emulator_on_plan(SupportPlan::Business);
    for account in OTHER_ACCOUNTS {
        for check_id in CHECK_IDS {
            store_member_recommendation(&engine, check_id, account);
        }
    }
    let expected = own_arns(CHECK_IDS);

    // Rows differ along the page size: one row per page, two, exactly the number
    // of the calling account's rows, and a page larger than it. The last size
    // matters most: at exactly that many results the token must CLEAR, which a
    // listing that also walked the other accounts' rows could not do.
    for page_size in [1usize, 2, expected.len(), 200] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One round beyond the number of pages the size implies, so a token that
        // never clears is caught as a loop rather than hanging.
        for _ in 0..=expected.len() + 1 {
            let query = match &token {
                Some(value) => format!("?maxResults={page_size}&nextToken={}", label(value)),
                None => format!("?maxResults={page_size}"),
            };
            let body = read(
                &router,
                &format!("maxResults={page_size}"),
                &format!("/v1/recommendations{query}"),
            )
            .await;
            let page = field_of_each(&body, "recommendationSummaries", "arn");
            assert!(
                page.len() <= page_size,
                "case maxResults={page_size}: expected at most {page_size} per page, got {}",
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
            "case maxResults={page_size}: expected the token to clear, got {token:?}"
        );
        assert_eq!(
            seen, expected,
            "case maxResults={page_size}: expected the calling account's rows once each in key \
             order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn the_organization_listing_still_reports_the_other_accounts_rows() {
    let (router, engine) = emulator_on_plan(SupportPlan::Business);
    // Scoping the account-level list does not drop the rows from the store: the
    // cross-account view of the same rows is the organization surface, which
    // reports one row per member account of the organization.
    let check_id = PRIORITIZED_CHECK_IDS[0];
    for account in OTHER_ACCOUNTS {
        enrol_member_account(&engine, check_id, account);
    }

    let body = read(
        &router,
        "organization accounts",
        &format!(
            "/v1/organization-recommendations/{}/accounts?maxResults=200",
            label(&organization_recommendation_arn(check_id))
        ),
    )
    .await;
    let mut members = field_of_each(
        &body,
        "accountRecommendationLifecycleSummaries",
        "accountRecommendationArn",
    );
    members.sort();

    let mut expected: Vec<String> = OTHER_ACCOUNTS
        .iter()
        .map(|account| member_recommendation_arn(account, check_id))
        .chain(std::iter::once(recommendation_arn(check_id)))
        .collect();
    expected.sort();
    assert_eq!(
        members, expected,
        "case organization accounts: expected every member account's recommendation \
         {expected:?}, got {members:?}"
    );

    // While the account-level listing of the same store reports only the calling
    // account's row for that check.
    let arns = listed(
        &router,
        &format!("?checkIdentifier={}", label(&check_arn(check_id))),
    )
    .await;
    assert_eq!(
        arns,
        own_arns(&[check_id]),
        "case account listing: expected only this account's recommendation, got {arns:?}"
    );
}

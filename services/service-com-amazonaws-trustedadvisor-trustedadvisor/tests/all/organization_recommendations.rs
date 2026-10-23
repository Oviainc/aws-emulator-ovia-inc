// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListOrganizationRecommendations`, driven over HTTP
//! through the same `MultiServiceRouter` the emulator serves with.
//!
//! The rules this suite pins, each over a TABLE of inputs rather than one call:
//!
//! * **the list reports exactly the stored organization recommendations its
//!   filters keep, in organization-recommendation-ARN order** — an absent
//!   filter keeps every one of them, and each filter is an equality or a
//!   membership test except `afterLastUpdatedAt`/`beforeLastUpdatedAt`, which
//!   are inclusive bounds on `lastUpdatedAt`;
//! * every ARN the list reports resolves in `GetOrganizationRecommendation` and
//!   both reads report the same members: the two are one store read twice;
//! * **`resourcesAggregates` is AGGREGATED on every read over the member
//!   accounts' recommendations** — never stored — so it equals what those
//!   accounts' `ListRecommendations` reports and it follows an exclusion made
//!   through `BatchUpdateRecommendationResourceExclusion` or a change in a
//!   member resource's status;
//! * `nextToken` walks every entry exactly once at any page size, and a token
//!   this read did not issue is refused with 400 `ValidationException`.
//!
//! The expected catalog below is written out by hand; its source is the `SEED`
//! table in `src/state.rs`. Member names, filters and statuses come from the
//! model — `smithy_query(query="operation",operation="ListOrganizationRecommendations",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
//! — GET `/v1/organization-recommendations` answers 200 with a required
//! `organizationRecommendationSummaries`, and binds `ValidationException` 400.
//!
//! Every call here is made as an account whose support plan includes the Trusted
//! Advisor API; the gate ahead of every handler is pinned in
//! `tests/all/support_level_gate.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{service_state, SupportPlan};
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_recommendation_lifecycle::{organization_recommendation_arn, read};
use crate::recommendation_loop::{
    call, check_arn, emulator_on_plan, error_code, field_of_each, label, recommendation_arn,
    resource_arn, timestamp_secs,
};

/// Every seeded check, in the order the store keys its organization
/// recommendation (check id order, since every such ARN shares one prefix).
/// Source: the `SEED` table in `src/state.rs`.
const CHECK_IDS: &[&str] = &[
    "amazon-ebs-snapshots",
    "iam-access-key-rotation",
    "low-utilization-ec2-instances",
    "rds-idle-db-instances",
];

/// The checks whose recommendation is `priority`, and those whose is
/// `standard` — the two halves the `type` filter cuts the store into.
const PRIORITY_CHECKS: &[&str] = &["low-utilization-ec2-instances", "rds-idle-db-instances"];
const STANDARD_CHECKS: &[&str] = &["amazon-ebs-snapshots", "iam-access-key-rotation"];

/// The ARN prefix `OrganizationRecommendationArn` declares
/// (`^arn:[\w-]+:trustedadvisor:::organization-recommendation\/[\w-]+$`): an
/// empty region AND account, because an organization recommendation spans the
/// organization's accounts rather than belonging to one.
const ORGANIZATION_ARN_PREFIX: &str = "arn:aws:trustedadvisor:::organization-recommendation/";

/// A mounted emulator on an entitled plan, with the engine it was built on so a
/// test can move the store the way the part of Trusted Advisor that produces
/// check results would (no modelled operation writes a resource's status).
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    emulator_on_plan(SupportPlan::Business)
}

/// The ARNs the list reports for `query`, asserting 200 first.
async fn list_arns(router: &MultiServiceRouter, query: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/v1/organization-recommendations{query}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {query:?}: expected 200, got {status} with {body}"
    );
    field_of_each(&body, "organizationRecommendationSummaries", "arn")
}

/// `resourcesAggregates` as `[okCount, warningCount, errorCount, excludedCount]`.
fn aggregates_of(case: &str, summary: &Value) -> [i64; 4] {
    let aggregates = &summary["resourcesAggregates"];
    ["okCount", "warningCount", "errorCount", "excludedCount"].map(|member| {
        aggregates[member]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: expected a numeric {member}, got {aggregates}"))
    })
}

/// The summary this list reports for one check's organization recommendation.
async fn summary_of(router: &MultiServiceRouter, case: &str, check_id: &str) -> Value {
    let (status, body) = call(router, "GET", "/v1/organization-recommendations", None).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200, got {status} with {body}"
    );
    let arn = organization_recommendation_arn(check_id);
    body["organizationRecommendationSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: no organizationRecommendationSummaries in {body}"))
        .iter()
        .find(|summary| summary["arn"].as_str() == Some(arn.as_str()))
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the list reports no {arn}: {body}"))
}

/// The account-level summary `ListRecommendations` reports for one check — the
/// member the organization recommendation aggregates.
async fn member_summary(router: &MultiServiceRouter, case: &str, check_id: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!(
            "/v1/recommendations?checkIdentifier={}",
            label(&check_arn(check_id))
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from ListRecommendations, got {status} with {body}"
    );
    let summary = body["recommendationSummaries"]
        .as_array()
        .and_then(|list| list.first())
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: no member recommendation in {body}"));
    assert_eq!(
        summary["arn"].as_str(),
        Some(recommendation_arn(check_id).as_str()),
        "case {case}: expected the member recommendation of {check_id}, got {:?}",
        summary["arn"]
    );
    summary
}

#[tokio::test]
async fn the_organization_list_reports_the_store_and_honours_every_filter() {
    let (router, _engine) = emulator();

    // Each row: the query string, and the checks whose organization
    // recommendation it must select, in ARN order. Rows differ along the member
    // filtered on, the number of matches (all, several, one, none), the
    // combination of filters, and the inclusive timestamp bounds. The seeded
    // `lastUpdatedAt` values come from the `SEED` table in `src/state.rs` (one
    // day apart in SEED order, which is not check-id order):
    //   low-utilization-ec2-instances  2024-01-02T00:00:00Z
    //   amazon-ebs-snapshots           2024-01-03T00:00:00Z
    //   iam-access-key-rotation        2024-01-04T00:00:00Z
    //   rds-idle-db-instances          2024-01-05T00:00:00Z
    let cases: &[(&str, &[&str])] = &[
        ("", CHECK_IDS),
        ("?type=priority", PRIORITY_CHECKS),
        ("?type=standard", STANDARD_CHECKS),
        ("?status=error", &["amazon-ebs-snapshots"]),
        ("?status=ok", &["iam-access-key-rotation"]),
        ("?status=warning", PRIORITY_CHECKS),
        ("?pillar=cost_optimizing", PRIORITY_CHECKS),
        ("?pillar=fault_tolerance", &["amazon-ebs-snapshots"]),
        // `performance` is the second pillar of one row: the filter is
        // membership, not equality.
        ("?pillar=performance", &["rds-idle-db-instances"]),
        ("?pillar=service_limits", &[]),
        ("?awsService=ebs", &["amazon-ebs-snapshots"]),
        ("?awsService=ec2", &["low-utilization-ec2-instances"]),
        ("?awsService=lambda", &[]),
        (
            "?source=ta_check",
            &[
                "amazon-ebs-snapshots",
                "iam-access-key-rotation",
                "low-utilization-ec2-instances",
            ],
        ),
        ("?source=rds", &["rds-idle-db-instances"]),
        ("?source=well_architected", &[]),
        (
            "?type=priority&status=warning&pillar=performance",
            &["rds-idle-db-instances"],
        ),
        // Two filters that cannot hold together select nothing.
        ("?type=standard&source=rds", &[]),
        (
            "?afterLastUpdatedAt=2024-01-04T00:00:00Z",
            &["iam-access-key-rotation", "rds-idle-db-instances"],
        ),
        (
            "?beforeLastUpdatedAt=2024-01-03T12:00:00Z",
            &["amazon-ebs-snapshots", "low-utilization-ec2-instances"],
        ),
        (
            "?afterLastUpdatedAt=2024-01-03T12:00:00Z&beforeLastUpdatedAt=2024-01-04T12:00:00Z",
            &["iam-access-key-rotation"],
        ),
        // Both bounds are inclusive, so the seeded instants themselves match.
        (
            "?afterLastUpdatedAt=2024-01-02T00:00:00Z&beforeLastUpdatedAt=2024-01-05T00:00:00Z",
            CHECK_IDS,
        ),
        ("?afterLastUpdatedAt=2030-01-01T00:00:00Z", &[]),
    ];

    for (query, expected) in cases {
        let arns = list_arns(&router, query).await;
        let expected_arns: Vec<String> = expected
            .iter()
            .map(|check| organization_recommendation_arn(check))
            .collect();
        assert_eq!(
            arns, expected_arns,
            "case {query:?}: expected {expected_arns:?}, got {arns:?}"
        );
    }

    // `checkIdentifier` selects by the check ARN each organization
    // recommendation carries — one row per check, including a check the issue
    // did not cite.
    for check_id in CHECK_IDS {
        let query = format!("?checkIdentifier={}", label(&check_arn(check_id)));
        let arns = list_arns(&router, &query).await;
        assert_eq!(
            arns,
            vec![organization_recommendation_arn(check_id)],
            "case checkIdentifier={check_id}: expected only that check's organization \
             recommendation, got {arns:?}"
        );
    }

    // `checkIdentifier` is modelled `length 20..64`: a check ARN at the upper
    // bound is accepted (it names nothing, so the page is empty), and one byte
    // past it is refused by the modelled constraint.
    // `arn:aws:trustedadvisor:::check/` is 31 bytes.
    let at_bound = format!("arn:aws:trustedadvisor:::check/{}", "c".repeat(33));
    assert_eq!(at_bound.len(), 64, "the bound case must be exactly 64 bytes");
    let arns = list_arns(&router, &format!("?checkIdentifier={}", label(&at_bound))).await;
    assert_eq!(
        arns,
        Vec::<String>::new(),
        "case checkIdentifier at the 64-byte bound: expected no match, got {arns:?}"
    );

    let past_bound = format!("arn:aws:trustedadvisor:::check/{}", "c".repeat(34));
    let (status, body) = call(
        &router,
        "GET",
        &format!(
            "/v1/organization-recommendations?checkIdentifier={}",
            label(&past_bound)
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "case checkIdentifier one byte past the bound: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case checkIdentifier one byte past the bound: expected ValidationException, got {body}"
    );
}

#[tokio::test]
async fn every_listed_summary_is_the_store_and_resolves_in_get() {
    let (router, _engine) = emulator();
    let (status, body) = call(&router, "GET", "/v1/organization-recommendations", None).await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");
    let summaries = body["organizationRecommendationSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no organizationRecommendationSummaries in {body}"))
        .clone();
    assert_eq!(
        summaries.len(),
        CHECK_IDS.len(),
        "expected one organization recommendation per seeded check, got {body}"
    );

    for (check_id, summary) in CHECK_IDS.iter().zip(&summaries) {
        let case = *check_id;
        let arn = organization_recommendation_arn(check_id);
        assert_eq!(
            summary["arn"].as_str(),
            Some(arn.as_str()),
            "case {case}: expected arn {arn}, got {:?}",
            summary["arn"]
        );

        // The ARN is the modelled form: the organization prefix, an id of
        // `[\w-]+`, and inside `length 20..200`.
        let id = arn.strip_prefix(ORGANIZATION_ARN_PREFIX).unwrap_or_else(|| {
            panic!("case {case}: expected the organization-recommendation ARN prefix, got {arn}")
        });
        assert!(
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-'),
            "case {case}: expected an id of [\\w-]+, got {id:?}"
        );
        assert!(
            (20..=200).contains(&arn.len()),
            "case {case}: expected the ARN inside length 20..200, got {}",
            arn.len()
        );
        assert_eq!(
            summary["id"].as_str(),
            Some(id),
            "case {case}: expected id {id:?}, got {:?}",
            summary["id"]
        );

        // The required members are computed from the store, not empty.
        for member in ["name", "type", "status", "source", "checkArn"] {
            assert!(
                summary[member].as_str().is_some_and(|text| !text.is_empty()),
                "case {case}: expected a non-empty {member}, got {:?}",
                summary[member]
            );
        }
        assert_eq!(
            summary["checkArn"].as_str(),
            Some(check_arn(check_id).as_str()),
            "case {case}: expected the checkArn of {check_id}, got {:?}",
            summary["checkArn"]
        );
        assert!(
            summary["pillars"]
                .as_array()
                .is_some_and(|list| !list.is_empty()),
            "case {case}: expected at least one pillar, got {:?}",
            summary["pillars"]
        );
        assert!(
            summary["awsServices"].as_array().is_some(),
            "case {case}: expected awsServices, got {:?}",
            summary["awsServices"]
        );
        // `createdAt` is never after `lastUpdatedAt`.
        let created = timestamp_secs(&summary["createdAt"]);
        let updated = timestamp_secs(&summary["lastUpdatedAt"]);
        assert!(
            created <= updated,
            "case {case}: expected createdAt {created} at or before lastUpdatedAt {updated}"
        );

        // The listing and the single read are one store read twice: every
        // listed ARN resolves, and the members both shapes declare agree.
        let got = read(&router, case, check_id).await;
        for member in [
            "id",
            "arn",
            "type",
            "status",
            "name",
            "source",
            "checkArn",
            "lifecycleStage",
            "pillars",
            "awsServices",
            "resourcesAggregates",
            "pillarSpecificAggregates",
            "createdAt",
            "lastUpdatedAt",
        ] {
            assert_eq!(
                got[member], summary[member],
                "case {case}: expected {member} {:?} from GetOrganizationRecommendation, got {:?}",
                summary[member], got[member]
            );
        }
    }
}

#[tokio::test]
async fn organization_aggregates_add_up_the_member_accounts_counts() {
    let (router, engine) = emulator();

    // The rule: an organization recommendation holds no resources of its own,
    // so its counts are the member accounts' counts added up — which, for the
    // one account this emulator presents, is that account's recommendation.
    //
    // Rows differ along WHAT changes a member's resources and how: an exclusion
    // made through the public API, several in one call, and a status change
    // made by the part of Trusted Advisor that produces check results. Each
    // expectation is [okCount, warningCount, errorCount, excludedCount].
    //
    // The seeded resources of `amazon-ebs-snapshots` (src/state.rs):
    //   res-ebs-0001 error, included;  res-ebs-0002 ok, excluded
    let check_id = "amazon-ebs-snapshots";
    let first = resource_arn(check_id, "res-ebs-0001");
    let second = resource_arn(check_id, "res-ebs-0002");

    enum Change {
        /// `BatchUpdateRecommendationResourceExclusion` entries, over the wire.
        Exclusions(Vec<(String, bool)>),
        /// A member resource's `ResourceStatus`, written in process.
        Status(&'static str, &'static str),
        None,
    }
    let rows: Vec<(&str, Change, [i64; 4])> = vec![
        ("the seeded store", Change::None, [1, 0, 1, 1]),
        (
            "an exclusion made through the API",
            Change::Exclusions(vec![(first.clone(), true)]),
            [1, 0, 1, 2],
        ),
        (
            "both resources included in one call",
            Change::Exclusions(vec![(first.clone(), false), (second.clone(), false)]),
            [1, 0, 1, 0],
        ),
        (
            "a member resource moves from ok to warning",
            Change::Status("res-ebs-0002", "warning"),
            [0, 1, 1, 0],
        ),
        (
            "and that resource is excluded too",
            Change::Exclusions(vec![(second.clone(), true)]),
            [0, 1, 1, 1],
        ),
    ];

    for (case, change, expected) in rows {
        match change {
            Change::Exclusions(entries) => {
                let payload = json!({
                    "recommendationResourceExclusions": entries
                        .iter()
                        .map(|(arn, is_excluded)| json!({"arn": arn, "isExcluded": is_excluded}))
                        .collect::<Vec<Value>>(),
                });
                let (status, body) = call(
                    &router,
                    "PUT",
                    "/v1/batch-update-recommendation-resource-exclusion",
                    Some(payload),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case}: expected 200 from the exclusion batch, got {status} with {body}"
                );
            }
            Change::Status(resource_id, status) => {
                let state = service_state(&*engine).expect("this service's state");
                let mut state = state.lock();
                let resource = state
                    .recommendation_resources
                    .values_mut()
                    .find(|resource| resource.id == resource_id)
                    .unwrap_or_else(|| panic!("case {case}: no seeded resource {resource_id}"));
                resource.status = status.to_string();
            }
            Change::None => {}
        }

        let summary = summary_of(&router, case, check_id).await;
        let actual = aggregates_of(case, &summary);
        assert_eq!(
            actual, expected,
            "case {case}: expected [ok, warning, error, excluded] {expected:?}, got {actual:?}"
        );

        // The same counts the member account reports for its own
        // recommendation: the organization view is those counts added up.
        let member = member_summary(&router, case, check_id).await;
        let member_aggregates = aggregates_of(case, &member);
        assert_eq!(
            actual, member_aggregates,
            "case {case}: expected the organization counts to equal the member's \
             {member_aggregates:?}, got {actual:?}"
        );
    }

    // And the rule holds for every other seeded organization recommendation,
    // whose members nothing above touched.
    for check_id in CHECK_IDS {
        let summary = summary_of(&router, check_id, check_id).await;
        let member = member_summary(&router, check_id, check_id).await;
        assert_eq!(
            aggregates_of(check_id, &summary),
            aggregates_of(check_id, &member),
            "case {check_id}: expected the organization counts to equal the member's"
        );
    }
}

#[tokio::test]
async fn paginating_the_organization_list_walks_every_entry_exactly_once() {
    let (router, _engine) = emulator();
    let expected: Vec<String> = CHECK_IDS
        .iter()
        .map(|check| organization_recommendation_arn(check))
        .collect();

    // Rows differ along the page size: one entry per page, two, exactly the
    // stored set, and a page larger than it.
    for page_size in [1usize, 2, 4, 200] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One extra round beyond the number of pages the size implies, so a
        // token that never clears is caught as a loop rather than hanging.
        for _ in 0..=expected.len() + 1 {
            let query = match &token {
                Some(value) => format!("?maxResults={page_size}&nextToken={}", label(value)),
                None => format!("?maxResults={page_size}"),
            };
            let (status, body) = call(
                &router,
                "GET",
                &format!("/v1/organization-recommendations{query}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size}: expected 200, got {status} with {body}"
            );
            let page = field_of_each(&body, "organizationRecommendationSummaries", "arn");
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
            "case maxResults={page_size}: expected every entry once in ARN order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_read_did_not_issue_is_refused() {
    let (router, _engine) = emulator();

    // A token this list DID issue, to replay elsewhere, and another list's
    // token, to replay here.
    let (_, first_page) = call(
        &router,
        "GET",
        "/v1/organization-recommendations?maxResults=1",
        None,
    )
    .await;
    let own_token = first_page["nextToken"]
        .as_str()
        .expect("a nextToken on a truncated page")
        .to_string();
    let (_, checks_page) = call(&router, "GET", "/v1/checks?maxResults=1", None).await;
    let checks_token = checks_page["nextToken"]
        .as_str()
        .expect("a nextToken on a truncated ListChecks page")
        .to_string();

    // Rows: a token this service never issued (the modelled `length 4..10000`
    // is satisfied, so the refusal is the handler's own); a token another list
    // issued; this list's own token under a FILTER it was not issued under,
    // which points into a different sequence; and its token on another list.
    let refused: &[(&str, String)] = &[
        (
            "a token this service never issued",
            "/v1/organization-recommendations?nextToken=%21%21%21%21".to_string(),
        ),
        (
            "a ListChecks token",
            format!(
                "/v1/organization-recommendations?nextToken={}",
                label(&checks_token)
            ),
        ),
        (
            "its own token under a different filter",
            format!(
                "/v1/organization-recommendations?type=priority&nextToken={}",
                label(&own_token)
            ),
        ),
        (
            "its own token on another list",
            format!("/v1/checks?nextToken={}", label(&own_token)),
        ),
    ];

    for (case, path) in refused {
        let (status, body) = call(&router, "GET", path, None).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
    }

    // And the token IS accepted by the read that issued it, resuming strictly
    // after the entry it names.
    let arns = list_arns(
        &router,
        &format!("?maxResults=1&nextToken={}", label(&own_token)),
    )
    .await;
    assert_eq!(
        arns,
        vec![organization_recommendation_arn(CHECK_IDS[1])],
        "case the issuing read accepts its own token: expected the second entry, got {arns:?}"
    );
}

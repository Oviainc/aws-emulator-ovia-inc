// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListOrganizationRecommendationResources`, driven over
//! HTTP through the same `MultiServiceRouter` the emulator serves with.
//!
//! The rules this suite pins, each over a TABLE of inputs rather than one call:
//!
//! * **the list reports exactly ONE `OrganizationRecommendationResourceSummary`
//!   per resource of the member accounts' recommendations the organization
//!   recommendation aggregates, in resource-ARN order** — an organization
//!   recommendation owns no resources of its own, so every value but two is the
//!   member resource's own stored one (and equals what the member account's
//!   `ListRecommendationResources` reports for it); `accountId` is the account
//!   that owns the resource, and `recommendationArn` is the ORGANIZATION
//!   recommendation's ARN, which is what that member is modelled to carry;
//! * `status`, `exclusionStatus`, `regionCode` and `affectedAccountId` keep
//!   exactly the resources that match, in any combination, and keep none when
//!   nothing does;
//! * `nextToken` walks every entry exactly once at any page size, and a token
//!   this read did not issue is refused with 400 `ValidationException`;
//! * an exclusion a member account makes through
//!   `BatchUpdateRecommendationResourceExclusion` is what this list reports for
//!   that resource on the next read, and it moves no other resource;
//! * a well-formed identifier that names no organization recommendation is 404
//!   `ResourceNotFoundException`, and an input outside a modelled constraint is
//!   400 `ValidationException`.
//!
//! The wire contract comes from the model —
//! `smithy_query(query="operation",operation="ListOrganizationRecommendationResources",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
//! — GET `/v1/organization-recommendations/{organizationRecommendationIdentifier}/resources`
//! answers 200 with a required `organizationRecommendationResourceSummaries`,
//! `maxResults` is `range 1..200`, `nextToken` is `length 4..10000`,
//! `affectedAccountId` is `length 12..12, pattern ^\d+$`, `status` is
//! `ResourceStatus` (`error | ok | warning`), `exclusionStatus` is
//! `ExclusionStatus` (`excluded | included`), and the operation binds
//! `ResourceNotFoundException` 404 and `ValidationException` 400.
//!
//! The seeded expectations below are written out by hand; their source is the
//! `SEED` table in `src/state.rs`. Every call is made as an account whose
//! support plan includes the Trusted Advisor API; the gate ahead of every
//! handler is pinned in `tests/all/support_level_gate.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{service_state, SupportPlan};
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_recommendation_lifecycle::{get_path, organization_recommendation_arn};
use crate::recommendation_loop::{
    call, emulator_on_plan, error_code, field_of_each, label, recommendation_arn, resource_arn,
    timestamp_secs,
};

/// One seeded resource of a seeded check's recommendation. Source: the `SEED`
/// table in `src/state.rs`.
struct SeededResource {
    id: &'static str,
    aws_resource_id: &'static str,
    region_code: &'static str,
    status: &'static str,
    exclusion_status: &'static str,
}

/// One seeded check, when its recommendation and resources were last updated,
/// and the resources themselves. Source: the `SEED` table in `src/state.rs`
/// (one day apart in SEED order, which is not check-id order).
struct Seeded {
    check_id: &'static str,
    last_updated_at: &'static str,
    resources: &'static [SeededResource],
}

const SEEDED: &[Seeded] = &[
    Seeded {
        check_id: "amazon-ebs-snapshots",
        last_updated_at: "2024-01-03T00:00:00Z",
        resources: &[
            SeededResource {
                id: "res-ebs-0001",
                aws_resource_id: "vol-0ea1b2c3d4e5f6789",
                region_code: "us-east-1",
                status: "error",
                exclusion_status: "included",
            },
            SeededResource {
                id: "res-ebs-0002",
                aws_resource_id: "vol-0ea2b2c3d4e5f6789",
                region_code: "ap-southeast-1",
                status: "ok",
                exclusion_status: "excluded",
            },
        ],
    },
    Seeded {
        check_id: "iam-access-key-rotation",
        last_updated_at: "2024-01-04T00:00:00Z",
        resources: &[SeededResource {
            id: "res-iam-0001",
            aws_resource_id: "AKIAIOSFODNN7EXAMPLE",
            region_code: "us-east-1",
            status: "ok",
            exclusion_status: "included",
        }],
    },
    Seeded {
        check_id: "low-utilization-ec2-instances",
        last_updated_at: "2024-01-02T00:00:00Z",
        resources: &[
            SeededResource {
                id: "res-ec2-0001",
                aws_resource_id: "i-0abc1de23f4567890",
                region_code: "us-east-1",
                status: "warning",
                exclusion_status: "included",
            },
            SeededResource {
                id: "res-ec2-0002",
                aws_resource_id: "i-0abc2de23f4567891",
                region_code: "us-west-2",
                status: "ok",
                exclusion_status: "included",
            },
            SeededResource {
                id: "res-ec2-0003",
                aws_resource_id: "i-0abc3de23f4567892",
                region_code: "eu-west-1",
                status: "warning",
                exclusion_status: "included",
            },
        ],
    },
    Seeded {
        check_id: "rds-idle-db-instances",
        last_updated_at: "2024-01-05T00:00:00Z",
        resources: &[
            SeededResource {
                id: "res-rds-0001",
                aws_resource_id: "reporting-prod",
                region_code: "us-east-1",
                status: "warning",
                exclusion_status: "included",
            },
            SeededResource {
                id: "res-rds-0002",
                aws_resource_id: "analytics-staging",
                region_code: "us-west-2",
                status: "ok",
                exclusion_status: "included",
            },
        ],
    },
];

/// The check whose organization recommendation the multi-account rows are built
/// on: three seeded resources spanning two `ResourceStatus` values and three
/// Regions, so each filter has matching and non-matching rows.
const CHECK: &str = "low-utilization-ec2-instances";

/// A mounted emulator on a plan that includes the Trusted Advisor API, with the
/// engine it was built on so a test can enrol organization member accounts the
/// way something outside this API would.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    emulator_on_plan(SupportPlan::Business)
}

/// The modelled URI of `ListOrganizationRecommendationResources` for one seeded
/// check's organization recommendation.
fn resources_path(check_id: &str) -> String {
    format!(
        "/v1/organization-recommendations/{}/resources",
        label(&organization_recommendation_arn(check_id))
    )
}

/// `arn:aws:trustedadvisor::<account>:recommendation/<check>-recommendation` —
/// the ARN pattern `AccountRecommendationArn` declares, for a member account
/// other than the emulator's own.
fn member_recommendation_arn(account_id: &str, check_id: &str) -> String {
    format!("arn:aws:trustedadvisor::{account_id}:recommendation/{check_id}-recommendation")
}

/// `arn:aws:trustedadvisor::<account>:recommendation-resource/<rec>/<resource>`
/// — the ARN pattern `RecommendationResourceArn` declares, for a member account
/// other than the emulator's own.
fn member_resource_arn(account_id: &str, check_id: &str, resource_id: &str) -> String {
    format!(
        "arn:aws:trustedadvisor::{account_id}:recommendation-resource/\
         {check_id}-recommendation/{resource_id}"
    )
}

/// Enrol one more member account of the organization: a copy of `check_id`'s
/// seeded recommendation owned by `account_id`, with a copy of each of its
/// resources, aggregated by that check's organization recommendation.
/// `rewrite` is applied to every copied resource, so a row can give an account
/// its own statuses, Regions and exclusion states.
///
/// Written in process because no modelled operation of this API enrols an
/// account or produces its check results — organization membership comes from
/// AWS Organizations, outside this service — exactly as
/// `tests/all/organization_recommendations.rs` writes a member resource's
/// status. The member list is deliberately grown in an order that is NOT ARN
/// order, so the page order under test is the handler's.
fn add_member_account(
    engine: &engine::InMemoryEngine,
    check_id: &str,
    account_id: &str,
    rewrite: impl Fn(&mut service_com_amazonaws_trustedadvisor_trustedadvisor::state::ResourceEntry),
) {
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
            rewrite(&mut copy);
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
async fn summaries(router: &MultiServiceRouter, case: &str, path: &str) -> Value {
    let (status, body) = call(router, "GET", path, None).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200, got {status} with {body}"
    );
    body
}

/// The `arn` of every summary one read reports.
async fn listed_arns(router: &MultiServiceRouter, case: &str, path: &str) -> Vec<String> {
    let body = summaries(router, case, path).await;
    field_of_each(
        &body,
        "organizationRecommendationResourceSummaries",
        "arn",
    )
}

/// The summaries one read reports, as an array.
async fn listed(router: &MultiServiceRouter, case: &str, path: &str) -> Vec<Value> {
    let body = summaries(router, case, path).await;
    body["organizationRecommendationResourceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: no summaries in {body}"))
        .clone()
}

#[tokio::test]
async fn the_resources_list_reports_the_member_accounts_resources_of_every_seeded_check() {
    let (router, _engine) = emulator();
    let account = engine::EMULATOR_ACCOUNT_ID;

    // Rows: every seeded organization recommendation, which differ along the
    // number of resources their member aggregates (one, two, three) and along
    // those resources' statuses, Regions and exclusion states.
    for row in SEEDED {
        let case = row.check_id;
        let organization_arn = organization_recommendation_arn(case);
        let body = summaries(&router, case, &resources_path(case)).await;
        let reported = body["organizationRecommendationResourceSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no summaries in {body}"));
        assert_eq!(
            reported.len(),
            row.resources.len(),
            "case {case}: expected one summary per member resource ({}), got {body}",
            row.resources.len()
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case}: expected no nextToken on a complete page, got {:?}",
            body["nextToken"]
        );

        // The resources of the one member account, in resource-ARN order —
        // which, within one account, is resource-id order.
        for (expected, summary) in row.resources.iter().zip(reported) {
            let case = format!("{case}/{}", expected.id);
            let expected_arn = resource_arn(row.check_id, expected.id);
            for (member, want) in [
                ("id", expected.id),
                ("arn", expected_arn.as_str()),
                ("awsResourceId", expected.aws_resource_id),
                ("regionCode", expected.region_code),
                ("status", expected.status),
                ("exclusionStatus", expected.exclusion_status),
                ("accountId", account),
                ("recommendationArn", organization_arn.as_str()),
            ] {
                assert_eq!(
                    summary[member].as_str(),
                    Some(want),
                    "case {case}: expected {member} {want}, got {:?}",
                    summary[member]
                );
            }
            // `accountId` is modelled `length 12..12, pattern ^\d+$`, and it is
            // the account segment of the resource's own ARN.
            let reported_account = summary["accountId"].as_str().unwrap_or_default();
            assert!(
                reported_account.len() == 12
                    && reported_account.chars().all(|c| c.is_ascii_digit()),
                "case {case}: expected a 12-digit accountId, got {reported_account:?}"
            );
            assert_eq!(
                expected_arn.split(':').nth(4),
                Some(reported_account),
                "case {case}: expected the accountId to be the ARN's account segment, got \
                 {reported_account:?} for {expected_arn}"
            );
            assert_eq!(
                timestamp_secs(&summary["lastUpdatedAt"]),
                timestamp_secs(&json!(row.last_updated_at)),
                "case {case}: expected lastUpdatedAt {}, got {:?}",
                row.last_updated_at,
                summary["lastUpdatedAt"]
            );
        }

        // And every value the two views share is the one stored copy: the
        // member account's own `ListRecommendationResources` reports the same
        // resources with the same values, differing only in the
        // `recommendationArn` each is reported under.
        let member_path = format!(
            "/v1/recommendations/{}/resources",
            label(&recommendation_arn(case))
        );
        let member_body = summaries(&router, case, &member_path).await;
        let member_reported = member_body["recommendationResourceSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no member summaries in {member_body}"));
        for (organization, member) in reported.iter().zip(member_reported) {
            for shared in [
                "id",
                "arn",
                "awsResourceId",
                "regionCode",
                "status",
                "exclusionStatus",
                "metadata",
                "lastUpdatedAt",
            ] {
                assert_eq!(
                    organization[shared], member[shared],
                    "case {case}: expected the organization view's {shared} to be the member's \
                     {:?}, got {:?}",
                    member[shared], organization[shared]
                );
            }
            assert_eq!(
                member["recommendationArn"].as_str(),
                Some(recommendation_arn(case).as_str()),
                "case {case}: expected the member view to report the account recommendation, got \
                 {:?}",
                member["recommendationArn"]
            );
        }
    }
}

/// One resource of `CHECK` as the store holds it after the member accounts
/// below are enrolled: the account that owns it and the values the filters cut
/// on.
struct Stored {
    account: &'static str,
    id: &'static str,
    status: &'static str,
    region_code: &'static str,
    exclusion_status: &'static str,
}

/// The emulator's own account's seeded resources of [`CHECK`], plus the two
/// member accounts [`enrol_member_accounts`] adds.
fn stored_rows() -> Vec<Stored> {
    let seeded = SEEDED
        .iter()
        .find(|row| row.check_id == CHECK)
        .expect("the seeded rows name CHECK")
        .resources;
    let mut rows: Vec<Stored> = Vec::new();
    for resource in seeded {
        // The emulator's own account, as the seed left it.
        rows.push(Stored {
            account: engine::EMULATOR_ACCOUNT_ID,
            id: resource.id,
            status: resource.status,
            region_code: resource.region_code,
            exclusion_status: resource.exclusion_status,
        });
        // `111122223333` holds a copy of each resource moved to `error` in one
        // Region and excluded.
        rows.push(Stored {
            account: "111122223333",
            id: resource.id,
            status: "error",
            region_code: "ap-south-1",
            exclusion_status: "excluded",
        });
        // `444455556666` holds them exactly as seeded.
        rows.push(Stored {
            account: "444455556666",
            id: resource.id,
            status: resource.status,
            region_code: resource.region_code,
            exclusion_status: resource.exclusion_status,
        });
    }
    rows
}

/// Enrol the two member accounts [`stored_rows`] describes, in an order that is
/// not ARN order.
fn enrol_member_accounts(engine: &engine::InMemoryEngine) {
    add_member_account(engine, CHECK, "444455556666", |_| {});
    add_member_account(engine, CHECK, "111122223333", |resource| {
        resource.status = "error".to_string();
        resource.region_code = "ap-south-1".to_string();
        resource.exclusion_status = "excluded".to_string();
    });
}

/// The ARN of a stored row's resource.
fn arn_of(row: &Stored) -> String {
    member_resource_arn(row.account, CHECK, row.id)
}

#[tokio::test]
async fn every_filter_selects_the_member_accounts_resources_it_keeps() {
    let (router, engine) = emulator();
    enrol_member_accounts(&engine);
    let rows = stored_rows();
    let own = engine::EMULATOR_ACCOUNT_ID;

    // Rows differ along the member filtered on, the number of matches (all,
    // several, one, none) and the combination of two filters. Each expectation
    // is derived from the stored set above by the SAME rule the request names,
    // then sorted into the resource-ARN order the list reports.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(String, Box<dyn Fn(&Stored) -> bool>)> = vec![
        ("".to_string(), Box::new(|_: &Stored| true)),
        (
            "?status=error".to_string(),
            Box::new(|row: &Stored| row.status == "error"),
        ),
        (
            "?status=ok".to_string(),
            Box::new(|row: &Stored| row.status == "ok"),
        ),
        (
            "?status=warning".to_string(),
            Box::new(|row: &Stored| row.status == "warning"),
        ),
        (
            "?exclusionStatus=excluded".to_string(),
            Box::new(|row: &Stored| row.exclusion_status == "excluded"),
        ),
        (
            "?exclusionStatus=included".to_string(),
            Box::new(|row: &Stored| row.exclusion_status == "included"),
        ),
        (
            "?regionCode=ap-south-1".to_string(),
            Box::new(|row: &Stored| row.region_code == "ap-south-1"),
        ),
        (
            "?regionCode=us-east-1".to_string(),
            Box::new(|row: &Stored| row.region_code == "us-east-1"),
        ),
        (
            "?regionCode=sa-east-1".to_string(),
            Box::new(|row: &Stored| row.region_code == "sa-east-1"),
        ),
        (
            "?affectedAccountId=111122223333".to_string(),
            Box::new(|row: &Stored| row.account == "111122223333"),
        ),
        (
            format!("?affectedAccountId={own}"),
            Box::new(|row: &Stored| row.account == engine::EMULATOR_ACCOUNT_ID),
        ),
        (
            "?affectedAccountId=222233334444".to_string(),
            Box::new(|row: &Stored| row.account == "222233334444"),
        ),
        (
            "?affectedAccountId=111122223333&status=ok".to_string(),
            Box::new(|row: &Stored| row.account == "111122223333" && row.status == "ok"),
        ),
        (
            "?affectedAccountId=444455556666&status=warning".to_string(),
            Box::new(|row: &Stored| row.account == "444455556666" && row.status == "warning"),
        ),
        (
            format!("?affectedAccountId={own}&regionCode=us-west-2"),
            Box::new(|row: &Stored| {
                row.account == engine::EMULATOR_ACCOUNT_ID && row.region_code == "us-west-2"
            }),
        ),
        (
            "?status=error&exclusionStatus=included".to_string(),
            Box::new(|row: &Stored| row.status == "error" && row.exclusion_status == "included"),
        ),
    ];

    for (query, keep) in cases {
        let mut expected: Vec<String> = rows.iter().filter(|row| keep(row)).map(arn_of).collect();
        expected.sort();
        let case = format!("query {query:?}");
        let path = format!("{}{query}", resources_path(CHECK));
        let reported = listed(&router, &case, &path).await;
        let arns: Vec<String> = reported
            .iter()
            .map(|summary| {
                summary["arn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: a summary with no arn: {summary}"))
                    .to_string()
            })
            .collect();
        assert_eq!(
            arns, expected,
            "case {case}: expected {expected:?} in ARN order, got {arns:?}"
        );
        // Each summary names the account that owns the resource, which is its
        // own ARN's account segment.
        for summary in &reported {
            let arn = summary["arn"].as_str().unwrap_or_default();
            assert_eq!(
                summary["accountId"].as_str(),
                arn.split(':').nth(4),
                "case {case}: expected accountId {:?} for {arn}, got {:?}",
                arn.split(':').nth(4),
                summary["accountId"]
            );
        }
    }
}

#[tokio::test]
async fn paginating_the_resources_list_walks_every_entry_exactly_once() {
    let (router, engine) = emulator();
    enrol_member_accounts(&engine);
    let mut expected: Vec<String> = stored_rows().iter().map(arn_of).collect();
    expected.sort();

    // Rows differ along the page size: one entry per page, two, three, exactly
    // the stored set, and a page larger than it (200 is the modelled maximum).
    for page_size in [1usize, 2, 3, 9, 200] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One round beyond the number of pages the size implies, so a token
        // that never clears is caught as a loop rather than hanging.
        for _ in 0..=expected.len() + 1 {
            let query = match &token {
                Some(value) => format!("?maxResults={page_size}&nextToken={}", label(value)),
                None => format!("?maxResults={page_size}"),
            };
            let case = format!("maxResults={page_size}");
            let body = summaries(&router, &case, &format!("{}{query}", resources_path(CHECK))).await;
            let page = field_of_each(
                &body,
                "organizationRecommendationResourceSummaries",
                "arn",
            );
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
            "case maxResults={page_size}: expected the token to clear, got {token:?}"
        );
        assert_eq!(
            seen, expected,
            "case maxResults={page_size}: expected every resource once in ARN order, got {seen:?}"
        );
    }

    // And a filtered sequence pages the same way: one account's three
    // resources, one per page.
    let mut filtered: Vec<String> = stored_rows()
        .iter()
        .filter(|row| row.account == "111122223333")
        .map(arn_of)
        .collect();
    filtered.sort();
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..=filtered.len() + 1 {
        let query = match &token {
            Some(value) => format!(
                "?affectedAccountId=111122223333&maxResults=1&nextToken={}",
                label(value)
            ),
            None => "?affectedAccountId=111122223333&maxResults=1".to_string(),
        };
        let body = summaries(
            &router,
            "filtered to one account at maxResults=1",
            &format!("{}{query}", resources_path(CHECK)),
        )
        .await;
        seen.extend(field_of_each(
            &body,
            "organizationRecommendationResourceSummaries",
            "arn",
        ));
        token = body["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, filtered,
        "case filtered to one account: expected its resources once in ARN order, got {seen:?}"
    );
}

#[tokio::test]
async fn a_next_token_this_read_did_not_issue_is_refused() {
    let (router, engine) = emulator();
    enrol_member_accounts(&engine);

    // A token THIS read issued, and ones other lists issued.
    let (_, first_page) = call(
        &router,
        "GET",
        &format!("{}?maxResults=1", resources_path(CHECK)),
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
    // issued; this read's own token under a FILTER it was not issued under; and
    // its own token on ANOTHER organization recommendation's resources, a
    // different sequence under the same operation.
    let refused: &[(&str, String)] = &[
        (
            "a token this service never issued",
            format!("{}?nextToken=%21%21%21%21", resources_path(CHECK)),
        ),
        (
            "a ListChecks token",
            format!(
                "{}?nextToken={}",
                resources_path(CHECK),
                label(&checks_token)
            ),
        ),
        (
            "its own token under an affectedAccountId filter",
            format!(
                "{}?affectedAccountId=111122223333&nextToken={}",
                resources_path(CHECK),
                label(&own_token)
            ),
        ),
        (
            "its own token under a status filter",
            format!(
                "{}?status=error&nextToken={}",
                resources_path(CHECK),
                label(&own_token)
            ),
        ),
        (
            "its own token on another organization recommendation's resources",
            format!(
                "{}?nextToken={}",
                resources_path("amazon-ebs-snapshots"),
                label(&own_token)
            ),
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
    let mut every_arn: Vec<String> = stored_rows().iter().map(arn_of).collect();
    every_arn.sort();
    let arns = listed_arns(
        &router,
        "the issuing read accepts its own token",
        &format!(
            "{}?maxResults=1&nextToken={}",
            resources_path(CHECK),
            label(&own_token)
        ),
    )
    .await;
    assert_eq!(
        arns,
        vec![every_arn[1].clone()],
        "case the issuing read accepts its own token: expected the second resource, got {arns:?}"
    );
}

#[tokio::test]
async fn an_exclusion_made_through_the_api_is_what_the_next_read_reports() {
    let (router, _engine) = emulator();
    let check = "amazon-ebs-snapshots";
    let first = resource_arn(check, "res-ebs-0001");
    let second = resource_arn(check, "res-ebs-0002");

    /// One row: the `BatchUpdateRecommendationResourceExclusion` entries to
    /// write first (none, for the seeded store), and the exclusion state the
    /// list must then report for `res-ebs-0001` and `res-ebs-0002`.
    struct Row {
        case: &'static str,
        entries: Vec<(String, bool)>,
        expected: [&'static str; 2],
    }

    // Rows: each exclusion state written on each resource, one entry at a time
    // and both in one call, so the expectation differs along which resource
    // carries which state. The seed leaves `res-ebs-0001` included and
    // `res-ebs-0002` excluded.
    let rows = vec![
        Row {
            case: "the seeded store",
            entries: Vec::new(),
            expected: ["included", "excluded"],
        },
        Row {
            case: "the first resource excluded",
            entries: vec![(first.clone(), true)],
            expected: ["excluded", "excluded"],
        },
        Row {
            case: "both resources included in one call",
            entries: vec![(first.clone(), false), (second.clone(), false)],
            expected: ["included", "included"],
        },
        Row {
            case: "the second resource excluded again",
            entries: vec![(second.clone(), true)],
            expected: ["included", "excluded"],
        },
    ];

    for Row {
        case,
        entries,
        expected,
    } in rows
    {
        if !entries.is_empty() {
            let payload = json!({
                "recommendationResourceExclusions": entries
                    .iter()
                    .map(|(arn, is_excluded)| json!({ "arn": arn, "isExcluded": is_excluded }))
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

        let reported = listed(&router, case, &resources_path(check)).await;
        let states: Vec<&str> = reported
            .iter()
            .map(|summary| summary["exclusionStatus"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            states,
            expected.to_vec(),
            "case {case}: expected exclusion states {expected:?}, got {states:?}"
        );

        // The same change the organization recommendation's own
        // `resourcesAggregates` counts: the list and the counters report one
        // store.
        let (status, organization) = call(&router, "GET", &get_path(check), None).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from GetOrganizationRecommendation, got {status} with \
             {organization}"
        );
        let counted = organization["organizationRecommendation"]["resourcesAggregates"]
            ["excludedCount"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: no excludedCount in {organization}"));
        let listed_excluded =
            i64::try_from(states.iter().filter(|state| **state == "excluded").count())
                .expect("a small count");
        assert_eq!(
            counted, listed_excluded,
            "case {case}: expected excludedCount {listed_excluded}, got {counted}"
        );

        // The `exclusionStatus` filter selects exactly the excluded resources
        // the list just reported.
        let excluded = listed_arns(
            &router,
            case,
            &format!("{}?exclusionStatus=excluded", resources_path(check)),
        )
        .await;
        assert_eq!(
            i64::try_from(excluded.len()).expect("a small count"),
            listed_excluded,
            "case {case}: expected the filter to keep {listed_excluded} resources, got \
             {excluded:?}"
        );
    }
}

#[tokio::test]
async fn an_identifier_that_names_nothing_is_not_found_and_a_bad_input_is_refused() {
    let (router, _engine) = emulator();

    // Rows differ along what makes the ARN name nothing: an all-zeros id, an id
    // no seed produced, a seeded id in another partition, and a seeded id with
    // the account-level suffix.
    let unknown = [
        (
            "an all-zeros organization recommendation id",
            "arn:aws:trustedadvisor:::organization-recommendation/00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "an id no seed produced",
            "arn:aws:trustedadvisor:::organization-recommendation/not-a-seeded-one".to_string(),
        ),
        (
            "a seeded id in another partition",
            format!(
                "arn:aws-us-gov:trustedadvisor:::organization-recommendation/{CHECK}-organization-recommendation"
            ),
        ),
        (
            "the account-level recommendation id",
            format!("arn:aws:trustedadvisor:::organization-recommendation/{CHECK}-recommendation"),
        ),
    ];
    for (case, arn) in unknown {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/v1/organization-recommendations/{}/resources", label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {body}"
        );
    }

    // Rows outside a modelled constraint, refused ahead of the handler: the
    // identifier's pattern, `maxResults`'s `range 1..200` on each side of each
    // published bound, `nextToken`'s `length 4..10000` minimum,
    // `affectedAccountId`'s `length 12..12` and `pattern ^\d+$`, and a value
    // outside each filtered enum.
    let path = resources_path(CHECK);
    let refused: &[(&str, String)] = &[
        (
            "an identifier outside the modelled pattern",
            "/v1/organization-recommendations/arn:aws:trustedadvisor:::not-one/resources"
                .to_string(),
        ),
        (
            "maxResults below the modelled range",
            format!("{path}?maxResults=0"),
        ),
        (
            "maxResults past the modelled range",
            format!("{path}?maxResults=201"),
        ),
        (
            "a nextToken shorter than the modelled minimum",
            format!("{path}?nextToken=abc"),
        ),
        (
            "an affectedAccountId of eleven digits",
            format!("{path}?affectedAccountId=11112222333"),
        ),
        (
            "an affectedAccountId of thirteen digits",
            format!("{path}?affectedAccountId=1111222233334"),
        ),
        (
            "an affectedAccountId that is not digits",
            format!("{path}?affectedAccountId=notadigits12"),
        ),
        (
            "a status outside ResourceStatus",
            format!("{path}?status=not-a-real-status"),
        ),
        (
            "an exclusionStatus outside ExclusionStatus",
            format!("{path}?exclusionStatus=all"),
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

    // Each published bound of `maxResults` is itself accepted, and the smaller
    // one truncates.
    let seeded: Vec<String> = SEEDED
        .iter()
        .find(|row| row.check_id == CHECK)
        .expect("the seeded rows name CHECK")
        .resources
        .iter()
        .map(|resource| resource_arn(CHECK, resource.id))
        .collect();
    for (page_size, expected) in [(1usize, &seeded[..1]), (200, &seeded[..])] {
        let case = format!("maxResults at the bound {page_size}");
        let arns = listed_arns(&router, &case, &format!("{path}?maxResults={page_size}")).await;
        assert_eq!(
            arns,
            expected.to_vec(),
            "case {case}: expected {expected:?}, got {arns:?}"
        );
    }
}

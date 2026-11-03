// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE view of a stored recommendation resource BOTH
//! resource lists of this service report — `ListRecommendationResources` over an
//! account-level recommendation's resources and
//! `ListOrganizationRecommendationResources` over the member accounts' resources
//! an organization recommendation aggregates — driven over HTTP through the same
//! `MultiServiceRouter` the emulator serves with.
//!
//! The rule, over a TABLE of filter combinations rather than one call per
//! operation: the two lists report ONE projection of a stored resource, so every
//! member both summaries declare carries that resource's own stored value in
//! each view, and one predicate selects a resource for both reads — a `status`,
//! `exclusionStatus` or `regionCode` in force keeps exactly the resources whose
//! own member equals it, and an absent filter keeps every resource. The two
//! views differ only in the members modelled differently: `recommendationArn`,
//! which each list reports as the recommendation the caller asked about, and
//! `accountId`, which only the organization summary declares and which is the
//! account segment of the resource's own ARN.
//!
//! The member lists below come from the two shapes —
//! `smithy_query(query="shape",shape="RecommendationResourceSummary,OrganizationRecommendationResourceSummary",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
//! — and the filter values the rows span from the model's own enums
//! (`RESOURCE_STATUS_VALUES` and `EXCLUSION_STATUS_VALUES` in
//! `ops::types::shared_types`); the wire contract of each read is pinned by
//! `tests/all/recommendation_loop.rs` and
//! `tests/all/organization_recommendation_resources.rs`.
//!
//! Every call is made as an account whose support plan includes the Trusted
//! Advisor API; the gate ahead of every handler is pinned in
//! `tests/all/support_level_gate.rs`.

use serde_json::Value;
use service_com_amazonaws_trustedadvisor_trustedadvisor::ops::types::{
    EXCLUSION_STATUS_VALUES, RESOURCE_STATUS_VALUES,
};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{service_state, SupportPlan};
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_recommendation_lifecycle::organization_recommendation_arn;
use crate::recommendation_loop::{call, emulator_on_plan, label, recommendation_arn, resource_arn};

/// The check both lists are driven over: its recommendation is prioritized, so
/// it has an organization recommendation aggregating the one member account's
/// copy — the same stored resources seen at both levels.
const CHECK: &str = "low-utilization-ec2-instances";

/// The members `RecommendationResourceSummary` and
/// `OrganizationRecommendationResourceSummary` BOTH declare, which is what the
/// shared projection of a stored resource fills. Source: the two shapes in this
/// service's model (see the module docs above); the members outside this list
/// are `recommendationArn` on both and `accountId` on the organization summary.
const SHARED_MEMBERS: &[&str] = &[
    "id",
    "arn",
    "awsResourceId",
    "regionCode",
    "status",
    "metadata",
    "lastUpdatedAt",
    "exclusionStatus",
];

/// One resource of [`CHECK`] as the store holds it for this suite: the values
/// the three shared filters cut on.
struct Stored {
    id: &'static str,
    status: &'static str,
    region_code: &'static str,
    exclusion_status: &'static str,
}

/// The stored resources the rows below are derived from: the three seeded
/// resources of [`CHECK`], moved so they span every `ResourceStatus` value and
/// both `ExclusionStatus` values and sit in three Regions — so each filter has
/// matching and non-matching rows at both levels.
const STORED: &[Stored] = &[
    Stored {
        id: "res-ec2-0001",
        status: "warning",
        region_code: "us-east-1",
        exclusion_status: "included",
    },
    Stored {
        id: "res-ec2-0002",
        status: "ok",
        region_code: "us-west-2",
        exclusion_status: "excluded",
    },
    Stored {
        id: "res-ec2-0003",
        status: "error",
        region_code: "eu-west-1",
        exclusion_status: "included",
    },
];

/// The two reads of one stored resource.
#[derive(Debug, Clone, Copy)]
enum Level {
    Account,
    Organization,
}

impl Level {
    /// Both levels, so every row of the table is driven through each read.
    const ALL: &'static [Self] = &[Self::Account, Self::Organization];

    /// The modelled URI of this level's read of [`CHECK`]'s resources, with the
    /// row's filters as its query.
    fn path(self, query: &str) -> String {
        let (collection, identifier) = match self {
            Self::Account => ("recommendations", recommendation_arn(CHECK)),
            Self::Organization => (
                "organization-recommendations",
                organization_recommendation_arn(CHECK),
            ),
        };
        format!("/v1/{collection}/{}/resources{query}", label(&identifier))
    }

    /// The response member this level wraps its summaries in.
    fn summaries_member(self) -> &'static str {
        match self {
            Self::Account => "recommendationResourceSummaries",
            Self::Organization => "organizationRecommendationResourceSummaries",
        }
    }

    /// The `recommendationArn` this level's summaries carry: the recommendation
    /// the caller asked about, which is the member modelled differently on the
    /// two summaries.
    fn reported_recommendation_arn(self) -> String {
        match self {
            Self::Account => recommendation_arn(CHECK),
            Self::Organization => organization_recommendation_arn(CHECK),
        }
    }
}

/// A mounted emulator whose [`CHECK`] resources are the [`STORED`] rows.
///
/// The statuses, Regions and exclusion states are written in process, the way
/// the part of Trusted Advisor that runs a check would: no modelled operation of
/// this API creates a recommendation resource or moves its status.
fn emulator_with_stored_rows() -> MultiServiceRouter {
    let (router, engine) = emulator_on_plan(SupportPlan::Business);
    let state = service_state(&*engine).expect("the in-memory engine carries this service's state");
    let mut state = state.lock();
    for row in STORED {
        let arn = resource_arn(CHECK, row.id);
        let resource = state
            .recommendation_resources
            .get_mut(&arn)
            .unwrap_or_else(|| panic!("no seeded resource {arn}"));
        resource.status = row.status.to_string();
        resource.region_code = row.region_code.to_string();
        resource.exclusion_status = row.exclusion_status.to_string();
    }
    drop(state);
    router
}

/// The summaries one level's read reports, asserting 200 first.
async fn summaries(
    router: &MultiServiceRouter,
    case: &str,
    level: Level,
    query: &str,
) -> Vec<Value> {
    let (status, body) = call(router, "GET", &level.path(query), None).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from the {level:?}-level read, got {status} with {body}"
    );
    body[level.summaries_member()]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a {} array, got {body}",
                level.summaries_member()
            )
        })
        .clone()
}

/// The `arn` of every summary in a read's answer.
fn arns_of(case: &str, reported: &[Value]) -> Vec<String> {
    reported
        .iter()
        .map(|summary| {
            summary["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a summary with no arn: {summary}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn both_resource_lists_select_the_same_resources_with_the_same_filters() {
    let router = emulator_with_stored_rows();

    // Rows differ along which filter is in force, the value it names, the
    // combination of two filters and the number of matches (all, several, one,
    // none). Each expectation is derived from the stored set by the SAME rule
    // the request names, then put into the resource-ARN order both reads report.
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, Box<dyn Fn(&Stored) -> bool>)> = vec![
        (String::new(), Box::new(|_: &Stored| true)),
        (
            "?status=warning".to_string(),
            Box::new(|row: &Stored| row.status == "warning"),
        ),
        (
            "?status=ok".to_string(),
            Box::new(|row: &Stored| row.status == "ok"),
        ),
        (
            "?status=error".to_string(),
            Box::new(|row: &Stored| row.status == "error"),
        ),
        (
            "?exclusionStatus=included".to_string(),
            Box::new(|row: &Stored| row.exclusion_status == "included"),
        ),
        (
            "?exclusionStatus=excluded".to_string(),
            Box::new(|row: &Stored| row.exclusion_status == "excluded"),
        ),
        (
            "?regionCode=us-east-1".to_string(),
            Box::new(|row: &Stored| row.region_code == "us-east-1"),
        ),
        (
            "?regionCode=eu-west-1".to_string(),
            Box::new(|row: &Stored| row.region_code == "eu-west-1"),
        ),
        // A Region no stored resource is in: both reads keep nothing.
        (
            "?regionCode=sa-east-1".to_string(),
            Box::new(|row: &Stored| row.region_code == "sa-east-1"),
        ),
        // The equality is exact, so a Region code differing only in case keeps
        // nothing either.
        (
            "?regionCode=US-EAST-1".to_string(),
            Box::new(|row: &Stored| row.region_code == "US-EAST-1"),
        ),
        (
            "?status=warning&exclusionStatus=included".to_string(),
            Box::new(|row: &Stored| row.status == "warning" && row.exclusion_status == "included"),
        ),
        (
            "?status=warning&regionCode=eu-west-1".to_string(),
            Box::new(|row: &Stored| row.status == "warning" && row.region_code == "eu-west-1"),
        ),
        (
            "?status=ok&exclusionStatus=included".to_string(),
            Box::new(|row: &Stored| row.status == "ok" && row.exclusion_status == "included"),
        ),
        (
            "?status=error&exclusionStatus=excluded&regionCode=eu-west-1".to_string(),
            Box::new(|row: &Stored| {
                row.status == "error"
                    && row.exclusion_status == "excluded"
                    && row.region_code == "eu-west-1"
            }),
        ),
    ];

    // The rows span every value the two filtered enums declare, so a value added
    // to either is not silently left untested.
    for status in RESOURCE_STATUS_VALUES {
        assert!(
            rows.iter()
                .any(|(query, _)| query.contains(&format!("status={status}"))),
            "the table must cover the resource status {status}"
        );
    }
    for exclusion in EXCLUSION_STATUS_VALUES {
        assert!(
            rows.iter()
                .any(|(query, _)| query.contains(&format!("exclusionStatus={exclusion}"))),
            "the table must cover the exclusion status {exclusion}"
        );
    }

    for (query, keep) in &rows {
        let mut expected: Vec<String> = STORED
            .iter()
            .filter(|row| keep(row))
            .map(|row| resource_arn(CHECK, row.id))
            .collect();
        expected.sort();

        // What each level reports under this row's filters, to be compared
        // against the rule AND against each other.
        let mut reported_levels = Vec::new();
        for level in Level::ALL {
            let case = format!("{level:?} {query:?}");
            let reported = summaries(&router, &case, *level, query).await;
            assert_eq!(
                arns_of(&case, &reported),
                expected,
                "case {case}: expected the filters to keep {expected:?}, got {reported:?}"
            );
            reported_levels.push(reported);
        }

        // One resource, one projection: every member both summaries declare
        // carries the same value in the two views.
        let case = format!("{query:?}");
        let (account, organization) = (&reported_levels[0], &reported_levels[1]);
        for (account_summary, organization_summary) in account.iter().zip(organization) {
            for member in SHARED_MEMBERS {
                assert_eq!(
                    account_summary[member], organization_summary[member],
                    "case {case}: expected both views of {:?} to report the same {member}, got \
                     {:?} and {:?}",
                    account_summary["arn"], account_summary[member], organization_summary[member]
                );
            }
        }
    }
}

#[tokio::test]
async fn the_two_views_of_one_resource_differ_only_where_they_are_modelled_to() {
    let router = emulator_with_stored_rows();
    // Two rows: the unfiltered read and a filtered one, so the differing
    // members are checked on a whole page and on a cut of it.
    for query in ["", "?status=warning"] {
        for level in Level::ALL {
            let case = format!("{level:?} {query:?}");
            let reported = summaries(&router, &case, *level, query).await;
            assert!(
                !reported.is_empty(),
                "case {case}: expected the read to report a resource, got {reported:?}"
            );
            for summary in &reported {
                let case = format!("{case}/{:?}", summary["arn"]);
                assert_eq!(
                    summary["recommendationArn"].as_str(),
                    Some(level.reported_recommendation_arn().as_str()),
                    "case {case}: expected the recommendation the caller asked about, got {:?}",
                    summary["recommendationArn"]
                );
                // `accountId` is declared only by the organization summary, and
                // it is the account segment of the resource's own ARN.
                let owning_account = summary["arn"]
                    .as_str()
                    .and_then(|arn| arn.split(':').nth(4))
                    .unwrap_or_else(|| panic!("case {case}: a summary with no resource ARN"));
                let expected_account = match level {
                    Level::Account => None,
                    Level::Organization => Some(owning_account),
                };
                assert_eq!(
                    summary["accountId"].as_str(),
                    expected_account,
                    "case {case}: expected accountId {expected_account:?}, got {:?}",
                    summary["accountId"]
                );
                assert_eq!(
                    owning_account,
                    engine::EMULATOR_ACCOUNT_ID,
                    "case {case}: expected the calling account's own resource, got \
                     {owning_account}"
                );
            }
        }
    }
}

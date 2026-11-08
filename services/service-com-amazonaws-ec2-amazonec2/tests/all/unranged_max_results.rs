// SPDX-License-Identifier: CC0-1.0

//! Where a paged describe of this service bounds its page size, and where it does not —
//! end to end over the wire the service speaks (`ec2Query`, through the shared
//! [`super::ec2_query`] harness).
//!
//! The rule under test: a paged describe refuses a `MaxResults` outside its bounds only
//! where those bounds are PUBLISHED — by the model's own `@range`, or by the member's own
//! documentation; where neither publishes one, a page size above the largest page this
//! service serves asks for THAT page — the answer is one page plus a `NextToken` for the
//! rest, not a refusal. A page of NONE is not a page, and stays refused.
//!
//! Each row is one operation and one page size, and the rows differ along what the rule
//! generalises over: whether the operation's `MaxResults` carries a modelled `@range`,
//! which bound a size sits on (each published bound, one past it, far past it), and which
//! operation of this service is asked. No row is the specification. The operations that
//! publish a page-size MINIMUM without a modelled range are covered by
//! [`super::published_parameter_limits`].

use ec2::state::{MAX_PAGE_SIZE, availability_zones_in_region};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code};

/// The Region every call here is signed for. Each operation below reports per-account or
/// per-Region state whose page size does not depend on the Region, so the Region only
/// selects the ec2Query envelope and the zone a volume is created in.
const REGION: &str = "us-west-2";

/// The largest page any describe of this service answers, read from the service rather
/// than written here so the two cannot drift.
fn largest_page() -> i64 {
    i64::try_from(MAX_PAGE_SIZE).unwrap_or(i64::MAX)
}

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The paged describes whose `MaxResults` the MODEL leaves unbounded above — the member
/// targets a plain `Integer` with no `@range`, so nothing in the contract bounds the page
/// size:
///
/// ```text
/// smithy_query(query="operation",operation="DescribeImages,DescribeVolumeStatus,DescribePrefixLists,DescribeImportImageTasks,DescribeImportSnapshotTasks,DescribeSpotInstanceRequests,DescribeSpotFleetRequests,DescribeSpotPriceHistory,DescribeVpcEndpointConnectionNotifications",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
///
/// `DescribeVolumesModifications` is deliberately NOT here: the model declares no range for
/// it either, but its own member documents "the maximum number of results (up to a limit of
/// 500)" — a bound the service publishes — so that operation keeps its refusal.
/// `DescribeInstances` and `DescribeVolumes` are out for the same reason, on the other
/// bound: AWS publishes a minimum of 5 for each one's `MaxResults`, which
/// [`PUBLISHED_MINIMUM_OPERATIONS`] covers.
const UNRANGED_PAGE_SIZE_OPERATIONS: [&str; 9] = [
    "DescribeImages",
    "DescribeVolumeStatus",
    "DescribePrefixLists",
    "DescribeImportImageTasks",
    "DescribeImportSnapshotTasks",
    "DescribeSpotInstanceRequests",
    "DescribeSpotFleetRequests",
    "DescribeSpotPriceHistory",
    "DescribeVpcEndpointConnectionNotifications",
];

/// The paged describes whose `MaxResults` the MODEL leaves unbounded but whose own
/// documentation publishes a MINIMUM page size, with that minimum: `DescribeInstances` and
/// `DescribeVolumes` each target a plain `Integer`, and the EC2 API Reference publishes
/// "Valid Range: Minimum value of 5" for both members (maximum 1000 and 500), so a smaller
/// page is refused rather than served.
const PUBLISHED_MINIMUM_OPERATIONS: [(&str, i64); 2] =
    [("DescribeInstances", 5), ("DescribeVolumes", 5)];

/// The paged describes whose `MaxResults` the model DOES bound, with the bounds it
/// declares, from the same `smithy_query` call as above: `DescribeSecurityGroupsMaxResults`
/// and `DescribeSubnetsMaxResults` carry `@range(min: 5, max: 1000)`, and
/// `DescribeFastLaunchImagesRequestMaxResults` carries `@range(min: 0, max: 200)`.
const RANGED_PAGE_SIZE_OPERATIONS: [(&str, i64, i64); 3] = [
    ("DescribeSecurityGroups", 5, 1_000),
    ("DescribeSubnets", 5, 1_000),
    ("DescribeFastLaunchImages", 0, 200),
];

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One describe carrying `MaxResults`, as its HTTP status and body.
async fn with_page_size(
    router: &MultiServiceRouter,
    action: &str,
    max_results: i64,
) -> (u16, String) {
    call(
        router,
        REGION,
        action,
        &[p("MaxResults", max_results.to_string())],
    )
    .await
}

/// A page size the model leaves unbounded above is SERVED at the ceiling and past it: the
/// operation answers one page rather than refusing the request.
#[tokio::test]
async fn a_page_size_above_the_ceiling_is_served_where_the_model_declares_no_range() {
    let router = emulator();

    // Rows: the smallest page, the largest this service serves, one past it, well past it,
    // and the largest value the member's `Integer` can carry.
    let sizes = [
        1_i64,
        largest_page(),
        largest_page() + 1,
        100_000,
        i64::from(i32::MAX),
    ];
    for action in UNRANGED_PAGE_SIZE_OPERATIONS {
        for max_results in sizes {
            let (status, body) = with_page_size(&router, action, max_results).await;
            assert_eq!(
                status, 200,
                "case {action} MaxResults={max_results}: expected HTTP 200, got {status} with \
                 body {body}"
            );
        }
    }
}

/// A page of NONE is still refused, with EC2's own code: a request for zero items is not a
/// request for a page.
#[tokio::test]
async fn a_page_of_none_is_refused_with_ec2s_own_code() {
    let router = emulator();

    for action in UNRANGED_PAGE_SIZE_OPERATIONS {
        for max_results in [0_i64, -1] {
            let (status, body) = with_page_size(&router, action, max_results).await;
            assert_eq!(
                status, 400,
                "case {action} MaxResults={max_results}: expected HTTP 400, got {status} with \
                 body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {action} MaxResults={max_results}: expected InvalidParameterValue, got \
                 {body}"
            );
        }
    }
}

/// Where the operation's own documentation publishes a MINIMUM page size, that minimum is
/// served and anything below it is refused with EC2's `InvalidParameterValue` — a page size
/// the operation will not page with is a value it cannot use.
///
/// The rows are the published bound and the values below it: the minimum itself, one below
/// it, well below it but still a page, and the two that ask for no page at all.
#[tokio::test]
async fn a_published_minimum_is_served_and_anything_below_it_is_refused() {
    let router = emulator();

    for (action, minimum) in PUBLISHED_MINIMUM_OPERATIONS {
        for max_results in [minimum, minimum + 1, largest_page()] {
            let (status, body) = with_page_size(&router, action, max_results).await;
            assert_eq!(
                status, 200,
                "case {action} MaxResults={max_results} (at or above the published minimum): \
                 expected HTTP 200, got {status} with body {body}"
            );
        }
        for max_results in [minimum - 1, 1, 0, -1] {
            let (status, body) = with_page_size(&router, action, max_results).await;
            assert_eq!(
                status, 400,
                "case {action} MaxResults={max_results} (below the published minimum): \
                 expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {action} MaxResults={max_results}: expected InvalidParameterValue, got \
                 {body}"
            );
        }
    }
}

/// Where the model DOES declare a range, each published bound is served and one past
/// either bound is refused — that bound comes from the contract, not from this service's
/// own page ceiling.
#[tokio::test]
async fn a_modelled_range_is_served_at_its_bounds_and_refused_past_them() {
    let router = emulator();

    for (action, minimum, maximum) in RANGED_PAGE_SIZE_OPERATIONS {
        for max_results in [minimum, maximum] {
            let (status, body) = with_page_size(&router, action, max_results).await;
            assert_eq!(
                status, 200,
                "case {action} MaxResults={max_results} (a published bound): expected HTTP \
                 200, got {status} with body {body}"
            );
        }
        for max_results in [minimum - 1, maximum + 1] {
            let (status, body) = with_page_size(&router, action, max_results).await;
            assert_eq!(
                status, 400,
                "case {action} MaxResults={max_results} (past a published bound): expected \
                 HTTP 400, got {status} with body {body}"
            );
        }
    }
}

/// A page size above the ceiling reports EVERY item the store holds and carries no token,
/// and a size BELOW the store still cuts the page: removing the ceiling refusal did not
/// remove the paging.
#[tokio::test]
async fn a_page_size_above_the_ceiling_holds_everything_and_carries_no_token() {
    let router = emulator();

    // Volumes, because this suite can create as many of them as it needs: six, which is
    // below every page size asserted below, so each of those pages holds all six, and above
    // the page size the cut below asks for.
    let zone = availability_zones_in_region(REGION)[0].zone_name.clone();
    let mut created: Vec<String> = Vec::new();
    for size in [4_i32, 8, 12, 16, 20, 24] {
        let (status, body) = call(
            &router,
            REGION,
            "CreateVolume",
            &[p("AvailabilityZone", zone.clone()), p("Size", size.to_string())],
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding a volume of {size} GiB: expected HTTP 200, got {status} with body {body}"
        );
        created.push(
            element(&body, "volumeId")
                .unwrap_or_else(|| panic!("the created volume must report its id: {body}")),
        );
    }
    created.sort();

    for max_results in [largest_page(), largest_page() + 1, i64::from(i32::MAX)] {
        let (status, body) = with_page_size(&router, "DescribeVolumes", max_results).await;
        assert_eq!(
            status, 200,
            "case MaxResults={max_results}: expected HTTP 200, got {status} with body {body}"
        );
        let mut reported = elements(&body, "volumeId");
        reported.sort();
        assert_eq!(
            reported, created,
            "case MaxResults={max_results}: expected every stored volume {created:?}, got \
             {reported:?}"
        );
        assert_eq!(
            element(&body, "nextToken"),
            None,
            "case MaxResults={max_results}: a page that holds everything carries no token, got \
             {body}"
        );
    }

    let (status, body) = with_page_size(&router, "DescribeVolumes", 5).await;
    assert_eq!(
        status, 200,
        "case MaxResults=5: expected HTTP 200, got {status} with body {body}"
    );
    let page = elements(&body, "volumeId");
    assert_eq!(
        page.len(),
        5,
        "case MaxResults=5: expected a page of five volumes, got {page:?} in {body}"
    );
    assert!(
        element(&body, "nextToken").is_some(),
        "case MaxResults=5: a sixth volume follows the page, so a token is due, got {body}"
    );
}

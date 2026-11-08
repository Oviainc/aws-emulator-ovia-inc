// SPDX-License-Identifier: CC0-1.0

//! How `DescribeInstances` PAGES, driven over the wire the service speaks (`ec2Query`, an
//! `application/x-www-form-urlencoded` POST answered with XML; README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! The rule under test: `MaxResults` CAPS THE ITEMS ON A PAGE, and the items a caller of
//! this operation counts are the INSTANCES — "the maximum number of items to return for
//! this request. To get the next page of items, make another request with the token
//! returned in the output" (`DescribeInstancesRequest$MaxResults`, the operation's
//! `@paginated` page size). So a page never reports more instances than the request asked
//! for, a `NextToken` is answered exactly while further instances remain, and following the
//! token covers every selected instance exactly once, in order, however the instances are
//! spread over reservations — a reservation whose instances straddle a page boundary is
//! reported on both pages, with the instances that fall in each.
//!
//! The rows span what that generalises over: the page size (the smallest page the operation
//! pages with, a size that divides the set, one that does not, one larger than the set, and
//! the largest the service serves) against two shapes of store — instances spread one per
//! reservation and several per reservation — so a page boundary falls inside a reservation in
//! some rows and between reservations in others.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, elements, filters};
use super::wire::{param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The image every launch names; which image an instance was launched from has no bearing
/// on how the describe pages.
const AMI: &str = "ami-0123456789abcdef0";

/// One call that must succeed, as its body.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Launch `count` instances in ONE reservation and answer their ids.
async fn launch(case: &str, router: &MultiServiceRouter, count: u32) -> Vec<String> {
    let body = ok(
        case,
        router,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("MinCount", &count.to_string()),
            param("MaxCount", &count.to_string()),
        ],
    )
    .await;
    let ids = elements(&body, "instanceId");
    assert_eq!(
        ids.len(),
        count as usize,
        "case {case:?}: expected {count} instance(s) from RunInstances, got {ids:?}"
    );
    ids
}

/// One page: the instance ids it reported, the reservation ids it reported them under, and
/// its `NextToken`.
struct Page {
    instance_ids: Vec<String>,
    reservation_ids: Vec<String>,
    next_token: Option<String>,
}

/// One `DescribeInstances` page, with `MaxResults` and an optional `NextToken`.
async fn page(
    case: &str,
    router: &MultiServiceRouter,
    max_results: Option<u32>,
    next_token: Option<&str>,
) -> Page {
    let mut params: Vec<Param> = Vec::new();
    if let Some(max_results) = max_results {
        params.push(param("MaxResults", &max_results.to_string()));
    }
    if let Some(token) = next_token {
        params.push(param("NextToken", token));
    }
    let body = ok(case, router, "DescribeInstances", &params).await;
    Page {
        instance_ids: elements(&body, "instanceId"),
        reservation_ids: elements(&body, "reservationId"),
        next_token: super::ec2_query::member(&body, "nextToken"),
    }
}

/// Walking the pages at any page size covers every instance exactly once, in the order an
/// unpaged read reports them, and no page is larger than the caller asked for.
#[tokio::test]
async fn max_results_caps_a_page_and_the_token_walks_to_the_full_set() {
    // Rows: how the store's instances are spread over reservations. The first row is six
    // instances in six reservations, the second six in two, the third six in one — so a
    // page boundary falls between reservations, inside one, and inside a single
    // reservation holding every instance.
    for launches in [vec![1, 1, 1, 1, 1, 1], vec![4, 2], vec![6]] {
        let setup = format!("reservations of {launches:?}");
        let (router, _engine) = super::ec2_query::emulator();
        let mut launched: Vec<String> = Vec::new();
        for count in &launches {
            launched.extend(launch(&setup, &router, *count).await);
        }

        let whole = page(&setup, &router, None, None).await;
        assert_eq!(
            whole.instance_ids, launched,
            "case {setup:?}: an unpaged read reports every instance in launch order, \
             got {:?}",
            whole.instance_ids
        );
        assert_eq!(
            whole.next_token, None,
            "case {setup:?}: a read that asked for no page size carries no token, got {:?}",
            whole.next_token
        );

        // Rows: the smallest page this operation pages with (`DescribeInstances` publishes
        // a minimum of 5), one that divides the set, one larger than the set, and the
        // largest page the service serves. A page size BELOW that minimum is refused rather
        // than served, which `published_page_size_minimums` covers.
        for max_results in [5_u32, 6, 7, 1000] {
            let case = format!("{setup} with MaxResults={max_results}");
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let answered = page(&case, &router, Some(max_results), token.as_deref()).await;
                pages += 1;
                assert!(
                    answered.instance_ids.len() <= max_results as usize,
                    "case {case:?}: page {pages} reported {} instances, more than the \
                     {max_results} asked for: {:?}",
                    answered.instance_ids.len(),
                    answered.instance_ids
                );
                assert!(
                    answered.reservation_ids.len() <= answered.instance_ids.len(),
                    "case {case:?}: page {pages} reported {} reservations for {} \
                     instances, so one carried none: {:?}",
                    answered.reservation_ids.len(),
                    answered.instance_ids.len(),
                    answered.reservation_ids
                );
                walked.extend(answered.instance_ids.clone());
                let remaining = launched.len().saturating_sub(walked.len());
                assert_eq!(
                    answered.next_token.is_some(),
                    remaining > 0,
                    "case {case:?}: page {pages} must carry a token exactly while \
                     instances remain ({remaining} left), got {:?}",
                    answered.next_token
                );
                match answered.next_token {
                    Some(next) => token = Some(next),
                    None => break,
                }
                assert!(
                    pages <= launched.len() + 1,
                    "case {case:?}: the walk must end, got {pages} pages for {} instances",
                    launched.len()
                );
            }
            assert_eq!(
                walked, launched,
                "case {case:?}: the walk must cover every instance exactly once, in order"
            );
            let expected_pages = launched.len().div_ceil(max_results.max(1) as usize);
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} page(s) for {} instances at \
                 {max_results} per page, got {pages}",
                launched.len()
            );
        }
    }
}

/// The first page of a filtered read is capped the same way: the cap is on the instances
/// the request SELECTED, not on the store.
#[tokio::test]
async fn a_filtered_read_is_capped_over_the_instances_it_selected() {
    let (router, _engine) = super::ec2_query::emulator();
    let first = launch("the first reservation", &router, 8).await;
    let second = launch("the second reservation", &router, 8).await;
    let reservation = super::ec2_query::element(
        &ok(
            "the first reservation's instances",
            &router,
            "DescribeInstances",
            &filters(&[("instance-id", &[first[0].as_str()])]),
        )
        .await,
        "reservationId",
    )
    .unwrap_or_default();

    // Rows: a page size below, at and above the SELECTED set's size, where the store holds
    // twice as many instances as the filter selects. Each is at or above the smallest page
    // this operation pages with.
    for (max_results, expected_first_page, expect_token) in
        [(5_u32, 5_usize, true), (8, 8, false), (10, 8, false)]
    {
        let case = format!("reservation-id with MaxResults={max_results}");
        let mut params = filters(&[("reservation-id", &[reservation.as_str()])]);
        params.push(param("MaxResults", &max_results.to_string()));
        let body = ok(&case, &router, "DescribeInstances", &params).await;
        let reported = elements(&body, "instanceId");
        assert_eq!(
            reported.len(),
            expected_first_page,
            "case {case:?}: expected {expected_first_page} instance(s) on the first page, \
             got {reported:?}"
        );
        assert!(
            reported.iter().all(|id| first.contains(id)),
            "case {case:?}: the filter must select only the first reservation's \
             instances, got {reported:?} against {second:?}"
        );
        assert_eq!(
            super::ec2_query::member(&body, "nextToken").is_some(),
            expect_token,
            "case {case:?}: the token is answered exactly while selected instances \
             remain, got {body}"
        );
    }
}

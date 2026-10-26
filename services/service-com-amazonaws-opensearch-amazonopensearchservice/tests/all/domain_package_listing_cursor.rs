// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how `ListPackagesForDomain` pages a domain's package
//! associations, and for the fact that paging it reports only.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. This suite adds no second copy of anything
//! the package suites already have: the harness comes from [`super::domain_lifecycle`],
//! the package helpers from [`super::package_associations`], the listing request
//! helpers and the published page-size bound from
//! [`super::package_listing_page_bounds`], and the dissociation request from
//! [`super::package_dissociation`]. Only the walk over a DOMAIN's listing is local —
//! the shared walk pages the sibling listing (a package's domains) and reports domain
//! names rather than package ids.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a token names a PLACE in the domain's set, so the page size that resumes it
//!   changes how the remainder is cut into pages and not which associations it
//!   reports, over a table crossing the page size a token was issued at with the one
//!   it is resumed at — and one token answers the same records however often it is
//!   used;
//! * paging a domain's packages is a report: served, paged and refused reads — each
//!   answered with the status and error code the model declares for it — leave every
//!   record a later caller sees exactly as it was;
//! * the pages follow the STORE, so once the clock has taken associations off the
//!   domain a walk reports exactly the links that are left, at every page size.

use serde_json::json;

use super::domain_lifecycle::{create_baseline, error_code, Emulator, SETTLE_WAIT};
use super::package_associations::{create_package, member, PACKAGE_TYPES, UNKNOWN_PACKAGE_ID};
use super::package_dissociation::dissociate_many;
use super::package_listing_page_bounds::{
    associate_many, details, list_for_domain, page_query, PUBLISHED_MAX_RESULTS,
};

/// The domain every case pages. The name satisfies `DomainName`'s
/// `@length(min: 3, max: 28)` with `@pattern("^[a-z][a-z0-9\-]+$")`.
const DOMAIN: &str = "cursor-domain";

/// How many packages the fixture puts on [`DOMAIN`]: more than any bounded page size
/// the cases ask for, so every row of the cursor table pages at least twice, and well
/// inside the account's package quota.
const ASSOCIATIONS: usize = 6;

/// The package ids a walk of [`DOMAIN`]'s listing reports, from the first page to the
/// last, over pages of `page_size` (`0` to send no page size at all).
async fn walk(emulator: &Emulator, page_size: i32, case: &str) -> Vec<String> {
    let first = list_for_domain(emulator, DOMAIN, &page_query(page_size, None)).await;
    let mut walked: Vec<String> = details(&first, case)
        .iter()
        .map(|one| member(one, "PackageID", case))
        .collect();
    if let Some(token) = first.body["NextToken"].as_str() {
        walked.extend(walk_from(emulator, token, page_size, case).await);
    }
    walked
}

/// The package ids a walk reports starting AT `token`, over pages of `page_size`.
async fn walk_from(emulator: &Emulator, token: &str, page_size: i32, case: &str) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut cursor = Some(token.to_owned());
    while let Some(token) = cursor {
        let answer = list_for_domain(emulator, DOMAIN, &page_query(page_size, Some(&token))).await;
        for one in details(&answer, case) {
            walked.push(member(&one, "PackageID", case));
        }
        cursor = answer.body["NextToken"].as_str().map(str::to_owned);
        assert!(
            walked.len() <= ASSOCIATIONS,
            "case {case}: paging kept issuing tokens past the end of the set, got {walked:?}"
        );
    }
    walked
}

/// One emulator holding [`ASSOCIATIONS`] available packages, spanning every
/// `PackageType` value, all of them put on [`DOMAIN`] in one call and settled — so the
/// domain's set is larger than every bounded page size the cases ask for.
struct Fixture {
    emulator: Emulator,
    /// The created package ids, in creation order.
    ids: Vec<String>,
}

async fn fixture() -> Fixture {
    let emulator = Emulator::new();
    let mut ids = Vec::with_capacity(ASSOCIATIONS);
    for index in 0..ASSOCIATIONS {
        // Cycling the type catalog spans every `PackageType` the model declares, which
        // `super::package_associations::every_package_type_is_covered` pins.
        let package_type = PACKAGE_TYPES[index % PACKAGE_TYPES.len()];
        ids.push(create_package(&emulator, &format!("cursor-{index}"), package_type).await);
    }
    // A package can be put on a domain only once its content is in place.
    tokio::time::sleep(SETTLE_WAIT).await;

    create_baseline(&emulator, DOMAIN).await;
    // A domain refuses a second associate while one is in flight, so the whole set is
    // linked in ONE call.
    let answer = associate_many(&emulator, DOMAIN, &ids).await;
    assert_eq!(
        answer.status, 200,
        "case AssociatePackages({DOMAIN:?}, every package): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    // Past the span the clock owes each association, so the records the cases read are
    // the ones it settled on.
    tokio::time::sleep(SETTLE_WAIT).await;
    Fixture { emulator, ids }
}

/// The whole set in listing order: the domain holds at most one association per
/// package and keys them by the package's id, so the order is the ids' own.
fn expected(ids: &[String]) -> Vec<String> {
    let mut sorted = ids.to_vec();
    sorted.sort();
    sorted
}

#[tokio::test]
async fn a_token_names_a_place_in_the_domains_set_whatever_page_size_resumes_it() {
    let Fixture { emulator, ids } = fixture().await;

    let whole = walk(&emulator, 0, "the unbounded walk").await;
    assert_eq!(
        whole,
        expected(&ids),
        "case the unbounded walk: expected every association the fixture made, got {whole:?}"
    );

    // One row per (page size the token was issued at, page size it is resumed at): the
    // smallest both ways, each crossed with a larger one, a resume that sends no page
    // size at all, a resume at the published maximum, and a token issued one short of
    // the whole set.
    for (issued_with, resumed_with) in [
        (1_i32, 1_i32),
        (1, 4),
        (3, 1),
        (2, 0),
        (4, PUBLISHED_MAX_RESULTS),
        (2, 3),
        (5, 2),
    ] {
        let case = format!("token issued at page size {issued_with}, resumed at {resumed_with}");
        let asked_for = usize::try_from(issued_with).unwrap_or_default();
        assert!(
            whole.len() > asked_for,
            "case {case}: this row needs a set longer than its page size, got {whole:?}"
        );

        let first = list_for_domain(&emulator, DOMAIN, &page_query(issued_with, None)).await;
        let head: Vec<String> = details(&first, &case)
            .iter()
            .map(|one| member(one, "PackageID", &case))
            .collect();
        assert_eq!(
            head.len(),
            asked_for,
            "case {case}: a page with more to come is as large as the request asked for, got \
             {head:?}"
        );
        let token = first.body["NextToken"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected a token while {} of {} associations are unreported, got \
                     {:?}",
                    whole.len() - head.len(),
                    whole.len(),
                    first.body
                )
            });

        // The token names a PLACE in the set, so the page size resuming it changes how
        // the remainder is cut into pages and not which associations it reports.
        let tail = walk_from(&emulator, &token, resumed_with, &case).await;
        let rejoined: Vec<String> = head.iter().chain(tail.iter()).cloned().collect();
        assert_eq!(
            rejoined, whole,
            "case {case}: expected the resumed walk to be the set's remainder, got {tail:?} after \
             {head:?}"
        );

        // Resuming from one token is a read: the second walk reports the same records.
        let again = walk_from(&emulator, &token, resumed_with, &case).await;
        assert_eq!(
            again, tail,
            "case {case}: expected one token to answer the same associations twice, got {again:?}"
        );
    }
}

#[tokio::test]
async fn paging_a_domains_packages_changes_nothing_a_later_caller_sees() {
    let Fixture { emulator, ids } = fixture().await;

    let before = details(
        &list_for_domain(&emulator, DOMAIN, "").await,
        "the first read",
    );
    assert_eq!(
        before.len(),
        ids.len(),
        "case the first read: expected every association the fixture made, got {before:?}"
    );

    // Served, paged and refused reads, all of which only report. Each row carries the
    // answer it must get — `None` where the read is served and reports a page, else the
    // status and error code the model declares for it — so a refusal that regressed
    // into a plausible page fails here rather than passing unread.
    let reads: Vec<(String, Option<(u16, &str)>)> = vec![
        (String::new(), None),
        ("maxResults=1".to_owned(), None),
        ("maxResults=3".to_owned(), None),
        (format!("maxResults={PUBLISHED_MAX_RESULTS}"), None),
        (
            "maxResults=-1".to_owned(),
            Some((400, "ValidationException")),
        ),
        (
            format!("maxResults={}", PUBLISHED_MAX_RESULTS + 1),
            Some((400, "ValidationException")),
        ),
        (
            format!("nextToken={UNKNOWN_PACKAGE_ID}"),
            Some((400, "ValidationException")),
        ),
    ];

    for (query, refusal) in &reads {
        let case = format!("ListPackagesForDomain({query:?})");
        let answer = list_for_domain(&emulator, DOMAIN, query).await;
        match refusal {
            None => assert!(
                !details(&answer, &case).is_empty(),
                "case {case}: expected the read to report a page, got {:?}",
                answer.body
            ),
            Some((status, code)) => assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (*status, *code),
                "case {case}: expected the modelled refusal, got {} with {:?}",
                answer.status,
                answer.body
            ),
        }
    }

    // And a listing of a domain nothing created, which has no set to report at all.
    let refused = list_for_domain(&emulator, "no-such-domain-4", "").await;
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (409, "ResourceNotFoundException"),
        "case ListPackagesForDomain(a domain nothing created): expected the modelled \
         missing-resource refusal, got {} with {:?}",
        refused.status,
        refused.body
    );

    for page_size in [0_i32, 1, 2, 4, PUBLISHED_MAX_RESULTS] {
        let case = format!("walked at page size {page_size}");
        let walked = walk(&emulator, page_size, &case).await;
        assert_eq!(
            walked,
            expected(&ids),
            "case {case}: expected every association exactly once, got {walked:?}"
        );
    }

    let after = details(
        &list_for_domain(&emulator, DOMAIN, "").await,
        "the read after every other read",
    );
    assert_eq!(
        after, before,
        "case the read after every other read: expected the records the first read reported, got \
         {after:?}"
    );
}

#[tokio::test]
async fn the_pages_report_the_links_the_domain_is_left_with() {
    let Fixture { emulator, ids } = fixture().await;

    // Two of the six taken off in one call — the first created and the last, so the
    // removals are not adjacent in the set's own order.
    let removed = vec![ids[0].clone(), ids[ASSOCIATIONS - 1].clone()];
    let answer = dissociate_many(
        &emulator,
        json!({ "DomainName": DOMAIN, "PackageList": removed }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case DissociatePackages({removed:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    // The clock owes the removal; past its span the links are off the domain.
    tokio::time::sleep(SETTLE_WAIT).await;

    let left: Vec<String> = expected(&ids)
        .into_iter()
        .filter(|id| !removed.contains(id))
        .collect();

    for page_size in [0_i32, 1, 2, 3, PUBLISHED_MAX_RESULTS] {
        let case = format!("after the dissociation, walked at page size {page_size}");
        let walked = walk(&emulator, page_size, &case).await;
        assert_eq!(
            walked, left,
            "case {case}: expected exactly the links the domain is left with, got {walked:?}"
        );
        for gone in &removed {
            assert!(
                !walked.contains(gone),
                "case {case}: expected no page to report the unlinked package {gone}, got \
                 {walked:?}"
            );
        }
    }

    // A token naming an association the domain no longer holds names no place in the
    // set, so it is a malformed request rather than an empty page.
    let refused = list_for_domain(
        &emulator,
        DOMAIN,
        &page_query(0, Some(removed[0].as_str())),
    )
    .await;
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case ListPackagesForDomain(a token naming an unlinked package): expected the request to \
         be refused, got {} with {:?}",
        refused.status,
        refused.body
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the page-size rule the implemented package listings share —
//! `ListDomainsForPackage`, `ListPackagesForDomain` and `DescribePackages` — and for
//! the cursor rule of `ListDomainsForPackage`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the package helpers from
//! [`super::package_associations`]; this suite adds no second copy of either. Its own
//! request helpers take the query string verbatim, because the cases here send page
//! sizes the typed helpers in [`super::package_association_listings`] cannot express
//! (a negative one, one past the published maximum).
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a page size outside the range the model publishes for `MaxResults` is a malformed
//!   request — 400 `ValidationException` naming the bound and the value — for every
//!   listing that takes one, over a table spanning one past the maximum, far past it,
//!   and negative values the shape's own range (a maximum and no minimum) does not
//!   catch; the published maximum itself, and the default a request sending none gets,
//!   are served;
//! * a request that asks for no particular page size reports the whole set in one page
//!   with no token, whether it sends nothing, the modelled default, or the published
//!   maximum;
//! * a token names a PLACE in the set, so the page size used to resume it changes how
//!   the remainder is cut into pages and not which records it reports, over a table
//!   crossing the page size a token was issued at with the one it is resumed at;
//! * listing is a report: served, paged and refused reads leave every record a later
//!   caller sees exactly as it was.

use serde_json::{json, Value};

use super::domain_lifecycle::{create_baseline, error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};
use super::package_associations::{available_packages, member, UNKNOWN_PACKAGE_ID};

/// The largest page size every listing under test publishes: `MaxResults` is
/// `@range(max: 100)` with `@default(0)` and no minimum
/// (`smithy_query(query="shape",shape="MaxResults")`).
pub(crate) const PUBLISHED_MAX_RESULTS: i32 = 100;

/// The domains the fixture creates. Each name satisfies `DomainName`'s
/// `@length(min: 3, max: 28)` with `@pattern("^[a-z][a-z0-9\-]+$")`, and there are more
/// of them than any page size the cases ask for, so every row pages.
const DOMAINS: [&str; 5] = [
    "bounds-a",
    "bounds-b",
    "bounds-c",
    "bounds-d",
    "bounds-e",
];

/// `AssociatePackage`: `POST /2021-01-01/packages/associate/{PackageID}/{DomainName}`.
/// `pub(crate)` so a sibling suite drives the single-package associate through this one
/// request helper rather than growing a second copy of it.
pub(crate) async fn associate_one(emulator: &Emulator, domain: &str, package_id: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/associate/{package_id}/{domain}"),
            Some(json!({})),
        )
        .await
}

/// `AssociatePackages`: `POST /2021-01-01/packages/associateMultiple`, which links a
/// whole batch in ONE call — a domain refuses a second associate while one is in
/// flight, so a domain taking several packages takes them together.
pub(crate) async fn associate_many(
    emulator: &Emulator,
    domain: &str,
    package_ids: &[String],
) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/associateMultiple"),
            Some(json!({
                "DomainName": domain,
                "PackageList": package_ids
                    .iter()
                    .map(|id| json!({ "PackageID": id }))
                    .collect::<Vec<Value>>(),
            })),
        )
        .await
}

/// `ListDomainsForPackage`: `GET /2021-01-01/packages/{PackageID}/domains`, carrying
/// `query` verbatim.
pub(crate) async fn list_for_package(
    emulator: &Emulator,
    package_id: &str,
    query: &str,
) -> Answer {
    emulator
        .call(
            "GET",
            &format!(
                "{PREFIX}/packages/{package_id}/domains{}",
                suffix(query)
            ),
            None,
        )
        .await
}

/// `ListPackagesForDomain`: `GET /2021-01-01/domain/{DomainName}/packages`, carrying
/// `query` verbatim.
pub(crate) async fn list_for_domain(emulator: &Emulator, domain: &str, query: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/domain/{domain}/packages{}", suffix(query)),
            None,
        )
        .await
}

/// `DescribePackages`: `POST /2021-01-01/packages/describe`. Its `MaxResults` carries
/// no `@httpQuery`, so it travels in the body rather than the query string.
async fn describe_packages(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call("POST", &format!("{PREFIX}/packages/describe"), Some(body))
        .await
}

/// `query` as a URI suffix: nothing at all for the request that sends no parameter.
fn suffix(query: &str) -> String {
    if query.is_empty() {
        return String::new();
    }
    format!("?{query}")
}

/// The query string for a paged listing: `page_size` of `0` sends no page size at all,
/// which is the request that asks for no particular one.
pub(crate) fn page_query(page_size: i32, token: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if page_size != 0 {
        parts.push(format!("maxResults={page_size}"));
    }
    if let Some(token) = token {
        parts.push(format!("nextToken={token}"));
    }
    parts.join("&")
}

/// The `DomainPackageDetailsList` of an answered listing, with the case named on
/// failure.
pub(crate) fn details(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainPackageDetailsList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a DomainPackageDetailsList, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `PackageDetailsList` of an answered `DescribePackages`, with the case named on
/// failure.
fn package_details(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["PackageDetailsList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a PackageDetailsList, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// One emulator with every package type created and available, [`DOMAINS`] created,
/// the first package put on EVERY domain and every package put on the first domain —
/// so both listings and the package registry hold a set larger than the page sizes the
/// cases ask for.
struct Fixture {
    emulator: Emulator,
    ids: Vec<String>,
}

async fn fixture() -> Fixture {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;
    for domain in DOMAINS {
        create_baseline(&emulator, domain).await;
    }

    // The first domain takes every package in one batch, and each of the others takes
    // the first package on its own.
    let answer = associate_many(&emulator, DOMAINS[0], &ids).await;
    assert_eq!(
        answer.status, 200,
        "case AssociatePackages({:?}, every package): expected 200, got {} with {:?}",
        DOMAINS[0], answer.status, answer.body
    );
    for domain in DOMAINS.iter().skip(1) {
        let answer = associate_one(&emulator, domain, &ids[0]).await;
        assert_eq!(
            answer.status, 200,
            "case AssociatePackage({domain:?}, the first package): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
    }
    Fixture { emulator, ids }
}

/// The whole set, in listing order: the domains the first package is on, and the
/// packages the first domain holds.
fn expected_domains() -> Vec<String> {
    let mut names: Vec<String> = DOMAINS.iter().map(|name| (*name).to_owned()).collect();
    names.sort();
    names
}

/// The domain names a walk of `package_id`'s listing reports, from the first page to
/// the last, over pages of `page_size` (`0` to send no page size at all).
async fn walk(emulator: &Emulator, package_id: &str, page_size: i32, case: &str) -> Vec<String> {
    let first = list_for_package(emulator, package_id, &page_query(page_size, None)).await;
    let mut walked: Vec<String> = details(&first, case)
        .iter()
        .map(|one| member(one, "DomainName", case))
        .collect();
    if let Some(token) = first.body["NextToken"].as_str() {
        walked.extend(walk_from(emulator, package_id, token, page_size, case).await);
    }
    walked
}

/// The domain names a walk reports starting AT `token`, over pages of `page_size`.
async fn walk_from(
    emulator: &Emulator,
    package_id: &str,
    token: &str,
    page_size: i32,
    case: &str,
) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut cursor = Some(token.to_owned());
    while let Some(token) = cursor {
        let answer =
            list_for_package(emulator, package_id, &page_query(page_size, Some(&token))).await;
        for one in details(&answer, case) {
            walked.push(member(&one, "DomainName", case));
        }
        cursor = answer.body["NextToken"].as_str().map(str::to_owned);
        assert!(
            walked.len() <= DOMAINS.len(),
            "case {case}: paging kept issuing tokens past the end of the set, got {walked:?}"
        );
    }
    walked
}

#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused_by_every_package_listing() {
    let Fixture { emulator, ids } = fixture().await;

    // Rows differ along the only thing this rule generalises over: the value sent. One
    // past the published maximum, far past it, and the negative values the shape's own
    // range cannot catch — it publishes a maximum and no minimum — at the first such
    // value and at the member type's floor.
    let refused: [i32; 5] = [PUBLISHED_MAX_RESULTS + 1, 1_000, i32::MAX, -1, i32::MIN];

    for value in refused {
        for (case, answer) in [
            (
                format!("ListDomainsForPackage(maxResults={value})"),
                list_for_package(&emulator, &ids[0], &format!("maxResults={value}")).await,
            ),
            (
                format!("ListPackagesForDomain(maxResults={value})"),
                list_for_domain(&emulator, DOMAINS[0], &format!("maxResults={value}")).await,
            ),
            (
                format!("DescribePackages(MaxResults={value})"),
                describe_packages(&emulator, json!({ "MaxResults": value })).await,
            ),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected the modelled validation refusal, got {:?}",
                answer.body
            );
        }
    }

    // A value BELOW the range is not a page size at all, so the refusal is the
    // listing's own and names the bound it broke and the value it carried.
    for value in [-1, i32::MIN] {
        for (case, answer) in [
            (
                format!("ListDomainsForPackage(maxResults={value})"),
                list_for_package(&emulator, &ids[0], &format!("maxResults={value}")).await,
            ),
            (
                format!("ListPackagesForDomain(maxResults={value})"),
                list_for_domain(&emulator, DOMAINS[0], &format!("maxResults={value}")).await,
            ),
            (
                format!("DescribePackages(MaxResults={value})"),
                describe_packages(&emulator, json!({ "MaxResults": value })).await,
            ),
        ] {
            assert_eq!(
                answer.body["message"].as_str(),
                Some(
                    format!("MaxResults must be between 0 and {PUBLISHED_MAX_RESULTS}: {value}")
                        .as_str()
                ),
                "case {case}: expected the refusal to name the bound and the value, got {:?}",
                answer.body
            );
        }
    }

    // The bound itself, and every page size inside the range, are served: the refusals
    // above are the value each request carried rather than the call.
    for value in [1, 2, PUBLISHED_MAX_RESULTS] {
        let case = format!("ListDomainsForPackage(maxResults={value})");
        let answer = list_for_package(&emulator, &ids[0], &format!("maxResults={value}")).await;
        assert!(
            !details(&answer, &case).is_empty(),
            "case {case}: the published bound is accepted and reports a page"
        );
        let case = format!("ListPackagesForDomain(maxResults={value})");
        let answer = list_for_domain(&emulator, DOMAINS[0], &format!("maxResults={value}")).await;
        assert!(
            !details(&answer, &case).is_empty(),
            "case {case}: the published bound is accepted and reports a page"
        );
        let case = format!("DescribePackages(MaxResults={value})");
        let answer = describe_packages(&emulator, json!({ "MaxResults": value })).await;
        assert!(
            !package_details(&answer, &case).is_empty(),
            "case {case}: the published bound is accepted and reports a page"
        );
    }
}

#[tokio::test]
async fn a_request_that_asks_for_no_page_size_reports_the_whole_set() {
    let Fixture { emulator, ids } = fixture().await;

    let domains = expected_domains();
    let mut packages = ids.clone();
    packages.sort();

    // The three ways a request asks for no particular page size: sending no parameter
    // at all, sending the modelled default, and sending the published maximum — which
    // is larger than either set here.
    let unbounded = [
        String::new(),
        "maxResults=0".to_owned(),
        format!("maxResults={PUBLISHED_MAX_RESULTS}"),
    ];

    for query in &unbounded {
        let case = format!("ListDomainsForPackage({query:?})");
        let answer = list_for_package(&emulator, &ids[0], query).await;
        let reported: Vec<String> = details(&answer, &case)
            .iter()
            .map(|one| member(one, "DomainName", &case))
            .collect();
        assert_eq!(
            reported, domains,
            "case {case}: expected the whole set in one page, got {reported:?}"
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case}: expected no token once the page ended the set, got {:?}",
            answer.body["NextToken"]
        );

        let case = format!("ListPackagesForDomain({query:?})");
        let answer = list_for_domain(&emulator, DOMAINS[0], query).await;
        let reported: Vec<String> = details(&answer, &case)
            .iter()
            .map(|one| member(one, "PackageID", &case))
            .collect();
        assert_eq!(
            reported, packages,
            "case {case}: expected the whole set in one page, got {reported:?}"
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case}: expected no token once the page ended the set, got {:?}",
            answer.body["NextToken"]
        );
    }
}

#[tokio::test]
async fn a_token_resumes_where_the_previous_page_stopped_whatever_resumes_it() {
    let Fixture { emulator, ids } = fixture().await;

    let whole = walk(&emulator, &ids[0], 0, "the unbounded walk").await;
    assert_eq!(
        whole,
        expected_domains(),
        "case the unbounded walk: expected every domain the package is on, got {whole:?}"
    );

    // One row per (page size the token was issued at, page size it is resumed at): the
    // smallest both ways, each crossed with a larger one, a resume that sends no page
    // size at all, and a resume at the published maximum.
    for (issued_with, resumed_with) in [(1_i32, 1_i32), (1, 3), (3, 1), (2, 0), (4, 100), (2, 3)] {
        let case = format!("token issued at page size {issued_with}, resumed at {resumed_with}");
        let asked_for = usize::try_from(issued_with).unwrap_or_default();
        assert!(
            whole.len() > asked_for,
            "case {case}: this row needs a set longer than its page size, got {whole:?}"
        );

        let first = list_for_package(&emulator, &ids[0], &page_query(issued_with, None)).await;
        let head: Vec<String> = details(&first, &case)
            .iter()
            .map(|one| member(one, "DomainName", &case))
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
                    "case {case}: expected a token while {} of {} domains are unreported, got {:?}",
                    whole.len() - head.len(),
                    whole.len(),
                    first.body
                )
            });

        // The token names a PLACE in the set, so the page size resuming it changes how
        // the remainder is cut into pages and not which domains it reports.
        let tail = walk_from(&emulator, &ids[0], &token, resumed_with, &case).await;
        let rejoined: Vec<String> = head.iter().chain(tail.iter()).cloned().collect();
        assert_eq!(
            rejoined, whole,
            "case {case}: expected the resumed walk to be the set's remainder, got {tail:?} after \
             {head:?}"
        );

        // Resuming from one token is a read: the second walk reports the same domains.
        let again = walk_from(&emulator, &ids[0], &token, resumed_with, &case).await;
        assert_eq!(
            again, tail,
            "case {case}: expected one token to answer the same domains twice, got {again:?}"
        );
    }
}

#[tokio::test]
async fn listing_a_packages_domains_changes_nothing_a_later_caller_sees() {
    let Fixture { emulator, ids } = fixture().await;

    // Past the span the clock owes each association, so the records below are the ones
    // it settled on and any later difference is a read's doing.
    tokio::time::sleep(SETTLE_WAIT).await;
    let before = details(
        &list_for_package(&emulator, &ids[0], "").await,
        "the first read",
    );
    assert_eq!(
        before.len(),
        DOMAINS.len(),
        "case the first read: expected every association the fixture made, got {before:?}"
    );

    // Served, paged and refused reads, all of which only report: a page bounded at
    // every size inside the range, a page size outside it at either end, a token
    // naming no place in the set, and a package id nothing minted.
    for query in [
        "",
        "maxResults=1",
        "maxResults=2",
        &format!("maxResults={PUBLISHED_MAX_RESULTS}"),
        "maxResults=-1",
        "maxResults=101",
        "nextToken=no-such-domain-9",
    ] {
        list_for_package(&emulator, &ids[0], query).await;
    }
    list_for_package(&emulator, UNKNOWN_PACKAGE_ID, "").await;

    for page_size in [0_i32, 1, 2, 3] {
        let case = format!("walked at page size {page_size}");
        let walked = walk(&emulator, &ids[0], page_size, &case).await;
        assert_eq!(
            walked,
            expected_domains(),
            "case {case}: expected every association exactly once, got {walked:?}"
        );
    }

    let after = details(
        &list_for_package(&emulator, &ids[0], "").await,
        "the read after every other read",
    );
    assert_eq!(
        after, before,
        "case the read after every other read: expected the records the first read reported, got \
         {after:?}"
    );
}

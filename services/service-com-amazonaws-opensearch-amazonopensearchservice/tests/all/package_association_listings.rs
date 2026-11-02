// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two read paths of a package association:
//! `ListPackagesForDomain` and `ListDomainsForPackage`, and for how a request that
//! names a domain nothing created is refused.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the package helpers from
//! [`super::package_associations`]; this suite adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * both listings report the SAME record the call that made the association answered
//!   with, each from the one stored association, over a table spanning every
//!   `PackageType` value, a domain with one association and one with several, and a
//!   package on one domain and on several;
//! * a `DomainName` that names no domain is a CLIENT-INPUT refusal — 400
//!   `ValidationException` stating that the domain does not exist — for every
//!   operation that links a package, over a table of syntactically valid names nothing
//!   created, and the request links nothing: the package it named still has no domain;
//! * a batch that names one package twice is refused as a whole and links nothing,
//!   while the same package named once is served and reports exactly one association;
//! * a listing of a domain's packages is a report over that domain's set, so a name
//!   nothing created and a domain that exists with nothing linked both report the
//!   empty set, and the sibling listing of ONE package's domains reports the same empty
//!   set for an id nothing minted;
//! * paging walks the whole set exactly once, with every page but the last as large as
//!   the request asked for.

use serde_json::{json, Value};

use super::domain_lifecycle::{create_baseline, error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};
use super::package_associations::{
    available_packages, member, PACKAGE_TYPES, UNKNOWN_PACKAGE_ID,
};

/// `AssociatePackage`: `POST /2021-01-01/packages/associate/{PackageID}/{DomainName}`.
async fn associate_one(emulator: &Emulator, domain: &str, package_id: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/associate/{package_id}/{domain}"),
            Some(json!({})),
        )
        .await
}

/// `AssociatePackages`: `POST /2021-01-01/packages/associateMultiple`.
async fn associate_many(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/associateMultiple"),
            Some(body),
        )
        .await
}

/// `ListPackagesForDomain`: `GET /2021-01-01/domain/{DomainName}/packages`, optionally
/// bounded by a page size and resumed from a token.
async fn list_for_domain(
    emulator: &Emulator,
    domain: &str,
    max_results: Option<usize>,
    token: Option<&str>,
) -> Answer {
    emulator
        .call(
            "GET",
            &format!(
                "{PREFIX}/domain/{domain}/packages{}",
                query(max_results, token)
            ),
            None,
        )
        .await
}

/// `ListDomainsForPackage`: `GET /2021-01-01/packages/{PackageID}/domains`, optionally
/// bounded by a page size and resumed from a token.
async fn list_for_package(
    emulator: &Emulator,
    package_id: &str,
    max_results: Option<usize>,
    token: Option<&str>,
) -> Answer {
    emulator
        .call(
            "GET",
            &format!(
                "{PREFIX}/packages/{package_id}/domains{}",
                query(max_results, token)
            ),
            None,
        )
        .await
}

/// The query string for a paged listing: the members the model binds to `maxResults`
/// and `nextToken`, and nothing when a case names neither.
fn query(max_results: Option<usize>, token: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(max) = max_results {
        parts.push(format!("maxResults={max}"));
    }
    if let Some(token) = token {
        parts.push(format!("nextToken={token}"));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("?{}", parts.join("&"))
}

/// The `DomainPackageDetailsList` of an answered listing, with the case named on
/// failure.
fn listed(answer: &Answer, case: &str) -> Vec<Value> {
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

#[tokio::test]
async fn both_listings_report_the_record_the_association_was_made_with() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    // One domain per row, so one row's writes are not another's precondition. The rows
    // differ along what the rule generalises over: how many packages a domain holds
    // (one, two, every type) and which types they are.
    let rows: Vec<(&str, &str, Vec<usize>)> = vec![
        ("one dictionary package", "listing-one", vec![0]),
        ("two packages of different types", "listing-two", vec![1, 2]),
        (
            "every package type at once",
            "listing-every",
            (0..PACKAGE_TYPES.len()).collect(),
        ),
    ];

    for (case, domain, packages) in rows {
        create_baseline(&emulator, domain).await;
        let answer = associate_many(
            &emulator,
            json!({
                "DomainName": domain,
                "PackageList": packages
                    .iter()
                    .map(|index| json!({ "PackageID": ids[*index] }))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 from AssociatePackages, got {} with {:?}",
            answer.status, answer.body
        );
        let made = answer.body["DomainPackageDetailsList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected the associations made, got {:?}", answer.body))
            .clone();

        // The domain's listing reports exactly the records the call answered with, in
        // the order the store holds them.
        let reported = listed(&list_for_domain(&emulator, domain, None, None).await, case);
        let mut expected = made.clone();
        expected.sort_by_key(|details| member(details, "PackageID", case));
        assert_eq!(
            reported, expected,
            "case {case}: the domain's listing reports the records the association made, got \
             {reported:?}"
        );

        // And so does each package's own listing, from the same stored association.
        for index in &packages {
            let package_id = &ids[*index];
            let case = format!("{case} / package {index}");
            let reported = listed(
                &list_for_package(&emulator, package_id, None, None).await,
                &case,
            );
            let one = made
                .iter()
                .find(|details| details["PackageID"] == json!(package_id))
                .unwrap_or_else(|| panic!("case {case}: the call answered with this package"));
            assert!(
                reported.contains(one),
                "case {case}: the package's listing reports the record the association made, \
                 got {reported:?}"
            );
            assert_eq!(
                reported
                    .iter()
                    .filter(|details| details["DomainName"] == json!(domain))
                    .count(),
                1,
                "case {case}: a domain holds ONE association per package, got {reported:?}"
            );
        }
    }

    // The status the clock owes an association is reported by both listings alike,
    // because both read the one stored record.
    tokio::time::sleep(SETTLE_WAIT).await;
    let settled = listed(
        &list_for_domain(&emulator, "listing-one", None, None).await,
        "listing-one settled",
    );
    assert_eq!(
        settled[0]["DomainPackageStatus"],
        json!("ACTIVE"),
        "case listing-one settled: the listing reports the status the clock left the \
         association in, got {settled:?}"
    );
    let from_package = listed(
        &list_for_package(&emulator, &ids[0], None, None).await,
        "listing-one settled, from the package",
    );
    assert!(
        from_package.contains(&settled[0]),
        "case listing-one settled: both listings report the one record, got {from_package:?}"
    );
}

#[tokio::test]
async fn a_domain_nothing_created_is_refused_as_invalid_input_and_links_nothing() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    // Every row is a `DomainName` the model's own pattern admits
    // (`^[a-z][a-z0-9\-]+$`, `@length(min: 3, max: 28)`) that nothing created: the
    // shortest the pattern allows, the longest, one carrying digits and hyphens, and
    // the name of a domain that was created and then deleted.
    let deleted = "assoc-gone-domain";
    create_baseline(&emulator, deleted).await;
    let released = emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/domain/{deleted}"),
            None,
        )
        .await;
    assert_eq!(
        released.status, 200,
        "case DeleteDomain({deleted:?}): expected 200, got {} with {:?}",
        released.status, released.body
    );

    let names = [
        "abc",
        "a23456789012345678901234567",
        "no-such-domain-9",
        deleted,
    ];

    for name in names {
        for (case, answer) in [
            (
                format!("AssociatePackage({name:?})"),
                associate_one(&emulator, name, &ids[0]).await,
            ),
            (
                format!("AssociatePackages({name:?})"),
                associate_many(
                    &emulator,
                    json!({
                        "DomainName": name,
                        "PackageList": [{ "PackageID": ids[0] }],
                    }),
                )
                .await,
            ),
            (
                format!("AssociatePackages({name:?}, naming no package)"),
                associate_many(
                    &emulator,
                    json!({ "DomainName": name, "PackageList": [] }),
                )
                .await,
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
                "case {case}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(format!("The domain {name} does not exist.").as_str()),
                "case {case}: expected the refusal to state that the domain does not exist, got \
                 {:?}",
                answer.body
            );
        }

        // The refused request linked nothing: the package it named still has no domain.
        let reported = listed(
            &list_for_package(&emulator, &ids[0], None, None).await,
            &format!("ListDomainsForPackage(after {name:?})"),
        );
        assert_eq!(
            reported,
            Vec::<Value>::new(),
            "case ListDomainsForPackage(after {name:?}): a refused request links nothing, got \
             {reported:?}"
        );
    }

    // The same call for a domain that EXISTS is served, so the refusals above are the
    // name each request carried rather than the call.
    let domain = "assoc-real-domain";
    create_baseline(&emulator, domain).await;
    let accepted = associate_one(&emulator, domain, &ids[0]).await;
    assert_eq!(
        accepted.status, 200,
        "case AssociatePackage({domain:?}): expected 200, got {} with {:?}",
        accepted.status, accepted.body
    );
    let reported = listed(
        &list_for_package(&emulator, &ids[0], None, None).await,
        "ListDomainsForPackage(after the accepted call)",
    );
    assert_eq!(
        reported.len(),
        1,
        "case ListDomainsForPackage(after the accepted call): expected the one association made, \
         got {reported:?}"
    );
}

#[tokio::test]
async fn a_batch_that_names_one_package_twice_is_refused_as_a_whole() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    // Rows differ in WHERE the repeat sits in the batch and how long the batch is: the
    // same package twice on its own, twice with another between them, and a repeat at
    // the end of a longer batch.
    let rows: Vec<(&str, &str, Vec<usize>)> = vec![
        ("the same package twice", "batch-twice", vec![0, 0]),
        (
            "a repeat with another package between",
            "batch-between",
            vec![1, 2, 1],
        ),
        (
            "a repeat at the end of a longer batch",
            "batch-tail",
            vec![0, 1, 2, 3, 3],
        ),
    ];

    for (case, domain, packages) in rows {
        create_baseline(&emulator, domain).await;
        let answer = associate_many(
            &emulator,
            json!({
                "DomainName": domain,
                "PackageList": packages
                    .iter()
                    .map(|index| json!({ "PackageID": ids[*index] }))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            answer.body
        );

        // The whole batch was refused: the domain holds no association at all.
        let reported = listed(
            &list_for_domain(&emulator, domain, None, None).await,
            case,
        );
        assert_eq!(
            reported,
            Vec::<Value>::new(),
            "case {case}: a refused batch links nothing, got {reported:?}"
        );

        // The same package named ONCE is served, and reports exactly one association.
        let once = associate_many(
            &emulator,
            json!({
                "DomainName": domain,
                "PackageList": [{ "PackageID": ids[packages[0]] }],
            }),
        )
        .await;
        assert_eq!(
            once.status, 200,
            "case {case} / named once: expected 200, got {} with {:?}",
            once.status, once.body
        );
        let reported = listed(
            &list_for_domain(&emulator, domain, None, None).await,
            &format!("{case} / named once"),
        );
        assert_eq!(
            reported.len(),
            1,
            "case {case} / named once: expected exactly one association, got {reported:?}"
        );
        assert_eq!(
            member(&reported[0], "PackageID", case),
            ids[packages[0]],
            "case {case} / named once: expected the package the call named, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_of_a_resource_nothing_created_is_the_empty_set() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    // A name the store holds no domain under holds no link either, whatever the name's
    // shape: the listing reports that empty set rather than refusing the name.
    for name in ["abc", "no-such-domain-7", "a23456789012345678901234567"] {
        let reported = listed(
            &list_for_domain(&emulator, name, None, None).await,
            &format!("ListPackagesForDomain({name:?})"),
        );
        assert_eq!(
            reported,
            Vec::<Value>::new(),
            "case ListPackagesForDomain({name:?}): expected the empty set, got {reported:?}"
        );
    }
    // An id the store holds no package under is associated with nothing either: the
    // listing reports that empty set rather than refusing the id.
    let answer = list_for_package(&emulator, UNKNOWN_PACKAGE_ID, None, None).await;
    let reported = listed(
        &answer,
        "ListDomainsForPackage(an id nothing minted)",
    );
    assert_eq!(
        reported,
        Vec::<Value>::new(),
        "case ListDomainsForPackage(an id nothing minted): expected the empty set, got \
         {reported:?}"
    );

    // A resource that EXISTS with nothing linked reports the empty set rather than a
    // refusal — which is what tells a caller the package is associated with nothing.
    create_baseline(&emulator, "listing-empty").await;
    assert_eq!(
        listed(
            &list_for_domain(&emulator, "listing-empty", None, None).await,
            "a domain with no package"
        ),
        Vec::<Value>::new(),
        "case a domain with no package: expected the empty set"
    );
    for (index, package_id) in ids.iter().enumerate() {
        assert_eq!(
            listed(
                &list_for_package(&emulator, package_id, None, None).await,
                &format!("package {index} on no domain")
            ),
            Vec::<Value>::new(),
            "case package {index} on no domain: expected the empty set"
        );
    }
}

#[tokio::test]
async fn paging_walks_the_whole_set_exactly_once() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    // One domain holding every package, and one package on three domains, so both
    // listings page over a set larger than the page size each request asks for.
    let domain = "paging-domain";
    create_baseline(&emulator, domain).await;
    let answer = associate_many(
        &emulator,
        json!({
            "DomainName": domain,
            "PackageList": ids
                .iter()
                .map(|id| json!({ "PackageID": id }))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case AssociatePackages(every package): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let domains = ["paging-a", "paging-b", "paging-c"];
    for name in domains {
        create_baseline(&emulator, name).await;
        let answer = associate_one(&emulator, name, &ids[0]).await;
        assert_eq!(
            answer.status, 200,
            "case AssociatePackage({name:?}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // (case, the whole set in listing order, how large a page each request asks for)
    let expected_packages: Vec<String> = {
        let mut sorted = ids.clone();
        sorted.sort();
        sorted
    };
    let mut expected_domains: Vec<String> = domains.iter().map(|name| (*name).to_owned()).collect();
    expected_domains.push(domain.to_owned());
    expected_domains.sort();

    for page_size in [1usize, 2, 3] {
        // The domain's packages, walked page by page.
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let case = format!("ListPackagesForDomain(maxResults={page_size})");
            let answer =
                list_for_domain(&emulator, domain, Some(page_size), token.as_deref()).await;
            let page = listed(&answer, &case);
            match answer.body["NextToken"].as_str() {
                Some(next) => {
                    assert_eq!(
                        page.len(),
                        page_size,
                        "case {case}: a page with more to come is as large as asked for, got \
                         {page:?}"
                    );
                    token = Some(next.to_owned());
                }
                None => {
                    assert!(
                        page.len() <= page_size,
                        "case {case}: the last page is no larger than asked for, got {page:?}"
                    );
                    for details in &page {
                        walked.push(member(details, "PackageID", &case));
                    }
                    break;
                }
            }
            for details in &page {
                walked.push(member(details, "PackageID", &case));
            }
        }
        assert_eq!(
            walked, expected_packages,
            "case ListPackagesForDomain(maxResults={page_size}): paging walks every association \
             exactly once"
        );

        // And the package's domains, the same way.
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let case = format!("ListDomainsForPackage(maxResults={page_size})");
            let answer =
                list_for_package(&emulator, &ids[0], Some(page_size), token.as_deref()).await;
            let page = listed(&answer, &case);
            let next = answer.body["NextToken"].as_str().map(str::to_owned);
            for details in &page {
                walked.push(member(details, "DomainName", &case));
            }
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, expected_domains,
            "case ListDomainsForPackage(maxResults={page_size}): paging walks every association \
             exactly once"
        );
    }

    // A token that names no place in the set is a malformed request rather than an
    // empty page.
    let refused = list_for_domain(&emulator, domain, None, Some(UNKNOWN_PACKAGE_ID)).await;
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case ListPackagesForDomain(a token naming nothing): expected the request to be refused, \
         got {} with {:?}",
        refused.status,
        refused.body
    );
    let refused = list_for_package(&emulator, &ids[0], None, Some("no-such-domain-7")).await;
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case ListDomainsForPackage(a token naming nothing): expected the request to be refused, \
         got {} with {:?}",
        refused.status,
        refused.body
    );
}

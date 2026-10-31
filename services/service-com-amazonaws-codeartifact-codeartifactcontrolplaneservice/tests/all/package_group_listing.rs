// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for `ListPackageGroups`.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code and body — not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! example:
//!
//! 1. The listing answers the package groups the addressed DOMAIN holds, in pattern order,
//!    the root group `/*` included — every group, at every depth, not just one group's
//!    children.
//! 2. `prefix` keeps "only package groups with patterns that match the prefix"
//!    (`smithy_query(query="doc", shape="ListPackageGroupsRequest", member="prefix")`),
//!    matched against each group's own pattern; an absent prefix keeps every group.
//! 3. Each entry carries every member of the group's own description —
//!    `PackageGroupSummary` declares the same members as `PackageGroupDescription`
//!    (`smithy_query(query="shape", shape="PackageGroupSummary")`) — so a listed entry
//!    equals what `DescribePackageGroup` answers for that pattern, member for member,
//!    including the DERIVED `parent`: inserting an intermediate group reparents the groups
//!    below it in this listing with no second call.
//! 4. The listing is `@paginated` over the patterns held: a page carries at most
//!    `maxResults` groups, a non-final page reports an opaque `nextToken` that resumes
//!    strictly after the last entry, the final page reports none, and the pages concatenate
//!    to the whole set exactly once. A token belongs to the listing that minted it.
//! 5. Reading changes nothing: the same request answers identically when repeated, and the
//!    listing leaves every record it reports where it was.
//! 6. Refusals: a domain the caller does not hold is `ResourceNotFoundException` (404) —
//!    not an empty page — a `nextToken` this listing did not mint is `ValidationException`
//!    (400), and a `maxResults` outside the modelled range (1..1000) is refused at the
//!    boundary.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_domain, emulator, package_group_arn, Answer, ACCOUNT};
use crate::package_group_hierarchy::{create_package_group, describe_package_group, ROOT};

/// `ListPackageGroups`: every input member is `httpQuery`-bound on
/// `POST /v1/package-groups`
/// (`smithy_query(query="operation", operation="ListPackageGroups")`).
async fn list_package_groups(
    router: &MultiServiceRouter,
    domain: &str,
    prefix: Option<&str>,
    max_results: Option<u32>,
    next_token: Option<&str>,
) -> Answer {
    let mut uri = format!("/v1/package-groups?domain={domain}");
    if let Some(prefix) = prefix {
        uri.push_str(&format!("&prefix={prefix}"));
    }
    if let Some(max_results) = max_results {
        uri.push_str(&format!("&max-results={max_results}"));
    }
    if let Some(next_token) = next_token {
        uri.push_str(&format!("&next-token={next_token}"));
    }
    call(router, "POST", &uri, None).await
}

/// The `packageGroups` of a successful listing.
fn listed<'a>(label: &str, answer: &'a Answer) -> &'a Vec<Value> {
    answer
        .body
        .get("packageGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {label:?}: no packageGroups in {answer:?}"))
}

/// The `pattern` of every entry of a successful listing, in the order reported.
fn listed_patterns(label: &str, answer: &Answer) -> Vec<String> {
    listed(label, answer)
        .iter()
        .map(|group| {
            group
                .get("pattern")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {label:?}: an entry with no pattern in {answer:?}"))
                .to_string()
        })
        .collect()
}

/// The `nextToken` of a listing, or `None` on the final page.
fn next_token(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Every pattern the domain holds under `prefix`, walked page by page at a page size of
/// `max_results`.
///
/// Returns the concatenated patterns and how many requests it took, so a caller can assert
/// both the contents and that the walk actually paginated.
async fn walk(
    router: &MultiServiceRouter,
    domain: &str,
    prefix: Option<&str>,
    max_results: Option<u32>,
) -> (Vec<String>, usize) {
    let label = format!("walk {prefix:?} at {max_results:?}");
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let answer = list_package_groups(router, domain, prefix, max_results, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: page {pages} expected 200, got {answer:?}"
        );
        let page = listed_patterns(&label, &answer);
        if let Some(limit) = max_results {
            assert!(
                page.len() <= limit as usize,
                "case {label:?}: page {pages} carries {} entries, past maxResults {limit}",
                page.len()
            );
        }
        collected.extend(page);
        pages += 1;
        match next_token(&answer) {
            Some(next) => token = Some(next),
            None => break,
        }
        assert!(
            pages < 100,
            "case {label:?}: the walk did not terminate after {pages} pages"
        );
    }
    (collected, pages)
}

/// Every pattern created for the tables below, in an order that is deliberately NOT
/// top-down: `/maven/com.example/*` is created before the group that ends up above it.
const HELD: &[&str] = &[
    "/npm/*",
    "/npm/@scope/*",
    "/npm/@scope/package$",
    "/npm/other/*",
    "/maven/com.example/*",
    "/maven/*",
    "/pypi/requests$",
];

/// Every pattern the domain holds once [`HELD`] exists, in the pattern (key) order the
/// listing reports: the root group the domain was created with, plus [`HELD`] sorted.
/// Written out by hand rather than computed, so the expectation does not restate the
/// implementation.
const ALL_PATTERNS: &[&str] = &[
    "/*",
    "/maven/*",
    "/maven/com.example/*",
    "/npm/*",
    "/npm/@scope/*",
    "/npm/@scope/package$",
    "/npm/other/*",
    "/pypi/requests$",
];

/// One row of the prefix table: a `prefix` and the patterns the listing must answer with.
struct PrefixCase {
    label: &'static str,
    /// `None` is "no prefix member sent at all".
    prefix: Option<&'static str>,
    /// In the pattern order the listing reports.
    expected: &'static [&'static str],
}

/// The table. Rows differ along what rule 2 generalises over: whether a prefix is sent at
/// all, its LENGTH (the modelled zero-length one, one character, a whole segment, a whole
/// pattern), where it ENDS (on a separator, mid-segment, past the last segment), how many
/// groups it keeps (all, several, one, none), and the terminator of the patterns it keeps
/// (`*` wildcard and `$` exact).
const PREFIX_CASES: &[PrefixCase] = &[
    PrefixCase {
        label: "no prefix: every group the domain holds",
        prefix: None,
        expected: ALL_PATTERNS,
    },
    PrefixCase {
        label: "the modelled zero-length prefix keeps every group",
        prefix: Some(""),
        expected: ALL_PATTERNS,
    },
    PrefixCase {
        label: "the root separator is a prefix of every pattern",
        prefix: Some("/"),
        expected: ALL_PATTERNS,
    },
    PrefixCase {
        label: "a whole format segment keeps that subtree",
        prefix: Some("/npm/"),
        expected: &[
            "/npm/*",
            "/npm/@scope/*",
            "/npm/@scope/package$",
            "/npm/other/*",
        ],
    },
    PrefixCase {
        label: "a format segment without its separator also keeps the format group itself",
        prefix: Some("/npm"),
        expected: &[
            "/npm/*",
            "/npm/@scope/*",
            "/npm/@scope/package$",
            "/npm/other/*",
        ],
    },
    PrefixCase {
        label: "a prefix ending mid-segment keeps only what it spells",
        prefix: Some("/npm/@sc"),
        expected: &["/npm/@scope/*", "/npm/@scope/package$"],
    },
    PrefixCase {
        label: "a dotted maven group id keeps its one group",
        prefix: Some("/maven/com.example/"),
        expected: &["/maven/com.example/*"],
    },
    PrefixCase {
        label: "a whole wildcard pattern keeps exactly that group",
        prefix: Some("/npm/other/*"),
        expected: &["/npm/other/*"],
    },
    PrefixCase {
        label: "a whole exact pattern keeps exactly that group",
        prefix: Some("/pypi/requests$"),
        expected: &["/pypi/requests$"],
    },
    PrefixCase {
        label: "a prefix past the last segment of every pattern keeps nothing",
        prefix: Some("/pypi/requests$/more"),
        expected: &[],
    },
    PrefixCase {
        label: "a format nobody holds keeps nothing",
        prefix: Some("/cargo/"),
        expected: &[],
    },
];

/// RULES 1, 2, 3 and 5 over the whole table.
#[tokio::test]
async fn package_groups_are_listed_for_the_domain_and_narrowed_by_prefix() {
    let router = emulator();
    let domain = "pg-list-prefix";
    let created_domain = create_domain(&router, domain, None).await;
    assert_eq!(
        created_domain.status, 200,
        "domain setup: expected 200, got {created_domain:?}"
    );
    for pattern in HELD {
        let created = create_package_group(&router, domain, pattern, None, None).await;
        assert_eq!(
            created.status, 200,
            "setup {pattern:?}: expected 200, got {created:?}"
        );
    }

    for case in PREFIX_CASES {
        let label = case.label;
        let answer = list_package_groups(&router, domain, case.prefix, None, None).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected 200, got {answer:?}"
        );

        // RULES 1 and 2.
        assert_eq!(
            listed_patterns(label, &answer),
            case.expected,
            "case {label:?}: expected the groups matching {:?}, got {answer:?}",
            case.prefix
        );
        assert!(
            next_token(&answer).is_none(),
            "case {label:?}: a single complete page reports no nextToken, got {answer:?}"
        );

        // RULE 3: the entry is the group's own record, derived members included.
        for entry in listed(label, &answer) {
            let pattern = entry
                .get("pattern")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {label:?}: an entry with no pattern in {entry:?}"));
            let described = describe_package_group(&router, domain, pattern).await;
            assert_eq!(
                described.status, 200,
                "case {label:?}/{pattern}: the listed group must be readable, got {described:?}"
            );
            assert_eq!(
                entry,
                described.group(),
                "case {label:?}/{pattern}: the listed summary must equal the group's description"
            );
            assert_eq!(
                entry.get("domainName").and_then(Value::as_str),
                Some(domain),
                "case {label:?}/{pattern}: expected domainName {domain}, got {entry:?}"
            );
            assert_eq!(
                entry.get("domainOwner").and_then(Value::as_str),
                Some(ACCOUNT),
                "case {label:?}/{pattern}: expected domainOwner {ACCOUNT}, got {entry:?}"
            );
            assert_eq!(
                entry.get("arn").and_then(Value::as_str),
                Some(package_group_arn(domain, pattern).as_str()),
                "case {label:?}/{pattern}: expected the package-group ARN, got {entry:?}"
            );
        }

        // RULE 5: the listing is a read, so repeating it answers identically.
        let again = list_package_groups(&router, domain, case.prefix, None, None).await;
        assert_eq!(
            again.body, answer.body,
            "case {label:?}: repeating the listing must answer identically"
        );
    }
}

/// RULE 3, isolated: a listed entry's `parent` is DERIVED from the patterns held, so
/// inserting a group between a parent and a leaf changes what this listing reports for that
/// leaf — with no rewrite of the leaf's own record.
#[tokio::test]
async fn an_inserted_group_reparents_the_entries_below_it() {
    let router = emulator();
    let domain = "pg-list-reparent";
    create_domain(&router, domain, None).await;

    /// The `parent.pattern` of the entry for `pattern` in a listing.
    fn parent_of(label: &str, answer: &Answer, pattern: &str) -> String {
        listed(label, answer)
            .iter()
            .find(|group| group.get("pattern").and_then(Value::as_str) == Some(pattern))
            .and_then(|group| group.get("parent"))
            .and_then(|parent| parent.get("pattern"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {label:?}: no parent for {pattern} in {answer:?}"))
            .to_string()
    }

    // Rows differ along the leaf's terminator and how many levels the insertion adds.
    let cases: [(&str, &str, &str); 3] = [
        ("one level above a wildcard leaf", "/npm/@scope/*", "/npm/*"),
        (
            "one level above an exact leaf",
            "/maven/com.example/artifact$",
            "/maven/com.example/*",
        ),
        (
            "two levels above an exact leaf",
            "/generic/team/service$",
            "/generic/team/*",
        ),
    ];

    for (label, leaf, inserted) in cases {
        let created = create_package_group(&router, domain, leaf, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected 200 creating {leaf}, got {created:?}"
        );

        // With nothing in between, the leaf's parent is the root group.
        let before = list_package_groups(&router, domain, None, None, None).await;
        assert_eq!(
            parent_of(label, &before, leaf),
            ROOT,
            "case {label:?}: before the insertion the leaf's parent is the root group"
        );

        let insertion = create_package_group(&router, domain, inserted, None, None).await;
        assert_eq!(
            insertion.status, 200,
            "case {label:?}: expected 200 creating {inserted}, got {insertion:?}"
        );

        let after = list_package_groups(&router, domain, None, None, None).await;
        assert_eq!(
            parent_of(label, &after, leaf),
            inserted,
            "case {label:?}: after the insertion the leaf's parent is {inserted}"
        );
        assert_eq!(
            parent_of(label, &after, inserted),
            ROOT,
            "case {label:?}: the inserted group itself hangs off the root group"
        );

        // The listing stays the one record: what it reports for the leaf is what a read of
        // the leaf answers.
        let described = describe_package_group(&router, domain, leaf).await;
        assert_eq!(
            described
                .group()
                .get("parent")
                .and_then(|parent| parent.get("pattern"))
                .and_then(Value::as_str),
            Some(inserted),
            "case {label:?}: DescribePackageGroup must agree with the listing, got {described:?}"
        );
    }
}

/// RULE 4: pagination over a domain's package groups, with and without a prefix.
#[tokio::test]
async fn package_group_listing_paginates() {
    let router = emulator();
    let domain = "pg-list-pages";
    create_domain(&router, domain, None).await;

    // Nine groups under `/npm/`, plus three outside it, plus the root group the domain was
    // created with: a prefix walk must page over the nine and never see the others.
    let mut npm: Vec<String> = (1..=9).map(|index| format!("/npm/ns{index}/*")).collect();
    npm.sort();
    let mut others: Vec<String> = vec![
        "/maven/*".to_string(),
        "/pypi/*".to_string(),
        "/generic/team$".to_string(),
    ];
    others.sort();
    for pattern in npm.iter().chain(others.iter()) {
        assert_eq!(
            create_package_group(&router, domain, pattern, None, None)
                .await
                .status,
            200,
            "setup: creating {pattern} must succeed"
        );
    }
    let mut all: Vec<String> = vec![ROOT.to_string()];
    all.extend(npm.iter().cloned());
    all.extend(others.iter().cloned());
    all.sort();

    // Page sizes spanning the class: one at a time, a size that divides the count, one that
    // does not, exactly the count, one past it, and the modelled maximum (1..1000).
    for (max_results, expected_pages) in [
        (Some(1), 9),
        (Some(3), 3),
        (Some(4), 3),
        (Some(9), 1),
        (Some(10), 1),
        (Some(1000), 1),
        (None, 1),
    ] {
        let (walked, pages) = walk(&router, domain, Some("/npm/"), max_results).await;
        assert_eq!(
            walked, npm,
            "case prefix /npm/ at {max_results:?}: the pages must concatenate to every matching group exactly once, got {walked:?}"
        );
        assert_eq!(
            pages, expected_pages,
            "case prefix /npm/ at {max_results:?}: expected {expected_pages} page(s), got {pages}"
        );
    }

    // The same walk with no prefix carries every group the domain holds, the root included.
    let (walked, pages) = walk(&router, domain, None, Some(5)).await;
    assert_eq!(
        walked, all,
        "case no prefix at 5: the pages must concatenate to every group exactly once, got {walked:?}"
    );
    assert_eq!(
        pages, 3,
        "case no prefix at 5: 13 groups at 5 per page is 3 pages, got {pages}"
    );

    // The token resumes strictly after the last entry of the page that reported it, and it
    // belongs to the listing that minted it: another domain's listing does not take it, and
    // neither does a string this service never issued.
    let first = list_package_groups(&router, domain, Some("/npm/"), Some(1), None).await;
    let token = next_token(&first).unwrap_or_else(|| {
        panic!("setup: the first page of 9 must carry a nextToken, got {first:?}")
    });
    let second = list_package_groups(&router, domain, Some("/npm/"), Some(1), Some(&token)).await;
    assert_eq!(
        listed_patterns("resume", &second),
        vec![npm[1].clone()],
        "resuming from the first page must answer the second matching group, got {second:?}"
    );

    let other_domain = "pg-list-pages-two";
    create_domain(&router, other_domain, None).await;
    for (label, listed_domain, token) in [
        ("a token minted for another domain", other_domain, token),
        (
            "a token this service never issued",
            domain,
            "not-a-token".to_string(),
        ),
    ] {
        let refused =
            list_package_groups(&router, listed_domain, None, Some(1), Some(&token)).await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "ValidationException".to_string()),
            "case {label:?}: expected 400 ValidationException, got {refused:?}"
        );
    }
}

/// RULE 6: the refusals, and RULE 5 against stored state.
#[tokio::test]
async fn package_group_listing_refusals() {
    let router = emulator();
    let domain = "pg-list-refusals";
    create_domain(&router, domain, None).await;
    let held = "/npm/*";
    assert_eq!(
        create_package_group(&router, domain, held, None, None)
            .await
            .status,
        200,
        "setup: creating {held} must succeed"
    );

    // A domain the caller does not hold: absent, and pinned to another account.
    for (label, uri_domain, owner) in [
        ("absent domain", "pg-list-absent", None),
        (
            "domain owned by another account",
            domain,
            Some("999999999999"),
        ),
    ] {
        let query = match owner {
            Some(owner) => format!("domain={uri_domain}&domain-owner={owner}"),
            None => format!("domain={uri_domain}"),
        };
        let refused = call(&router, "POST", &format!("/v1/package-groups?{query}"), None).await;
        assert_eq!(
            (refused.status, refused.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected 404 ResourceNotFoundException, got {refused:?}"
        );
        assert!(
            refused.message().contains(uri_domain),
            "case {label:?}: the refusal must name the domain, got {:?}",
            refused.message()
        );
    }

    // `maxResults` outside the modelled range (1..1000) is refused at the boundary; each
    // published bound is accepted.
    for max_results in [0, 1001] {
        let refused = list_package_groups(&router, domain, None, Some(max_results), None).await;
        assert_eq!(
            refused.status, 400,
            "case maxResults={max_results}: expected 400, got {refused:?}"
        );
    }
    for max_results in [1, 1000] {
        let accepted = list_package_groups(&router, domain, None, Some(max_results), None).await;
        assert_eq!(
            accepted.status, 200,
            "case maxResults={max_results}: expected 200, got {accepted:?}"
        );
    }

    // RULE 5: a listing left every record it reported where it was.
    let before = describe_package_group(&router, domain, held).await;
    let listing = list_package_groups(&router, domain, None, None, None).await;
    assert_eq!(
        listing.status, 200,
        "expected 200 from the listing, got {listing:?}"
    );
    let after = describe_package_group(&router, domain, held).await;
    assert_eq!(
        after.group(),
        before.group(),
        "the listing must leave the group's record unchanged"
    );
}

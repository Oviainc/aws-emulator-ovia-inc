// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for `ListSubPackageGroups`.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code and body — not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of hierarchies rather than one
//! recorded example:
//!
//! 1. The listing answers exactly the DIRECT children of the addressed group: the groups
//!    the domain holds whose own `parent` is that group. Never the group itself, never a
//!    grandchild, never a sibling — and the relation is derived from the patterns held, so
//!    creating an intermediate group moves a child off one listing and onto the new one.
//! 2. Each entry carries every member of the group's own description — `PackageGroupSummary`
//!    declares the same members as `PackageGroupDescription`
//!    (`smithy_query(query="shape", shape="PackageGroupSummary")`) — so a listed entry
//!    equals what `DescribePackageGroup` answers for that pattern, member for member,
//!    including the derived `parent` and `originConfiguration`.
//! 3. The listing is `@paginated` over the child patterns: a page carries at most
//!    `maxResults` children, a non-final page reports an opaque `nextToken` that resumes
//!    strictly after the last entry, the final page reports none, and the pages
//!    concatenate to the whole child set exactly once. A token belongs to the listing that
//!    minted it.
//! 4. Reading changes nothing: the same request answers identically when repeated, and a
//!    listing never alters the groups it reports.
//! 5. Refusals: a domain the caller does not hold and a pattern the domain does not hold
//!    are `ResourceNotFoundException` (404) — not an empty page, which would claim a group
//!    exists with no children — and a `packageGroup` that is not a path from the domain
//!    root is `ValidationException` (400).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_domain, emulator, package_group_arn, Answer, ACCOUNT};
use crate::package_group_hierarchy::{create_package_group, describe_package_group, ROOT};

/// `ListSubPackageGroups`: every input member is `httpQuery`-bound on
/// `POST /v1/package-groups/sub-groups`
/// (`smithy_query(query="operation", operation="ListSubPackageGroups")`).
async fn list_sub_package_groups(
    router: &MultiServiceRouter,
    domain: &str,
    pattern: &str,
    max_results: Option<u32>,
    next_token: Option<&str>,
) -> Answer {
    let mut uri =
        format!("/v1/package-groups/sub-groups?domain={domain}&package-group={pattern}");
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

/// Every child pattern of `parent`, walked page by page at a page size of `max_results`.
///
/// Returns the concatenated patterns and how many requests it took, so a caller can assert
/// both the contents and that the walk actually paginated.
async fn walk_children(
    router: &MultiServiceRouter,
    domain: &str,
    parent: &str,
    max_results: Option<u32>,
) -> (Vec<String>, usize) {
    let label = format!("walk {parent} at {max_results:?}");
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let answer =
            list_sub_package_groups(router, domain, parent, max_results, token.as_deref()).await;
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

/// One row of the hierarchy table: a parent pattern and the children it must report once
/// every pattern in [`HIERARCHY`] exists.
struct ChildrenCase {
    label: &'static str,
    parent: &'static str,
    /// Written out by hand from the pattern paths rather than computed, so the expectation
    /// does not restate the implementation. In the pattern order the listing reports.
    children: &'static [&'static str],
}

/// Every pattern created for the hierarchy table, in an order that is deliberately NOT
/// top-down: `/maven/com.example/*` and `/pypi/requests$` are created before (or without)
/// the group that ends up above them.
const HIERARCHY: &[&str] = &[
    "/npm/*",
    "/npm/@scope/*",
    "/npm/@scope/package$",
    "/npm/other/*",
    "/maven/com.example/*",
    "/maven/*",
    "/pypi/requests$",
];

/// The table. Rows differ along what rule 1 generalises over: the DEPTH of the parent (the
/// domain root, one and two segments below it), the NUMBER of children (none, one, two,
/// three), the terminator of parent and child (`*` wildcard vs `$` exact), and the
/// character classes a namespace segment admits (`@scope`, a dotted Maven group id).
const CHILDREN_CASES: &[ChildrenCase] = &[
    ChildrenCase {
        label: "the root group: one child per format, and an exact pattern with no format group",
        parent: ROOT,
        children: &["/maven/*", "/npm/*", "/pypi/requests$"],
    },
    ChildrenCase {
        label: "a format group with two namespace children, not its grandchild",
        parent: "/npm/*",
        children: &["/npm/@scope/*", "/npm/other/*"],
    },
    ChildrenCase {
        label: "a namespace group whose only child is an exact pattern",
        parent: "/npm/@scope/*",
        children: &["/npm/@scope/package$"],
    },
    ChildrenCase {
        label: "an exact pattern is nobody's ancestor",
        parent: "/npm/@scope/package$",
        children: &[],
    },
    ChildrenCase {
        label: "a namespace group with no children at all",
        parent: "/npm/other/*",
        children: &[],
    },
    ChildrenCase {
        label: "a format group created after its own child",
        parent: "/maven/*",
        children: &["/maven/com.example/*"],
    },
    ChildrenCase {
        label: "an exact pattern directly under the root",
        parent: "/pypi/requests$",
        children: &[],
    },
];

/// RULES 1, 2 and 4 over the whole table.
#[tokio::test]
async fn sub_package_groups_are_the_direct_children_of_the_addressed_group() {
    let router = emulator();
    let domain = "sub-pg-children";
    let created_domain = create_domain(&router, domain, None).await;
    assert_eq!(
        created_domain.status, 200,
        "domain setup: expected 200, got {created_domain:?}"
    );
    for pattern in HIERARCHY {
        let created = create_package_group(&router, domain, pattern, None, None).await;
        assert_eq!(
            created.status, 200,
            "setup {pattern:?}: expected 200, got {created:?}"
        );
    }

    for case in CHILDREN_CASES {
        let label = case.label;
        let answer = list_sub_package_groups(&router, domain, case.parent, None, None).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected 200, got {answer:?}"
        );

        // RULE 1.
        assert_eq!(
            listed_patterns(label, &answer),
            case.children,
            "case {label:?}: expected the direct children of {:?}, got {answer:?}",
            case.parent
        );
        assert!(
            next_token(&answer).is_none(),
            "case {label:?}: a single complete page reports no nextToken, got {answer:?}"
        );

        // RULE 2: the entry is the group's own record, derived members included.
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
            assert_eq!(
                entry
                    .get("parent")
                    .and_then(|parent| parent.get("pattern"))
                    .and_then(Value::as_str),
                Some(case.parent),
                "case {label:?}/{pattern}: a child's parent is the group listed, got {entry:?}"
            );
        }

        // RULE 4: the listing is a read, so repeating it answers identically.
        let again = list_sub_package_groups(&router, domain, case.parent, None, None).await;
        assert_eq!(
            again.body, answer.body,
            "case {label:?}: repeating the listing must answer identically"
        );
    }
}

/// RULE 1, isolated: the parent/child relation is DERIVED from the patterns held, so
/// inserting a group between a parent and a leaf moves that leaf off the parent's listing
/// and onto the inserted group's.
#[tokio::test]
async fn inserting_a_group_moves_a_child_between_listings() {
    let router = emulator();
    let domain = "sub-pg-reparent";
    create_domain(&router, domain, None).await;

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

        // With nothing in between, the leaf is a child of the root group.
        let from_root = list_sub_package_groups(&router, domain, ROOT, None, None).await;
        assert!(
            listed_patterns(label, &from_root).contains(&leaf.to_string()),
            "case {label:?}: before the insertion the root must list {leaf}, got {from_root:?}"
        );

        let insertion = create_package_group(&router, domain, inserted, None, None).await;
        assert_eq!(
            insertion.status, 200,
            "case {label:?}: expected 200 creating {inserted}, got {insertion:?}"
        );

        let from_root = list_sub_package_groups(&router, domain, ROOT, None, None).await;
        assert!(
            !listed_patterns(label, &from_root).contains(&leaf.to_string()),
            "case {label:?}: after the insertion the root must not list {leaf}, got {from_root:?}"
        );
        let from_inserted = list_sub_package_groups(&router, domain, inserted, None, None).await;
        assert_eq!(
            listed_patterns(label, &from_inserted),
            vec![leaf.to_string()],
            "case {label:?}: the inserted group must list {leaf}, got {from_inserted:?}"
        );
    }
}

/// RULE 3: pagination over one group's children.
#[tokio::test]
async fn sub_package_groups_paginate() {
    let router = emulator();
    let domain = "sub-pg-pages";
    create_domain(&router, domain, None).await;
    let parent = "/npm/*";
    assert_eq!(
        create_package_group(&router, domain, parent, None, None)
            .await
            .status,
        200,
        "setup: creating {parent} must succeed"
    );

    // Seven children, plus a grandchild under each of two of them: the grandchildren must
    // never appear on a page of the parent's listing, whatever the page size.
    let mut children: Vec<String> = (1..=7).map(|index| format!("/npm/ns{index}/*")).collect();
    children.sort();
    for child in &children {
        assert_eq!(
            create_package_group(&router, domain, child, None, None)
                .await
                .status,
            200,
            "setup: creating {child} must succeed"
        );
    }
    for grandchild in ["/npm/ns1/pkg$", "/npm/ns7/deep/*"] {
        assert_eq!(
            create_package_group(&router, domain, grandchild, None, None)
                .await
                .status,
            200,
            "setup: creating {grandchild} must succeed"
        );
    }

    // Page sizes spanning the class: one at a time, a size that divides the count, one
    // that does not, exactly the count, one past it, and the modelled maximum (1..1000).
    for (max_results, expected_pages) in [
        (Some(1), 7),
        (Some(2), 4),
        (Some(3), 3),
        (Some(7), 1),
        (Some(8), 1),
        (Some(1000), 1),
        (None, 1),
    ] {
        let (walked, pages) = walk_children(&router, domain, parent, max_results).await;
        assert_eq!(
            walked, children,
            "case {max_results:?}: the pages must concatenate to every child exactly once, got {walked:?}"
        );
        assert_eq!(
            pages, expected_pages,
            "case {max_results:?}: expected {expected_pages} page(s), got {pages}"
        );
    }

    // A token belongs to the listing that minted it: one minted for this parent's children
    // is not a place in another parent's listing, and neither is a string this service
    // never issued.
    let first = list_sub_package_groups(&router, domain, parent, Some(1), None).await;
    let token = next_token(&first)
        .unwrap_or_else(|| panic!("setup: the first page of 7 must carry a nextToken, got {first:?}"));
    for (label, listed_parent, token) in [
        ("a token minted for another group", ROOT, token.clone()),
        (
            "a token this service never issued",
            parent,
            "not-a-token".to_string(),
        ),
    ] {
        let refused =
            list_sub_package_groups(&router, domain, listed_parent, Some(1), Some(&token)).await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "ValidationException".to_string()),
            "case {label:?}: expected 400 ValidationException, got {refused:?}"
        );
    }

    // The token resumes strictly after the last entry of the page that reported it.
    let second = list_sub_package_groups(&router, domain, parent, Some(1), Some(&token)).await;
    assert_eq!(
        listed_patterns("resume", &second),
        vec![children[1].clone()],
        "resuming from the first page must answer the second child, got {second:?}"
    );
}

/// RULE 5: the refusals.
#[tokio::test]
async fn sub_package_group_listing_refusals() {
    let router = emulator();
    let domain = "sub-pg-refusals";
    create_domain(&router, domain, None).await;
    let held = "/npm/*";
    assert_eq!(
        create_package_group(&router, domain, held, None, None)
            .await
            .status,
        200,
        "setup: creating {held} must succeed"
    );

    // A pattern the domain does not hold. Rows differ in depth and terminator, and one is
    // a PREFIX of a held pattern rather than unrelated.
    for pattern in ["/npm$", "/maven/*", "/npm/@scope/*", "/npm/@scope/package$"] {
        let missing = list_sub_package_groups(&router, domain, pattern, None, None).await;
        assert_eq!(
            (missing.status, missing.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {pattern:?}: expected 404 ResourceNotFoundException, got {missing:?}"
        );
        assert!(
            missing.message().contains(pattern),
            "case {pattern:?}: the refusal must name the pattern, got {:?}",
            missing.message()
        );
    }

    // A domain the caller does not hold: absent, and pinned to another account.
    for (label, uri_domain, owner) in [
        ("absent domain", "sub-pg-absent", None),
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
        let refused = call(
            &router,
            "POST",
            &format!("/v1/package-groups/sub-groups?{query}&package-group={held}"),
            None,
        )
        .await;
        assert_eq!(
            (refused.status, refused.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected 404 ResourceNotFoundException, got {refused:?}"
        );
    }

    // A `packageGroup` that is not a path from the domain root is a bad REQUEST, not a
    // lookup that misses; a value outside the modelled length bounds (len 2..520) is
    // refused at the boundary, which answers 400 without reaching the handler.
    for (label, pattern, refused_by_handler) in [
        ("no leading slash", "npm/*".to_string(), true),
        ("relative exact pattern", "npm/pkg$".to_string(), true),
        ("one under the modelled minimum", "*".to_string(), false),
        (
            "one past the modelled maximum",
            format!("/npm/{}/*", "a".repeat(514)),
            false,
        ),
    ] {
        let refused = list_sub_package_groups(&router, domain, &pattern, None, None).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: expected 400, got {refused:?}"
        );
        if refused_by_handler {
            assert_eq!(
                refused.code(),
                "ValidationException",
                "case {label:?}: expected the ValidationException code, got {refused:?}"
            );
        }
    }

    // `maxResults` outside the modelled range (1..1000) is refused at the boundary.
    for max_results in [0, 1001] {
        let refused = list_sub_package_groups(&router, domain, held, Some(max_results), None).await;
        assert_eq!(
            refused.status, 400,
            "case maxResults={max_results}: expected 400, got {refused:?}"
        );
    }
}

/// RULE 4, against stored state: a listing of a group whose restrictions were just moved
/// reports the children's INHERITED configuration, and reading it leaves every group's own
/// record where it was.
#[tokio::test]
async fn listing_reports_inherited_origin_configuration_without_changing_it() {
    /// Every `PackageGroupOriginRestrictionType` the model declares
    /// (`smithy_query(query="shape", shape="PackageGroupOriginRestrictionType")`).
    const RESTRICTION_TYPES: [&str; 3] = ["EXTERNAL_UPSTREAM", "INTERNAL_UPSTREAM", "PUBLISH"];

    let router = emulator();
    let domain = "sub-pg-inherit";
    create_domain(&router, domain, None).await;
    let parent = "/npm/*";
    let children = ["/npm/@scope/*", "/npm/other/*"];
    for pattern in [parent, children[0], children[1]] {
        assert_eq!(
            create_package_group(&router, domain, pattern, None, None)
                .await
                .status,
            200,
            "setup: creating {pattern} must succeed"
        );
    }

    // Move the PARENT's restrictions off INHERIT, one mode per type, so each child's
    // effective mode has a distinct value to resolve to.
    let modes = [
        ("EXTERNAL_UPSTREAM", "BLOCK"),
        ("INTERNAL_UPSTREAM", "ALLOW"),
        ("PUBLISH", "BLOCK"),
    ];
    let restrictions: serde_json::Map<String, Value> = modes
        .iter()
        .map(|(restriction_type, mode)| ((*restriction_type).to_string(), json!(mode)))
        .collect();
    let updated = call(
        &router,
        "PUT",
        &format!("/v1/package-group-origin-configuration?domain={domain}&package-group={parent}"),
        Some(json!({ "restrictions": Value::Object(restrictions) })),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "setup: moving {parent} off INHERIT expected 200, got {updated:?}"
    );

    let mut before: Vec<Value> = Vec::new();
    for pattern in children {
        before.push(
            describe_package_group(&router, domain, pattern)
                .await
                .group()
                .clone(),
        );
    }

    let listing = list_sub_package_groups(&router, domain, parent, None, None).await;
    assert_eq!(
        listing.status, 200,
        "expected 200 from the listing, got {listing:?}"
    );
    let entries = listed("inherited configuration", &listing);
    assert_eq!(
        listed_patterns("inherited configuration", &listing),
        children,
        "expected both children, got {listing:?}"
    );
    for (entry, child) in entries.iter().zip(children) {
        for (restriction_type, parent_mode) in modes {
            let restriction = entry
                .get("originConfiguration")
                .and_then(|configuration| configuration.get("restrictions"))
                .and_then(|restrictions| restrictions.get(restriction_type))
                .unwrap_or_else(|| {
                    panic!("case {child}: no {restriction_type} restriction in {entry:?}")
                });
            assert_eq!(
                restriction.get("mode").and_then(Value::as_str),
                Some("INHERIT"),
                "case {child}/{restriction_type}: a child's own mode still inherits, got {restriction:?}"
            );
            assert_eq!(
                restriction.get("effectiveMode").and_then(Value::as_str),
                Some(parent_mode),
                "case {child}/{restriction_type}: expected the parent's mode {parent_mode}, got {restriction:?}"
            );
            assert_eq!(
                restriction
                    .get("inheritedFrom")
                    .and_then(|from| from.get("pattern"))
                    .and_then(Value::as_str),
                Some(parent),
                "case {child}/{restriction_type}: expected inheritedFrom {parent}, got {restriction:?}"
            );
        }
    }
    for restriction_type in RESTRICTION_TYPES {
        assert!(
            modes
                .iter()
                .any(|(moved, _)| *moved == restriction_type),
            "every modelled restriction type must be exercised, {restriction_type} was not"
        );
    }

    // RULE 4: the listing changed none of the records it reported.
    for (pattern, before) in children.iter().zip(before) {
        let after = describe_package_group(&router, domain, pattern).await;
        assert_eq!(
            after.group(),
            &before,
            "case {pattern}: the listing must leave the group's record unchanged"
        );
    }
}

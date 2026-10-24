// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for a package group's allowed-repository lists:
//! `UpdatePackageGroupOriginConfiguration` writes them and
//! `ListAllowedRepositoriesForGroup` reports them.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code and body.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one example:
//!
//! 1. The allowed-repository list belongs to ONE restriction of ONE group: a list written
//!    for a restriction type is reported for that type and no other, and reading it back
//!    answers exactly the names stored, in key order.
//! 2. Only a restriction whose OWN `mode` is `ALLOW_SPECIFIC_REPOSITORIES` has such a
//!    list, so listing one in any other mode — `INHERIT` included, even where the
//!    `effectiveMode` inherited from an ancestor IS `ALLOW_SPECIFIC_REPOSITORIES` — is
//!    refused as an invalid parameter rather than answered with an empty list.
//! 3. A restriction whose list a request adds to or removes from must END that request in
//!    `ALLOW_SPECIFIC_REPOSITORIES`, whether that mode was already stored or is set by the
//!    same request; the refusal names the RESULTING mode and the types it refused.
//! 4. A group with no parent cannot be set to `INHERIT`: there is nothing above it to
//!    inherit from.
//! 5. `allowedRepositoryUpdates` reports what the stored list actually gained and lost.
//! 6. The listing is `@paginated` over the names: a `maxResults` walk covers the list
//!    exactly once, and a `nextToken` this service did not mint for THIS listing is
//!    refused.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_domain, emulator, Answer};
use crate::package_group_hierarchy::{create_package_group, describe_package_group, ROOT};

/// Every `PackageGroupOriginRestrictionType` the model declares, in the model's own order.
///
/// Source: `smithy_query(query="shape", shape="PackageGroupOriginRestrictionType")` →
/// values `["EXTERNAL_UPSTREAM", "INTERNAL_UPSTREAM", "PUBLISH"]`.
pub(crate) const RESTRICTION_TYPES: [&str; 3] =
    ["EXTERNAL_UPSTREAM", "INTERNAL_UPSTREAM", "PUBLISH"];

/// The only `PackageGroupOriginRestrictionMode` that carries an allowed-repository list.
pub(crate) const ALLOW_SPECIFIC: &str = "ALLOW_SPECIFIC_REPOSITORIES";

/// The modes that carry NO allowed-repository list — the rest of
/// `PackageGroupOriginRestrictionMode` (`["ALLOW", "ALLOW_SPECIFIC_REPOSITORIES", "BLOCK",
/// "INHERIT"]`).
const MODES_WITHOUT_A_LIST: [&str; 3] = ["ALLOW", "BLOCK", "INHERIT"];

/// `ListAllowedRepositoriesForGroup`: every input member is `httpQuery`-bound.
pub(crate) async fn list_allowed_repositories(
    router: &MultiServiceRouter,
    domain: &str,
    pattern: &str,
    restriction_type: &str,
    max_results: Option<u32>,
    next_token: Option<&str>,
) -> Answer {
    let mut uri = format!(
        "/v1/package-group-allowed-repositories?domain={domain}&package-group={pattern}\
         &originRestrictionType={restriction_type}"
    );
    if let Some(max_results) = max_results {
        uri.push_str(&format!("&max-results={max_results}"));
    }
    if let Some(next_token) = next_token {
        uri.push_str(&format!("&next-token={next_token}"));
    }
    call(router, "GET", &uri, None).await
}

/// `UpdatePackageGroupOriginConfiguration`: the domain and the pattern are
/// `httpQuery`-bound, everything else travels in the body.
pub(crate) async fn update_origin_configuration(
    router: &MultiServiceRouter,
    domain: &str,
    pattern: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/v1/package-group-origin-configuration?domain={domain}&package-group={pattern}"),
        Some(body),
    )
    .await
}

/// A `PackageGroupAllowedRepository` list for one restriction type.
pub(crate) fn allowed_repository_entries(restriction_type: &str, repositories: &[&str]) -> Value {
    Value::Array(
        repositories
            .iter()
            .map(|repository| {
                json!({
                    "repositoryName": repository,
                    "originRestrictionType": restriction_type,
                })
            })
            .collect(),
    )
}

/// Put `repositories` in the allowed-repository list of `restriction_type`, moving that
/// restriction to `ALLOW_SPECIFIC_REPOSITORIES` in the same call.
pub(crate) async fn allow_specific_repositories(
    router: &MultiServiceRouter,
    domain: &str,
    pattern: &str,
    restriction_type: &str,
    repositories: &[&str],
) -> Answer {
    update_origin_configuration(
        router,
        domain,
        pattern,
        json!({
            "restrictions": { restriction_type: ALLOW_SPECIFIC },
            "addAllowedRepositories": allowed_repository_entries(restriction_type, repositories),
        }),
    )
    .await
}

/// The `allowedRepositories` of a successful listing.
pub(crate) fn listed(label: &str, answer: &Answer) -> Vec<String> {
    answer
        .body
        .get("allowedRepositories")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {label:?}: no allowedRepositories in {answer:?}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("case {label:?}: a non-string name in {answer:?}"))
                .to_string()
        })
        .collect()
}

/// The `nextToken` of a listing, or `None` on the last page.
fn next_token(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One restriction of a group's `originConfiguration`, as `DescribePackageGroup` reports
/// it.
pub(crate) fn restriction<'a>(label: &str, group: &'a Value, restriction_type: &str) -> &'a Value {
    group
        .get("originConfiguration")
        .and_then(|configuration| configuration.get("restrictions"))
        .and_then(|restrictions| restrictions.get(restriction_type))
        .unwrap_or_else(|| panic!("case {label:?}: no {restriction_type} restriction in {group:?}"))
}

/// One row of the write-then-read table: a restriction type and the repositories its list
/// is given.
struct ListCase {
    label: &'static str,
    pattern: &'static str,
    restriction_type: &'static str,
    repositories: Vec<String>,
}

/// The table. Rows differ along what rules 1 and 5 generalise over: the restriction TYPE
/// (every value the model declares), the NUMBER of repositories in one list, the character
/// classes a repository name admits and the modelled name-length bounds.
///
/// `RepositoryName` is len 2..100 pattern `^[A-Za-z0-9][A-Za-z0-9._\-]{1,99}$`
/// (`smithy_query(query="shape", shape="RepositoryName")`), so the shortest and longest
/// rows sit exactly on those bounds.
fn list_cases() -> Vec<ListCase> {
    let longest_name = format!("r{}", "a".repeat(99));
    assert_eq!(
        longest_name.chars().count(),
        100,
        "the longest repository-name row must be exactly the modelled maximum"
    );

    vec![
        ListCase {
            label: "one repository, the minimum name length",
            pattern: "/npm/*",
            restriction_type: "EXTERNAL_UPSTREAM",
            repositories: vec!["ab".to_string()],
        },
        ListCase {
            label: "several repositories, every punctuation the name admits",
            pattern: "/maven/*",
            restriction_type: "INTERNAL_UPSTREAM",
            repositories: vec![
                "prod-repo".to_string(),
                "Repo.With.Dots".to_string(),
                "repo_with_underscores".to_string(),
                "0leading-digit".to_string(),
            ],
        },
        ListCase {
            label: "the modelled maximum name length",
            pattern: "/pypi/*",
            restriction_type: "PUBLISH",
            repositories: vec![longest_name],
        },
        ListCase {
            label: "a list long enough to span pages",
            pattern: "/generic/*",
            restriction_type: "EXTERNAL_UPSTREAM",
            repositories: (0..7).map(|index| format!("repo-{index}")).collect(),
        },
    ]
}

/// RULES 1, 2 and 5: a written list reads back exactly, for the restriction type it was
/// written for and no other, and the update report names what the list gained.
#[tokio::test]
async fn allowed_repositories_round_trip_per_restriction_type() {
    let router = emulator();
    let domain = "pg-allowed-repos";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "domain setup must succeed"
    );

    for case in list_cases() {
        let label = case.label;
        assert_eq!(
            create_package_group(&router, domain, case.pattern, None, None)
                .await
                .status,
            200,
            "case {label:?}: creating {} must succeed",
            case.pattern
        );

        let names: Vec<&str> = case.repositories.iter().map(String::as_str).collect();
        let updated = allow_specific_repositories(
            &router,
            domain,
            case.pattern,
            case.restriction_type,
            &names,
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected 200 from the update, got {updated:?}"
        );

        // RULE 5: every name is new to the list, so every name is reported ADDED — in the
        // stored order, which is the order the listing reports too.
        let mut expected_names = case.repositories.clone();
        expected_names.sort();
        assert_eq!(
            updated.body.get("allowedRepositoryUpdates"),
            Some(&json!({ case.restriction_type: { "ADDED": expected_names } })),
            "case {label:?}: expected the names added, got {updated:?}"
        );

        // RULE 1: the list reads back exactly, in key order.
        let listed_answer = list_allowed_repositories(
            &router,
            domain,
            case.pattern,
            case.restriction_type,
            None,
            None,
        )
        .await;
        assert_eq!(
            listed_answer.status, 200,
            "case {label:?}: expected 200 from the listing, got {listed_answer:?}"
        );
        assert_eq!(
            listed(label, &listed_answer),
            expected_names,
            "case {label:?}: expected the stored list, got {listed_answer:?}"
        );
        assert_eq!(
            next_token(&listed_answer),
            None,
            "case {label:?}: a single page must report no nextToken, got {listed_answer:?}"
        );

        // The same record, through the group's own description: the mode moved, and
        // `repositoriesCount` counts the list.
        let described = describe_package_group(&router, domain, case.pattern).await;
        let restriction = restriction(label, described.group(), case.restriction_type);
        assert_eq!(
            restriction.get("mode").and_then(Value::as_str),
            Some(ALLOW_SPECIFIC),
            "case {label:?}: expected the mode the update set, got {restriction:?}"
        );
        assert_eq!(
            restriction.get("repositoriesCount").and_then(Value::as_u64),
            Some(case.repositories.len() as u64),
            "case {label:?}: expected repositoriesCount {}, got {restriction:?}",
            case.repositories.len()
        );

        // RULE 1, the isolation half: the OTHER restriction types of the same group got no
        // list, so listing them is refused rather than answered with these names.
        for other in RESTRICTION_TYPES
            .iter()
            .filter(|other| **other != case.restriction_type)
        {
            let refused =
                list_allowed_repositories(&router, domain, case.pattern, other, None, None).await;
            assert_eq!(
                (refused.status, refused.code()),
                (400, "ValidationException".to_string()),
                "case {label:?}/{other}: a restriction with no list of its own must be refused, got {refused:?}"
            );
        }

        // RULE 5, the other direction: removing one stored name reports it REMOVED and
        // takes it out of the listing, leaving the rest.
        let removed = expected_names
            .first()
            .expect("every row names at least one repository")
            .clone();
        let removal = update_origin_configuration(
            &router,
            domain,
            case.pattern,
            json!({
                "removeAllowedRepositories":
                    allowed_repository_entries(case.restriction_type, &[removed.as_str()]),
            }),
        )
        .await;
        assert_eq!(
            removal.status, 200,
            "case {label:?}: expected 200 from the removal, got {removal:?}"
        );
        assert_eq!(
            removal.body.get("allowedRepositoryUpdates"),
            Some(&json!({ case.restriction_type: { "REMOVED": [removed.clone()] } })),
            "case {label:?}: expected {removed} reported removed, got {removal:?}"
        );
        let after_removal = list_allowed_repositories(
            &router,
            domain,
            case.pattern,
            case.restriction_type,
            None,
            None,
        )
        .await;
        assert_eq!(
            listed(label, &after_removal),
            expected_names[1..].to_vec(),
            "case {label:?}: expected {removed} gone from the list, got {after_removal:?}"
        );

        // A removal of a name the list no longer holds moves nothing, so it reports
        // nothing (RULE 5).
        let repeat = update_origin_configuration(
            &router,
            domain,
            case.pattern,
            json!({
                "removeAllowedRepositories":
                    allowed_repository_entries(case.restriction_type, &[removed.as_str()]),
            }),
        )
        .await;
        assert_eq!(
            (repeat.status, repeat.body.get("allowedRepositoryUpdates")),
            (200, Some(&json!({}))),
            "case {label:?}: a removal that moves nothing must report nothing, got {repeat:?}"
        );
    }
}

/// RULE 2: the mode that decides whether there is a list to report is the group's OWN
/// mode. A group that INHERITS `ALLOW_SPECIFIC_REPOSITORIES` from an ancestor resolves
/// that effective mode — and holds no list of its own, so listing it is refused.
#[tokio::test]
async fn an_inheriting_group_has_no_allowed_repository_list_of_its_own() {
    let router = emulator();
    let domain = "pg-inherited-list";
    create_domain(&router, domain, None).await;

    let parent = "/npm/*";
    // Rows differ in DEPTH below the group that owns the list, so the rule is asserted on
    // a direct child and on a grandchild whose chain runs through another inheriting
    // group, and on both pattern terminators.
    let children = [
        ("direct child, wildcard", "/npm/@scope/*"),
        ("grandchild, exact", "/npm/@scope/package$"),
    ];

    create_package_group(&router, domain, parent, None, None).await;
    for (_, child) in children {
        create_package_group(&router, domain, child, None, None).await;
    }

    let repositories = ["upstream-one", "upstream-two"];
    let updated = allow_specific_repositories(
        &router,
        domain,
        parent,
        "EXTERNAL_UPSTREAM",
        &repositories,
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "setup: the parent's list must be written, got {updated:?}"
    );

    // The group that OWNS the list reports it.
    let owner = list_allowed_repositories(&router, domain, parent, "EXTERNAL_UPSTREAM", None, None)
        .await;
    assert_eq!(
        listed("owner", &owner),
        repositories
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<String>>(),
        "the group that holds the list must report it, got {owner:?}"
    );

    for (label, child) in children {
        // The effective mode DOES resolve to the ancestor's, with its count...
        let described = describe_package_group(&router, domain, child).await;
        let restriction = restriction(label, described.group(), "EXTERNAL_UPSTREAM");
        assert_eq!(
            restriction.get("mode").and_then(Value::as_str),
            Some("INHERIT"),
            "case {label:?}: expected an inheriting mode, got {restriction:?}"
        );
        assert_eq!(
            restriction.get("effectiveMode").and_then(Value::as_str),
            Some(ALLOW_SPECIFIC),
            "case {label:?}: expected the ancestor's effective mode, got {restriction:?}"
        );
        assert_eq!(
            restriction
                .get("inheritedFrom")
                .and_then(|from| from.get("pattern"))
                .and_then(Value::as_str),
            Some(parent),
            "case {label:?}: expected the mode inherited from {parent}, got {restriction:?}"
        );

        // ...and listing the child is still refused: the list is the ANCESTOR's.
        let refused =
            list_allowed_repositories(&router, domain, child, "EXTERNAL_UPSTREAM", None, None)
                .await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "ValidationException".to_string()),
            "case {label:?}: expected a 400 ValidationException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            format!("OriginRestrictionMode is not {ALLOW_SPECIFIC} for the given OriginControlType"),
            "case {label:?}: expected the refusal to name the mode required, got {refused:?}"
        );
    }
}

/// RULE 2 over the rest of `PackageGroupOriginRestrictionMode`: no mode but
/// `ALLOW_SPECIFIC_REPOSITORIES` has a list, whatever list the restriction held before.
#[tokio::test]
async fn listing_a_restriction_without_a_specific_repository_list_is_refused() {
    let router = emulator();
    let domain = "pg-no-list";
    create_domain(&router, domain, None).await;
    let group = "/npm/*";
    create_package_group(&router, domain, group, None, None).await;

    for restriction_type in RESTRICTION_TYPES {
        for mode in MODES_WITHOUT_A_LIST {
            let label = format!("{restriction_type}/{mode}");

            // Start from a list that DOES exist, so the refusal comes from the mode and
            // not from an empty list.
            let seeded = allow_specific_repositories(
                &router,
                domain,
                group,
                restriction_type,
                &["seeded-repo"],
            )
            .await;
            assert_eq!(
                seeded.status, 200,
                "case {label:?}: seeding the list must succeed, got {seeded:?}"
            );

            let moved = update_origin_configuration(
                &router,
                domain,
                group,
                json!({ "restrictions": { restriction_type: mode } }),
            )
            .await;
            assert_eq!(
                moved.status, 200,
                "case {label:?}: moving the mode must succeed, got {moved:?}"
            );

            let refused =
                list_allowed_repositories(&router, domain, group, restriction_type, None, None)
                    .await;
            assert_eq!(
                (refused.status, refused.code()),
                (400, "ValidationException".to_string()),
                "case {label:?}: expected a 400 ValidationException, got {refused:?}"
            );

            // The list itself SURVIVED the mode change: moving back reports it again, so
            // nothing was silently discarded.
            let back = update_origin_configuration(
                &router,
                domain,
                group,
                json!({ "restrictions": { restriction_type: ALLOW_SPECIFIC } }),
            )
            .await;
            assert_eq!(
                back.status, 200,
                "case {label:?}: moving the mode back must succeed, got {back:?}"
            );
            let listed_again =
                list_allowed_repositories(&router, domain, group, restriction_type, None, None)
                    .await;
            assert_eq!(
                listed(&label, &listed_again),
                vec!["seeded-repo".to_string()],
                "case {label:?}: the stored list must survive a mode change, got {listed_again:?}"
            );
        }
    }
}

/// RULES 3 and 4: the cross-field refusals of the write, and the record left untouched by
/// each one.
#[tokio::test]
async fn origin_configuration_update_refusals() {
    let router = emulator();
    let domain = "pg-update-refusals";
    create_domain(&router, domain, None).await;
    let group = "/npm/*";
    create_package_group(&router, domain, group, None, None).await;

    // RULE 3. Rows differ along WHERE the resulting mode comes from (the stored one, or
    // one this same request sets), which list member is used, and how many restriction
    // types one request offends.
    let refusals = [
        (
            "add while the stored mode inherits",
            json!({
                "addAllowedRepositories":
                    allowed_repository_entries("EXTERNAL_UPSTREAM", &["repo-one"]),
            }),
            "[EXTERNAL_UPSTREAM]",
        ),
        (
            "add while the same request sets INHERIT",
            json!({
                "restrictions": { "EXTERNAL_UPSTREAM": "INHERIT" },
                "addAllowedRepositories":
                    allowed_repository_entries("EXTERNAL_UPSTREAM", &["repo-one"]),
            }),
            "[EXTERNAL_UPSTREAM]",
        ),
        (
            "remove while the same request sets ALLOW",
            json!({
                "restrictions": { "PUBLISH": "ALLOW" },
                "removeAllowedRepositories":
                    allowed_repository_entries("PUBLISH", &["repo-one"]),
            }),
            "[PUBLISH]",
        ),
        (
            "two offending types in one request",
            json!({
                "restrictions": { "PUBLISH": "BLOCK" },
                "addAllowedRepositories": json!([
                    { "repositoryName": "repo-one", "originRestrictionType": "PUBLISH" },
                    { "repositoryName": "repo-two", "originRestrictionType": "INTERNAL_UPSTREAM" },
                ]),
            }),
            "[INTERNAL_UPSTREAM, PUBLISH]",
        ),
    ];

    for (label, body, expected_types) in refusals {
        let refused = update_origin_configuration(&router, domain, group, body).await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "ValidationException".to_string()),
            "case {label:?}: expected a 400 ValidationException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            format!(
                "Resulting mode is not {ALLOW_SPECIFIC} for {expected_types}. Cannot add or \
                 remove allowed-repositories."
            ),
            "case {label:?}: expected the refusal to name the resulting mode and the types, got {refused:?}"
        );

        // Nothing was written: every restriction of the group still inherits.
        let described = describe_package_group(&router, domain, group).await;
        for restriction_type in RESTRICTION_TYPES {
            let restriction = restriction(label, described.group(), restriction_type);
            assert_eq!(
                restriction.get("mode").and_then(Value::as_str),
                Some("INHERIT"),
                "case {label:?}/{restriction_type}: a refused update must store nothing, got {restriction:?}"
            );
        }
    }

    // RULE 4: the root group has no parent, so it cannot inherit — for any restriction
    // type — and a group that HAS one can.
    for restriction_type in RESTRICTION_TYPES {
        let refused = update_origin_configuration(
            &router,
            domain,
            ROOT,
            json!({ "restrictions": { restriction_type: "INHERIT" } }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "ValidationException".to_string()),
            "case {restriction_type:?}: the root group cannot inherit, got {refused:?}"
        );
        let root = describe_package_group(&router, domain, ROOT).await;
        assert_eq!(
            restriction(restriction_type, root.group(), restriction_type)
                .get("mode")
                .and_then(Value::as_str),
            Some("ALLOW"),
            "case {restriction_type:?}: a refused update must leave the root group alone, got {root:?}"
        );

        let accepted = update_origin_configuration(
            &router,
            domain,
            group,
            json!({ "restrictions": { restriction_type: "INHERIT" } }),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {restriction_type:?}: a group with a parent can inherit, got {accepted:?}"
        );
    }
}

/// RULE 6: the listing pages over the names. A walk at each page size covers the list once,
/// in order, and a `nextToken` from another listing is refused.
#[tokio::test]
async fn allowed_repositories_paginate() {
    let router = emulator();
    let domain = "pg-allowed-paging";
    create_domain(&router, domain, None).await;
    let group = "/npm/*";
    create_package_group(&router, domain, group, None, None).await;

    let repositories: Vec<String> = (0..7).map(|index| format!("repo-{index}")).collect();
    let names: Vec<&str> = repositories.iter().map(String::as_str).collect();
    assert_eq!(
        allow_specific_repositories(&router, domain, group, "EXTERNAL_UPSTREAM", &names)
            .await
            .status,
        200,
        "setup: the list must be written"
    );
    // A second list, for another restriction type of the same group, so a cursor minted
    // for one cannot resolve in the other.
    assert_eq!(
        allow_specific_repositories(&router, domain, group, "PUBLISH", &names)
            .await
            .status,
        200,
        "setup: the second list must be written"
    );

    // Page sizes from the modelled minimum up past the list's own length, plus the
    // modelled maximum (`ListAllowedRepositoriesForGroupMaxResults` is range 1..1000).
    for page_size in [1_u32, 2, 3, 7, 8, 1000] {
        let label = format!("page size {page_size}");
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let page = list_allowed_repositories(
                &router,
                domain,
                group,
                "EXTERNAL_UPSTREAM",
                Some(page_size),
                token.as_deref(),
            )
            .await;
            assert_eq!(
                page.status, 200,
                "case {label:?}: expected 200, got {page:?}"
            );
            let on_page = listed(&label, &page);
            assert!(
                on_page.len() <= page_size as usize && !on_page.is_empty(),
                "case {label:?}: a page must hold 1..={page_size} names, got {on_page:?}"
            );
            collected.extend(on_page);
            pages += 1;
            assert!(
                pages <= repositories.len() + 1,
                "case {label:?}: the walk must terminate, got {collected:?}"
            );
            match next_token(&page) {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            collected, repositories,
            "case {label:?}: the walk must cover the list exactly once, in order"
        );
    }

    // A token this service did not mint for THIS listing: a token from the same group's
    // OTHER restriction type, and one that is not a token at all.
    let other_listing = list_allowed_repositories(
        &router,
        domain,
        group,
        "PUBLISH",
        Some(1),
        None,
    )
    .await;
    let foreign = next_token(&other_listing).expect("the second list must span pages");
    for (label, token) in [
        ("a cursor from another restriction type", foreign),
        ("not a token at all", "deadbeef".to_string()),
    ] {
        let refused = list_allowed_repositories(
            &router,
            domain,
            group,
            "EXTERNAL_UPSTREAM",
            None,
            Some(&token),
        )
        .await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "ValidationException".to_string()),
            "case {label:?}: expected a 400 ValidationException, got {refused:?}"
        );
    }
}

/// Both operations address the same resources, so both answer 404 for a domain or a
/// pattern the caller does not hold, and 400 for a pattern no package group can have.
#[tokio::test]
async fn allowed_repository_operations_refuse_resources_the_caller_does_not_hold() {
    let router = emulator();
    let domain = "pg-allowed-missing";
    create_domain(&router, domain, None).await;
    let held = "/npm/*";
    create_package_group(&router, domain, held, None, None).await;

    // Rows differ along WHICH part of the address misses: the domain, the owner it is
    // pinned to, and the pattern.
    let not_found = [
        ("absent domain", "pg-absent".to_string(), held.to_string()),
        (
            "domain pinned to another account",
            format!("{domain}&domain-owner=999999999999"),
            held.to_string(),
        ),
        (
            "pattern the domain does not hold",
            domain.to_string(),
            "/maven/*".to_string(),
        ),
        (
            "exact pattern the domain does not hold",
            domain.to_string(),
            "/npm/@scope/package$".to_string(),
        ),
    ];

    for (label, domain_query, pattern) in not_found {
        let listed_answer = call(
            &router,
            "GET",
            &format!(
                "/v1/package-group-allowed-repositories?domain={domain_query}\
                 &package-group={pattern}&originRestrictionType=PUBLISH"
            ),
            None,
        )
        .await;
        assert_eq!(
            (listed_answer.status, listed_answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected 404 from the listing, got {listed_answer:?}"
        );

        let updated = call(
            &router,
            "PUT",
            &format!(
                "/v1/package-group-origin-configuration?domain={domain_query}\
                 &package-group={pattern}"
            ),
            Some(json!({ "restrictions": { "PUBLISH": ALLOW_SPECIFIC } })),
        )
        .await;
        assert_eq!(
            (updated.status, updated.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected 404 from the update, got {updated:?}"
        );
    }

    // A pattern that is not a path from the domain root is not an identifier any group can
    // have: a bad request at both operations, not a lookup that misses.
    for pattern in ["npm/*", "npm/pkg$"] {
        let listed_answer =
            list_allowed_repositories(&router, domain, pattern, "PUBLISH", None, None).await;
        assert_eq!(
            (listed_answer.status, listed_answer.code()),
            (400, "ValidationException".to_string()),
            "case {pattern:?}: expected 400 from the listing, got {listed_answer:?}"
        );
        let updated = update_origin_configuration(
            &router,
            domain,
            pattern,
            json!({ "restrictions": { "PUBLISH": ALLOW_SPECIFIC } }),
        )
        .await;
        assert_eq!(
            (updated.status, updated.code()),
            (400, "ValidationException".to_string()),
            "case {pattern:?}: expected 400 from the update, got {updated:?}"
        );
    }
}

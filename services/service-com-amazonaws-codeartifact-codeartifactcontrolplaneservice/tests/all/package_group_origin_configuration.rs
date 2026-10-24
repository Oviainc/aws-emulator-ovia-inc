// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for `UpdatePackageGroupOriginConfiguration` as a WRITE to a
//! package group's origin restrictions, and for what the hierarchy then resolves.
//!
//! `tests/all/package_group_allowed_repositories.rs` pins the allowed-repository list and
//! its listing; this suite pins the rest of the operation's contract, each rule asserted
//! over a TABLE of inputs rather than one example:
//!
//! 1. A request sets the `mode` of exactly the restriction types it names and leaves every
//!    other type of the same group where it was, and the `packageGroup` it answers with is
//!    the record it stored — `DescribePackageGroup` of the same pattern answers the same
//!    object, member for member.
//! 2. `effectiveMode` is the group's own `mode` unless that mode is `INHERIT`, in which
//!    case it is "the value of `mode` of the first parent group which does not have a value
//!    of `INHERIT`" (`PackageGroupOriginRestriction$effectiveMode`), with `inheritedFrom`
//!    naming that group — resolved from the patterns the domain holds at READ time, at any
//!    depth, and only for a group whose own mode inherits.
//! 3. `allowedRepositoryUpdates` reports what the stored list actually gained and lost, so
//!    a request that moves nothing reports nothing.
//! 4. A member outside a modelled enum, and a list entry that names no repository or no
//!    restriction type, are refused as invalid parameters and store nothing.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{create_domain, emulator, Answer};
use crate::package_group_allowed_repositories::{
    allow_specific_repositories, allowed_repository_entries, list_allowed_repositories, listed,
    restriction, update_origin_configuration, ALLOW_SPECIFIC, RESTRICTION_TYPES,
};
use crate::package_group_hierarchy::{create_package_group, describe_package_group, ROOT};

/// Every `PackageGroupOriginRestrictionMode` the model declares, in the model's own order.
///
/// Source: `smithy_query(query="shape", shape="PackageGroupOriginRestrictionMode")` →
/// values `["ALLOW", "ALLOW_SPECIFIC_REPOSITORIES", "BLOCK", "INHERIT"]`. The restriction
/// TYPE list and the one mode that carries a list have a single owner, the sibling suite
/// `package_group_allowed_repositories`.
const MODES: [&str; 4] = ["ALLOW", "ALLOW_SPECIFIC_REPOSITORIES", "BLOCK", "INHERIT"];

/// The mode the root group of every domain carries, which is what a group below it resolves
/// to while nothing in between names a mode of its own.
const ROOT_MODE: &str = "ALLOW";

/// The mode a created group carries for every restriction type.
const INHERIT: &str = "INHERIT";

/// The `inheritedFrom` pattern of a restriction, or `None` when it reports none.
fn inherited_from(label: &str, group: &Value, restriction_type: &str) -> Option<String> {
    restriction(label, group, restriction_type)
        .get("inheritedFrom")
        .and_then(|from| from.get("pattern"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Assert that one restriction of `group` reports exactly the resolution rule 2 owes it.
fn assert_restriction(
    label: &str,
    group: &Value,
    restriction_type: &str,
    expected_mode: &str,
    expected_effective_mode: &str,
    expected_inherited_from: Option<&str>,
    expected_repositories_count: Option<u64>,
) {
    let restriction = restriction(label, group, restriction_type);
    assert_eq!(
        restriction.get("mode").and_then(Value::as_str),
        Some(expected_mode),
        "case {label:?}/{restriction_type}: expected mode {expected_mode}, got {restriction:?}"
    );
    assert_eq!(
        restriction.get("effectiveMode").and_then(Value::as_str),
        Some(expected_effective_mode),
        "case {label:?}/{restriction_type}: expected effectiveMode {expected_effective_mode}, got {restriction:?}"
    );
    assert_eq!(
        inherited_from(label, group, restriction_type).as_deref(),
        expected_inherited_from,
        "case {label:?}/{restriction_type}: expected inheritedFrom {expected_inherited_from:?}, got {restriction:?}"
    );
    assert_eq!(
        restriction.get("repositoriesCount").and_then(Value::as_u64),
        expected_repositories_count,
        "case {label:?}/{restriction_type}: expected repositoriesCount {expected_repositories_count:?}, got {restriction:?}"
    );
}

/// RULE 1, over every `(restriction type, mode)` the model admits: one request moves the
/// types it names and nothing else, and answers with the record it stored.
#[tokio::test]
async fn a_mode_is_written_for_the_named_restriction_type_only() {
    let router = emulator();
    let domain = "pg-origin-modes";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "domain setup must succeed"
    );
    // A group with a parent, so `INHERIT` is admissible for it — rule 4 of the
    // allowed-repository suite covers the root group, which has nothing to inherit from.
    let group = "/npm/*";
    assert_eq!(
        create_package_group(&router, domain, group, None, None)
            .await
            .status,
        200,
        "group setup must succeed"
    );

    // What each restriction type of the group carries as the table walks it: every type
    // starts out inheriting, and only the type a row names moves.
    let mut stored: BTreeMap<&str, &str> = RESTRICTION_TYPES
        .iter()
        .map(|restriction_type| (*restriction_type, INHERIT))
        .collect();

    for mode in MODES {
        for restriction_type in RESTRICTION_TYPES {
            let label = format!("{restriction_type} -> {mode}");
            let updated = update_origin_configuration(
                &router,
                domain,
                group,
                json!({ "restrictions": { restriction_type: mode } }),
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {label:?}: expected 200 from the update, got {updated:?}"
            );
            stored.insert(restriction_type, mode);

            // RULE 1: the answer IS the stored record, so a read of the same pattern
            // cannot disagree with it.
            let described = describe_package_group(&router, domain, group).await;
            assert_eq!(
                updated.group(),
                described.group(),
                "case {label:?}: the update must answer with the record it stored, got {updated:?} and {described:?}"
            );

            // Every type reports the mode most recently written FOR IT, which is the
            // independence half of rule 1.
            for (other_type, expected_mode) in &stored {
                let expected_effective_mode = if *expected_mode == INHERIT {
                    ROOT_MODE
                } else {
                    expected_mode
                };
                assert_restriction(
                    &label,
                    described.group(),
                    other_type,
                    expected_mode,
                    expected_effective_mode,
                    (*expected_mode == INHERIT).then_some(ROOT),
                    // No row here writes a list, so a restriction that resolves to
                    // ALLOW_SPECIFIC_REPOSITORIES counts an empty one.
                    (expected_effective_mode == ALLOW_SPECIFIC).then_some(0),
                );
            }
        }
    }
}

/// RULE 2: a mode written on an ancestor resolves for every group below it, at any depth,
/// until a nearer ancestor names a mode of its own — including for a group created after
/// the write, since the resolution is derived from the patterns held and not stored per
/// group.
#[tokio::test]
async fn an_ancestors_mode_resolves_down_the_hierarchy() {
    let router = emulator();
    let domain = "pg-origin-inheritance";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "setup: the domain must be created"
    );

    // A chain of increasing depth under the root group, plus a sibling branch, so the rule
    // is asserted on a direct child, a grandchild, a great-grandchild on both pattern
    // terminators, and a group that does not descend from the one that was written.
    let written = "/npm/*";
    let middle = "/npm/@scope/*";
    let deepest = "/npm/@scope/package$";
    let other_branch = "/maven/*";
    for pattern in [written, middle, deepest, other_branch] {
        assert_eq!(
            create_package_group(&router, domain, pattern, None, None)
                .await
                .status,
            200,
            "setup: creating {pattern} must succeed"
        );
    }

    // ONE request that moves two restriction types of the written group, one of them to
    // the mode that carries a list, and fills that list in the same call.
    let repositories = ["repo-alpha", "repo-beta"];
    let updated = update_origin_configuration(
        &router,
        domain,
        written,
        json!({
            "restrictions": {
                "EXTERNAL_UPSTREAM": "BLOCK",
                "INTERNAL_UPSTREAM": ALLOW_SPECIFIC,
            },
            "addAllowedRepositories":
                allowed_repository_entries("INTERNAL_UPSTREAM", &repositories),
        }),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "the multi-restriction update must succeed, got {updated:?}"
    );
    assert_eq!(
        updated.body.get("allowedRepositoryUpdates"),
        Some(&json!({ "INTERNAL_UPSTREAM": { "ADDED": repositories } })),
        "expected the list the request filled reported added, got {updated:?}"
    );

    // The written group carries its own modes; the type it did not name still inherits,
    // from the root group.
    let described = describe_package_group(&router, domain, written).await;
    assert_restriction(
        written,
        described.group(),
        "EXTERNAL_UPSTREAM",
        "BLOCK",
        "BLOCK",
        None,
        None,
    );
    assert_restriction(
        written,
        described.group(),
        "INTERNAL_UPSTREAM",
        ALLOW_SPECIFIC,
        ALLOW_SPECIFIC,
        None,
        Some(repositories.len() as u64),
    );
    assert_restriction(
        written,
        described.group(),
        "PUBLISH",
        INHERIT,
        ROOT_MODE,
        Some(ROOT),
        None,
    );

    // Every DESCENDANT resolves the written modes, whatever its depth, and a group created
    // after the write resolves them too.
    let late_child = "/npm/@other/*";
    assert_eq!(
        create_package_group(&router, domain, late_child, None, None)
            .await
            .status,
        200,
        "setup: a group created after the write must be created"
    );

    for (label, pattern) in [
        ("direct child", middle),
        ("grandchild, exact terminator", deepest),
        ("child created after the write", late_child),
    ] {
        let described = describe_package_group(&router, domain, pattern).await;
        assert_restriction(
            label,
            described.group(),
            "EXTERNAL_UPSTREAM",
            INHERIT,
            "BLOCK",
            Some(written),
            None,
        );
        assert_restriction(
            label,
            described.group(),
            "INTERNAL_UPSTREAM",
            INHERIT,
            ALLOW_SPECIFIC,
            Some(written),
            // The count is the list of the group the effective mode came FROM: the
            // descendant holds no list of its own.
            Some(repositories.len() as u64),
        );
        assert_restriction(
            label,
            described.group(),
            "PUBLISH",
            INHERIT,
            ROOT_MODE,
            Some(ROOT),
            None,
        );
    }

    // A branch that does not descend from the written group is untouched by it.
    let sibling = describe_package_group(&router, domain, other_branch).await;
    for restriction_type in RESTRICTION_TYPES {
        assert_restriction(
            other_branch,
            sibling.group(),
            restriction_type,
            INHERIT,
            ROOT_MODE,
            Some(ROOT),
            None,
        );
    }

    // A NEARER ancestor naming a mode of its own takes over: the walk stops at the FIRST
    // non-inheriting parent, so the deepest group follows the middle group now and the
    // written group keeps what it had.
    let nearer = update_origin_configuration(
        &router,
        domain,
        middle,
        json!({ "restrictions": { "EXTERNAL_UPSTREAM": "ALLOW" } }),
    )
    .await;
    assert_eq!(
        nearer.status, 200,
        "the nearer ancestor's update must succeed, got {nearer:?}"
    );

    let described = describe_package_group(&router, domain, deepest).await;
    assert_restriction(
        "nearest ancestor wins",
        described.group(),
        "EXTERNAL_UPSTREAM",
        INHERIT,
        "ALLOW",
        Some(middle),
        None,
    );
    let described = describe_package_group(&router, domain, written).await;
    assert_restriction(
        "the written group is unaffected by its child",
        described.group(),
        "EXTERNAL_UPSTREAM",
        "BLOCK",
        "BLOCK",
        None,
        None,
    );
}

/// RULE 3: the report names what the stored list actually gained and lost, so a request
/// that re-sends what is already stored, or takes out what is not, reports nothing.
#[tokio::test]
async fn allowed_repository_updates_report_only_what_moved() {
    let router = emulator();
    let domain = "pg-origin-updates";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "setup: the domain must be created"
    );
    let group = "/npm/*";
    assert_eq!(
        create_package_group(&router, domain, group, None, None)
            .await
            .status,
        200,
        "setup: {group} must be created"
    );

    let restriction_type = "EXTERNAL_UPSTREAM";
    let seeded =
        allow_specific_repositories(&router, domain, group, restriction_type, &["alpha", "beta"])
            .await;
    assert_eq!(
        seeded.status, 200,
        "setup: the list must be written, got {seeded:?}"
    );

    // Rows differ along WHICH names a request names relative to the stored list (all
    // stored, none stored, a mix) and along which member names them.
    let rows: Vec<(&str, Value, Value, Vec<&str>)> = vec![
        (
            "re-adding every stored name moves nothing",
            json!({
                "addAllowedRepositories": allowed_repository_entries(restriction_type, &["alpha", "beta"]),
            }),
            json!({}),
            vec!["alpha", "beta"],
        ),
        (
            "a mix reports only the name the list gained",
            json!({
                "addAllowedRepositories": allowed_repository_entries(restriction_type, &["beta", "gamma"]),
            }),
            json!({ restriction_type: { "ADDED": ["gamma"] } }),
            vec!["alpha", "beta", "gamma"],
        ),
        (
            "removing a name the list never held moves nothing",
            json!({
                "removeAllowedRepositories": allowed_repository_entries(restriction_type, &["delta"]),
            }),
            json!({}),
            vec!["alpha", "beta", "gamma"],
        ),
        (
            "a mixed removal reports only the name the list lost",
            json!({
                "removeAllowedRepositories":
                    allowed_repository_entries(restriction_type, &["alpha", "delta"]),
            }),
            json!({ restriction_type: { "REMOVED": ["alpha"] } }),
            vec!["beta", "gamma"],
        ),
        (
            "a request naming no list at all moves nothing",
            json!({}),
            json!({}),
            vec!["beta", "gamma"],
        ),
    ];

    for (label, body, expected_updates, expected_list) in rows {
        let answer = update_origin_configuration(&router, domain, group, body).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected 200, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("allowedRepositoryUpdates"),
            Some(&expected_updates),
            "case {label:?}: expected {expected_updates:?}, got {answer:?}"
        );

        let listed_answer =
            list_allowed_repositories(&router, domain, group, restriction_type, None, None).await;
        assert_eq!(
            listed(label, &listed_answer),
            expected_list
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {label:?}: expected the stored list {expected_list:?}, got {listed_answer:?}"
        );
        let described = describe_package_group(&router, domain, group).await;
        assert_restriction(
            label,
            described.group(),
            restriction_type,
            ALLOW_SPECIFIC,
            ALLOW_SPECIFIC,
            None,
            Some(expected_list.len() as u64),
        );
    }

    // One request that both removes and adds the same name leaves it in the list: the
    // removal is applied first, so the request's addition is what stands.
    let both = update_origin_configuration(
        &router,
        domain,
        group,
        json!({
            "removeAllowedRepositories": allowed_repository_entries(restriction_type, &["beta"]),
            "addAllowedRepositories": allowed_repository_entries(restriction_type, &["beta"]),
        }),
    )
    .await;
    assert_eq!(
        both.status, 200,
        "a request that removes and adds one name must succeed, got {both:?}"
    );
    let listed_answer =
        list_allowed_repositories(&router, domain, group, restriction_type, None, None).await;
    assert_eq!(
        listed("removed and added in one request", &listed_answer),
        vec!["beta".to_string(), "gamma".to_string()],
        "the name the request added must be in the list, got {listed_answer:?}"
    );

    // Two restriction types in ONE request: each list is its own, and both are reported.
    let multi = update_origin_configuration(
        &router,
        domain,
        group,
        json!({
            "restrictions": { "PUBLISH": ALLOW_SPECIFIC },
            "addAllowedRepositories": json!([
                { "repositoryName": "shared", "originRestrictionType": restriction_type },
                { "repositoryName": "publish-only", "originRestrictionType": "PUBLISH" },
            ]),
        }),
    )
    .await;
    assert_eq!(
        multi.status, 200,
        "a two-type update must succeed, got {multi:?}"
    );
    assert_eq!(
        multi.body.get("allowedRepositoryUpdates"),
        Some(&json!({
            restriction_type: { "ADDED": ["shared"] },
            "PUBLISH": { "ADDED": ["publish-only"] },
        })),
        "expected both lists reported, got {multi:?}"
    );
    let publish_list =
        list_allowed_repositories(&router, domain, group, "PUBLISH", None, None).await;
    assert_eq!(
        listed("the second type's list", &publish_list),
        vec!["publish-only".to_string()],
        "each restriction type keeps its own list, got {publish_list:?}"
    );
}

/// RULE 4: a value outside a modelled enum, and a list entry that names no repository or no
/// restriction type, are refused — and the group is left exactly as it was.
#[tokio::test]
async fn origin_configuration_update_refuses_unmodelled_members() {
    let router = emulator();
    let domain = "pg-origin-bad-input";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "setup: the domain must be created"
    );
    let group = "/npm/*";
    assert_eq!(
        create_package_group(&router, domain, group, None, None)
            .await
            .status,
        200,
        "setup: {group} must be created"
    );

    // Rows differ along WHICH member carries the unusable value and what makes it
    // unusable: a key outside `PackageGroupOriginRestrictionType`, a value outside
    // `PackageGroupOriginRestrictionMode`, and a `PackageGroupAllowedRepository` missing
    // either of its two members. The fragment is the part of the refusal that attributes
    // the failure, which is what a client branches on.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a restriction type the model does not declare",
            json!({ "restrictions": { "NOT_A_TYPE": "ALLOW" } }),
            "NOT_A_TYPE",
        ),
        (
            "a mode the model does not declare",
            json!({ "restrictions": { "PUBLISH": "NOT_A_MODE" } }),
            "NOT_A_MODE",
        ),
        (
            "an added repository for a restriction type the model does not declare",
            json!({
                "addAllowedRepositories": json!([
                    { "repositoryName": "repo-one", "originRestrictionType": "NOT_A_TYPE" },
                ]),
            }),
            "NOT_A_TYPE",
        ),
        (
            "an added repository that names no restriction type",
            json!({
                "addAllowedRepositories": json!([{ "repositoryName": "repo-one" }]),
            }),
            "originRestrictionType",
        ),
        (
            "an added repository that names no repository",
            json!({
                "addAllowedRepositories": json!([{ "originRestrictionType": "PUBLISH" }]),
            }),
            "repositoryName",
        ),
        (
            "a removed repository that names no restriction type",
            json!({
                "removeAllowedRepositories": json!([{ "repositoryName": "repo-one" }]),
            }),
            "originRestrictionType",
        ),
    ];

    for (label, body, fragment) in rows {
        let refused = update_origin_configuration(&router, domain, group, body).await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "ValidationException".to_string()),
            "case {label:?}: expected a 400 ValidationException, got {refused:?}"
        );
        let message = refused.message();
        assert!(
            message.contains(fragment),
            "case {label:?}: expected the refusal to name {fragment:?}, got {message:?}"
        );

        assert_nothing_written(label, &router, domain, group).await;
    }
}

/// Assert that `group` still carries exactly what a freshly created group carries — the
/// state every refused request above must leave behind.
async fn assert_nothing_written(
    label: &str,
    router: &MultiServiceRouter,
    domain: &str,
    group: &str,
) {
    let described: Answer = describe_package_group(router, domain, group).await;
    for restriction_type in RESTRICTION_TYPES {
        assert_restriction(
            label,
            described.group(),
            restriction_type,
            INHERIT,
            ROOT_MODE,
            Some(ROOT),
            None,
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for `GetAssociatedPackageGroup`: which package group a
//! package resolves to, and how strong that association is.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted is the wire behaviour a
//! CodeArtifact client sees — status, error code and body. The request plumbing is the
//! shared `harness`, and the `CreatePackageGroup`/`DescribePackageGroup` helpers are the
//! ones `package_group_hierarchy.rs` already exposes; what is new here is the resolution
//! itself. That suite owns the hierarchy rules (parent, inherited restrictions,
//! create/describe refusals); this one owns the association.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one example:
//!
//! 1. A package resolves to the MOST SPECIFIC package group in its domain whose pattern
//!    matches the package's `/<format>[/<namespace>]/<name>` path — the match being on
//!    the longest literal, and an exact (`$`) pattern beating a relative (`*`, `~`) one.
//! 2. `associationType` is `STRONG` where the matched pattern covers whole components of
//!    that path (the group names the package's own format, namespace and name) and `WEAK`
//!    where it matched only part of a component — a relative match on a namespace or
//!    name prefix.
//! 3. Every domain holds the root group `/*`, which matches every package of every
//!    format, so the association resolves even for a package no group names — and for a
//!    package no repository holds, which this operation does not require.
//! 4. The association is DERIVED from the groups the domain currently holds: creating a
//!    more specific group moves a package's association onto it.
//! 5. Resolving an association is a READ: it stores no group, so a pattern nobody
//!    created is still unknown to `DescribePackageGroup` afterwards.
//! 6. A package of a format whose versions carry a namespace cannot be resolved without
//!    one (400), and an unknown domain is 404.

use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_domain, emulator, package_group_arn, Answer};
use crate::package_group_hierarchy::{create_package_group, describe_package_group, ROOT};

/// `GetAssociatedPackageGroup`: every input is `httpQuery`-bound, and `namespace` is sent
/// only when the caller has one.
async fn get_associated_package_group(
    router: &MultiServiceRouter,
    domain: &str,
    format: &str,
    namespace: Option<&str>,
    package: &str,
) -> Answer {
    let mut uri = format!(
        "/v1/get-associated-package-group?domain={domain}&format={format}&package={package}"
    );
    if let Some(namespace) = namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    call(router, "GET", &uri, None).await
}

/// The ARN the emulator mints for a package group, as every answer must report it: the
/// pattern as a path, each segment percent-encoded
/// (`crate::harness::package_group_arn`, pinned by
/// `tests/all/package_group_arn_encoding.rs`).
fn group_arn(domain: &str, pattern: &str) -> String {
    package_group_arn(domain, pattern)
}

/// One package to resolve, and the group it must resolve to.
#[derive(Debug)]
struct AssociationCase {
    format: &'static str,
    namespace: Option<&'static str>,
    package: &'static str,
    expected_pattern: &'static str,
    expected_type: &'static str,
}

/// The groups every association case below is resolved against, beside the root group
/// the domain is created with. The patterns differ along what the rule generalises over:
/// the format segment, the presence of a namespace segment, nesting depth, and the
/// terminator that decides how the pattern matches (`*` and `~` relative, `$` exact).
const GROUPS: &[&str] = &[
    "/npm/*",
    "/npm/@scope/*",
    "/npm/@scope/exact-pkg$",
    "/maven/com.example~",
    "/generic/*",
];

#[tokio::test]
async fn association_picks_the_most_specific_matching_pattern() {
    let router = emulator();
    let domain = "assoc-domain";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "setup: the domain must be created before its package groups"
    );
    for pattern in GROUPS {
        let created = create_package_group(&router, domain, pattern, None, None).await;
        assert_eq!(
            created.status, 200,
            "setup: group {pattern} must be created, got {created:?}"
        );
    }

    let cases = [
        // No npm group names this package, so the format-wide group matches — on the
        // whole `/npm/` component, so the match is exact.
        AssociationCase {
            format: "npm",
            namespace: None,
            package: "left-pad",
            expected_pattern: "/npm/*",
            expected_type: "STRONG",
        },
        // The namespace is part of the path, so the namespaced group is more specific
        // than the format-wide one.
        AssociationCase {
            format: "npm",
            namespace: Some("@scope"),
            package: "other-pkg",
            expected_pattern: "/npm/@scope/*",
            expected_type: "STRONG",
        },
        // An exact pattern matches this one package and beats both relative patterns
        // above it.
        AssociationCase {
            format: "npm",
            namespace: Some("@scope"),
            package: "exact-pkg",
            expected_pattern: "/npm/@scope/exact-pkg$",
            expected_type: "STRONG",
        },
        // A `~` pattern matches every path carrying its literal as a prefix, so a
        // SUB-namespace still matches — but only part of that component, so the
        // association is relative.
        AssociationCase {
            format: "maven",
            namespace: Some("com.example.util"),
            package: "core",
            expected_pattern: "/maven/com.example~",
            expected_type: "WEAK",
        },
        // The same group matched on the namespace it actually names: a whole component,
        // so this one is exact.
        AssociationCase {
            format: "maven",
            namespace: Some("com.example"),
            package: "core",
            expected_pattern: "/maven/com.example~",
            expected_type: "STRONG",
        },
        // The namespace of a generic package is part of its path too.
        AssociationCase {
            format: "generic",
            namespace: Some("reports"),
            package: "quarterly.tar.gz",
            expected_pattern: "/generic/*",
            expected_type: "STRONG",
        },
        // No group names these formats at all: the root group matches every package.
        AssociationCase {
            format: "pypi",
            namespace: None,
            package: "requests",
            expected_pattern: ROOT,
            expected_type: "STRONG",
        },
        AssociationCase {
            format: "cargo",
            namespace: None,
            package: "serde",
            expected_pattern: ROOT,
            expected_type: "STRONG",
        },
        // A namespace that no group's literal is a prefix of falls back to the format
        // group rather than to a sibling namespace's group.
        AssociationCase {
            format: "npm",
            namespace: Some("@other"),
            package: "exact-pkg",
            expected_pattern: "/npm/*",
            expected_type: "STRONG",
        },
    ];

    for case in &cases {
        let answer =
            get_associated_package_group(&router, domain, case.format, case.namespace, case.package)
                .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        assert_eq!(
            answer.group()["pattern"].as_str(),
            Some(case.expected_pattern),
            "case {case:?}: expected pattern {}, got {:?}",
            case.expected_pattern,
            answer.group()
        );
        assert_eq!(
            answer.body["associationType"].as_str(),
            Some(case.expected_type),
            "case {case:?}: expected associationType {}, got {:?}",
            case.expected_type,
            answer.body["associationType"]
        );
        assert_eq!(
            answer.group()["arn"].as_str(),
            Some(group_arn(domain, case.expected_pattern).as_str()),
            "case {case:?}: expected the matched group's ARN, got {:?}",
            answer.group()
        );

        // The association reports the group as `DescribePackageGroup` reports it, so the
        // two readers of one record cannot disagree.
        let described = describe_package_group(&router, domain, case.expected_pattern).await;
        assert_eq!(
            described.group(),
            answer.group(),
            "case {case:?}: expected the described group {:?}, got {:?}",
            described.group(),
            answer.group()
        );
    }
}

#[tokio::test]
async fn a_more_specific_group_created_later_moves_the_association() {
    let router = emulator();
    let domain = "moving-domain";
    create_domain(&router, domain, None).await;

    // Each step narrows the pattern that matches the SAME package, so the association
    // has to follow the groups the domain holds rather than anything stored against the
    // package — which does not exist in any repository here at all.
    let steps = [
        (None, ROOT),
        (Some("/npm/*"), "/npm/*"),
        (Some("/npm/@acme/*"), "/npm/@acme/*"),
        (Some("/npm/@acme/widget$"), "/npm/@acme/widget$"),
    ];

    for (create, expected) in steps {
        if let Some(pattern) = create {
            let created = create_package_group(&router, domain, pattern, None, None).await;
            assert_eq!(
                created.status, 200,
                "case {pattern}: expected 200, got {created:?}"
            );
        }
        let answer =
            get_associated_package_group(&router, domain, "npm", Some("@acme"), "widget").await;
        assert_eq!(
            answer.group()["pattern"].as_str(),
            Some(expected),
            "case created={create:?}: expected pattern {expected}, got {:?}",
            answer.group()
        );
        assert_eq!(
            answer.body["associationType"].as_str(),
            Some("STRONG"),
            "case created={create:?}: every pattern here covers whole path components, so the association is exact: {:?}",
            answer.body["associationType"]
        );
    }

    // A group that names only a PREFIX of the namespace is matched relatively, even
    // though it is the most specific group the package has.
    let created = create_package_group(&router, domain, "/npm/@acme-tools~", None, None).await;
    assert_eq!(
        created.status, 200,
        "case /npm/@acme-tools~: expected 200, got {created:?}"
    );
    let relative =
        get_associated_package_group(&router, domain, "npm", Some("@acme-tools-ui"), "gadget").await;
    assert_eq!(
        relative.group()["pattern"].as_str(),
        Some("/npm/@acme-tools~"),
        "case sub-namespace: expected the prefix group, got {:?}",
        relative.group()
    );
    assert_eq!(
        relative.body["associationType"].as_str(),
        Some("WEAK"),
        "case sub-namespace: expected WEAK, got {:?}",
        relative.body["associationType"]
    );
}

#[tokio::test]
async fn resolving_an_association_stores_nothing() {
    let router = emulator();
    let domain = "read-only-domain";
    create_domain(&router, domain, None).await;
    create_package_group(&router, domain, "/npm/*", None, None).await;

    let before = describe_package_group(&router, domain, "/npm/*").await;

    // Resolve several packages, including ones no group names.
    for (format, namespace, package) in [
        ("npm", None, "left-pad"),
        ("npm", Some("@scope"), "widget"),
        ("pypi", None, "requests"),
        ("maven", Some("com.example"), "core"),
    ] {
        let answer = get_associated_package_group(&router, domain, format, namespace, package).await;
        assert_eq!(
            answer.status, 200,
            "case {format}/{namespace:?}/{package}: expected 200, got {answer:?}"
        );
    }

    let after = describe_package_group(&router, domain, "/npm/*").await;
    assert_eq!(
        after.group(),
        before.group(),
        "case /npm/*: expected the read to leave the group unchanged: {:?} vs {:?}",
        before.group(),
        after.group()
    );

    // No group was materialised for any resolved path: each is still unknown.
    for pattern in [
        "/npm/left-pad$",
        "/npm/@scope/widget$",
        "/pypi/*",
        "/maven/com.example/core$",
    ] {
        let described = describe_package_group(&router, domain, pattern).await;
        assert_eq!(
            described.status, 404,
            "case {pattern}: expected 404 for a group nobody created, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_package_that_cannot_be_addressed_is_refused_as_a_bad_request() {
    let router = emulator();
    let domain = "refusal-domain";
    create_domain(&router, domain, None).await;

    // The namespace of a Maven package is its groupId, of a Swift package its scope and
    // of a generic package its namespace, so a package of those formats cannot be
    // addressed without one (`GetAssociatedPackageGroupRequest$namespace`).
    for format in ["generic", "maven", "swift"] {
        let answer = get_associated_package_group(&router, domain, format, None, "a-package").await;
        assert_eq!(
            (answer.status, answer.code()),
            (400, "ValidationException".to_string()),
            "case {format} with no namespace: expected a 400 ValidationException, got {answer:?}"
        );
    }

    // Every other format's versions carry no namespace component, so they resolve
    // without one.
    for format in ["cargo", "npm", "nuget", "pypi", "ruby"] {
        let answer = get_associated_package_group(&router, domain, format, None, "a-package").await;
        assert_eq!(
            answer.status, 200,
            "case {format} with no namespace: expected 200, got {answer:?}"
        );
        assert_eq!(
            answer.group()["pattern"].as_str(),
            Some(ROOT),
            "case {format}: expected the root group, got {:?}",
            answer.group()
        );
    }

    // A domain nobody created holds no groups to resolve against.
    let unknown = get_associated_package_group(&router, "no-such-domain", "npm", None, "left-pad")
        .await;
    assert_eq!(
        (unknown.status, unknown.code()),
        (404, "ResourceNotFoundException".to_string()),
        "case unknown domain: expected a 404 ResourceNotFoundException, got {unknown:?}"
    );
    assert_eq!(
        unknown.body.get("packageGroup"),
        None,
        "case unknown domain: a refusal reports no group, got {:?}",
        unknown.body
    );
}

/// Both `associationType` values the table above expects are values the model declares,
/// so a model change shows up here rather than as a table that asserts a value the wire
/// cannot carry.
///
/// Source: `smithy_query(query="shape", shape="PackageGroupAssociationType")` → values
/// `["STRONG", "WEAK"]`, re-exported as the generated `*_VALUES` constant.
#[test]
fn the_expected_association_types_are_the_modelled_values() {
    use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::ops::types::PACKAGE_GROUP_ASSOCIATION_TYPE_VALUES;

    for value in ["STRONG", "WEAK"] {
        assert!(
            PACKAGE_GROUP_ASSOCIATION_TYPE_VALUES.contains(&value),
            "case {value}: not a PackageGroupAssociationType value {PACKAGE_GROUP_ASSOCIATION_TYPE_VALUES:?}"
        );
    }
    assert_eq!(
        PACKAGE_GROUP_ASSOCIATION_TYPE_VALUES.len(),
        2,
        "the model declares more association types than this suite covers: {PACKAGE_GROUP_ASSOCIATION_TYPE_VALUES:?}"
    );
}

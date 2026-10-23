// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for CodeArtifact's package groups.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code and body — not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of patterns rather than one recorded
//! example:
//!
//! 1. A package group's `parent` is the MOST SPECIFIC group the domain holds whose
//!    pattern covers a strict prefix of the path the group's pattern covers. It is
//!    derived from the patterns held, so creating an intermediate group reparents the
//!    groups below it, with no second call.
//! 2. A created group carries `mode: INHERIT` for every
//!    `PackageGroupOriginRestrictionType`, and its `effectiveMode` is "the value of
//!    `mode` of the first parent group which does not have a value of `INHERIT`"
//!    (`PackageGroupOriginRestriction$effectiveMode`), with `inheritedFrom` naming that
//!    group — which is the root group `/*` every domain is created with, allowing
//!    publishing and both upstream kinds.
//! 3. A create answers with the record it stored: a `DescribePackageGroup` of the same
//!    pattern answers with the same `packageGroup` object, member for member.
//! 4. The pattern IS the group's identifier within its domain: a repeat is
//!    `ConflictException` (409), a pattern the domain does not hold is
//!    `ResourceNotFoundException` (404), and a domain the caller does not hold is 404
//!    for both operations.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_domain, emulator, Answer, ACCOUNT, REGION};

/// The pattern of the root package group, which every domain holds from creation.
pub(crate) const ROOT: &str = "/*";

/// Every `PackageGroupOriginRestrictionType` the model declares — a group's
/// `originConfiguration.restrictions` must carry one entry per type.
///
/// Source: `smithy_query(query="shape", shape="PackageGroupOriginRestrictionType")` →
/// values `["EXTERNAL_UPSTREAM", "INTERNAL_UPSTREAM", "PUBLISH"]`.
const RESTRICTION_TYPES: [&str; 3] = ["EXTERNAL_UPSTREAM", "INTERNAL_UPSTREAM", "PUBLISH"];

/// `CreatePackageGroup`: the pattern travels in the BODY (`packageGroup` carries no
/// `httpQuery` binding), the domain in the query string.
pub(crate) async fn create_package_group(
    router: &MultiServiceRouter,
    domain: &str,
    pattern: &str,
    contact_info: Option<&str>,
    description: Option<&str>,
) -> Answer {
    let mut body = json!({ "packageGroup": pattern });
    if let Some(contact_info) = contact_info {
        body["contactInfo"] = json!(contact_info);
    }
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    call(
        router,
        "POST",
        &format!("/v1/package-group?domain={domain}"),
        Some(body),
    )
    .await
}

/// `DescribePackageGroup`: both the domain and the pattern are `httpQuery`-bound.
pub(crate) async fn describe_package_group(
    router: &MultiServiceRouter,
    domain: &str,
    pattern: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/package-group?domain={domain}&package-group={pattern}"),
        None,
    )
    .await
}

/// The `pattern` of a group's `parent`, or `None` when it reports no parent.
fn parent_pattern(group: &Value) -> Option<&str> {
    group
        .get("parent")
        .and_then(|parent| parent.get("pattern"))
        .and_then(Value::as_str)
}

/// One restriction of a group's `originConfiguration`.
fn restriction<'a>(label: &str, group: &'a Value, restriction_type: &str) -> &'a Value {
    group
        .get("originConfiguration")
        .and_then(|configuration| configuration.get("restrictions"))
        .and_then(|restrictions| restrictions.get(restriction_type))
        .unwrap_or_else(|| panic!("case {label:?}: no {restriction_type} restriction in {group:?}"))
}

/// The ARN a pattern gets: `package-group/<domain>` with the pattern — itself a path from
/// the domain root — appended.
fn expected_arn(domain: &str, pattern: &str) -> String {
    format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/{domain}{pattern}")
}

/// Assert that every origin restriction of `group` inherits, and resolves to the root
/// group's `ALLOW` — the state of every group this suite creates, since nothing here
/// moves a restriction off `INHERIT`.
fn assert_inherits_allow_from_root(label: &str, group: &Value) {
    for restriction_type in RESTRICTION_TYPES {
        let restriction = restriction(label, group, restriction_type);
        assert_eq!(
            restriction.get("mode").and_then(Value::as_str),
            Some("INHERIT"),
            "case {label:?}/{restriction_type}: expected mode INHERIT, got {restriction:?}"
        );
        assert_eq!(
            restriction.get("effectiveMode").and_then(Value::as_str),
            Some("ALLOW"),
            "case {label:?}/{restriction_type}: expected effectiveMode ALLOW, got {restriction:?}"
        );
        assert_eq!(
            restriction
                .get("inheritedFrom")
                .and_then(|from| from.get("pattern"))
                .and_then(Value::as_str),
            Some(ROOT),
            "case {label:?}/{restriction_type}: expected inheritedFrom {ROOT}, got {restriction:?}"
        );
        assert!(
            restriction.get("repositoriesCount").is_none(),
            "case {label:?}/{restriction_type}: repositoriesCount belongs to an allowed-repository list, got {restriction:?}"
        );
    }
}

/// One package group to create.
struct GroupCase {
    label: &'static str,
    pattern: String,
    /// The parent this group must report once EVERY case in the table exists, written
    /// out by hand from the pattern paths rather than computed, so the expectation does
    /// not restate the implementation.
    parent_when_complete: &'static str,
    contact_info: Option<String>,
    description: Option<String>,
}

/// The hierarchy table. Rows differ along what rule 1 generalises over: the DEPTH of the
/// pattern (1, 2 and 3 path segments below the root), the package format segment, the
/// terminator (`/*` prefix vs `$` exact), the character classes a namespace segment
/// admits (`@scope`, a dotted Maven group id), and the modelled maximum pattern length.
///
/// `PackageGroupPattern` is len 2..520 pattern `^[^\p{C}\p{IsWhitespace}]+$` and
/// `Description` is len 0..1000 (`smithy_query(query="operation",
/// operation="CreatePackageGroup")`), so the longest rows below sit exactly on those
/// bounds.
fn hierarchy_cases() -> Vec<GroupCase> {
    // A 520-character pattern — the modelled maximum: "/npm/" + 513 name characters +
    // "/*". It sits under `/npm/*` like any other npm namespace.
    let longest_pattern = format!("/npm/{}/*", "a".repeat(513));
    assert_eq!(
        longest_pattern.chars().count(),
        520,
        "the longest pattern row must be exactly the modelled maximum"
    );

    vec![
        GroupCase {
            label: "one segment: a package format",
            pattern: "/npm/*".to_string(),
            parent_when_complete: ROOT,
            contact_info: Some("npm-admins@example.com".to_string()),
            description: Some("every npm package".to_string()),
        },
        GroupCase {
            label: "two segments: a scoped npm namespace",
            pattern: "/npm/@scope/*".to_string(),
            parent_when_complete: "/npm/*",
            contact_info: None,
            description: None,
        },
        GroupCase {
            label: "three segments, exact terminator: one npm package",
            pattern: "/npm/@scope/package$".to_string(),
            parent_when_complete: "/npm/@scope/*",
            // Both members are modelled len 0.., so the empty string is a legal value
            // and must come back as one rather than as an omitted member.
            contact_info: Some(String::new()),
            description: Some(String::new()),
        },
        GroupCase {
            label: "sibling format with a dotted maven namespace",
            pattern: "/maven/com.example.library/*".to_string(),
            parent_when_complete: "/maven/*",
            contact_info: None,
            description: Some("d".repeat(1000)),
        },
        GroupCase {
            label: "sibling format, created after its own child",
            pattern: "/maven/*".to_string(),
            parent_when_complete: ROOT,
            contact_info: None,
            description: None,
        },
        GroupCase {
            label: "exact terminator directly under a format",
            pattern: "/pypi/requests$".to_string(),
            parent_when_complete: "/pypi/*",
            contact_info: None,
            description: None,
        },
        GroupCase {
            label: "format whose only child is an exact pattern",
            pattern: "/pypi/*".to_string(),
            parent_when_complete: ROOT,
            contact_info: None,
            description: None,
        },
        GroupCase {
            label: "modelled maximum pattern length",
            pattern: longest_pattern,
            parent_when_complete: "/npm/*",
            contact_info: None,
            description: None,
        },
    ]
}

/// RULES 1, 2 and 3 over the whole table: every group round-trips through stored state,
/// reports the parent its pattern places it under — whatever ORDER the groups were
/// created in — and inherits every origin restriction from the root group.
#[tokio::test]
async fn created_package_groups_round_trip_and_report_their_place_in_the_hierarchy() {
    let router = emulator();
    let domain = "pg-hierarchy";
    let created_domain = create_domain(&router, domain, None).await;
    assert_eq!(
        created_domain.status, 200,
        "domain setup: expected 200, got {created_domain:?}"
    );

    let cases = hierarchy_cases();

    // Deliberately NOT top-down: several rows are created before the group that ends up
    // being their parent exists (`/maven/com.example.library/*` before `/maven/*`,
    // `/pypi/requests$` before `/pypi/*`), so rule 1 is tested on patterns whose parent
    // changed after they were stored.
    for case in &cases {
        let label = case.label;
        let created = create_package_group(
            &router,
            domain,
            &case.pattern,
            case.contact_info.as_deref(),
            case.description.as_deref(),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected 200, got {created:?}"
        );

        let group = created.group();
        assert_eq!(
            group.get("pattern").and_then(Value::as_str),
            Some(case.pattern.as_str()),
            "case {label:?}: expected the requested pattern, got {group:?}"
        );
        assert_eq!(
            group.get("domainName").and_then(Value::as_str),
            Some(domain),
            "case {label:?}: expected domainName {domain}, got {group:?}"
        );
        assert_eq!(
            group.get("domainOwner").and_then(Value::as_str),
            Some(ACCOUNT),
            "case {label:?}: expected domainOwner {ACCOUNT}, got {group:?}"
        );
        assert_eq!(
            group.get("arn").and_then(Value::as_str),
            Some(expected_arn(domain, &case.pattern).as_str()),
            "case {label:?}: expected the package-group ARN, got {group:?}"
        );
        assert_eq!(
            group.get("contactInfo").and_then(Value::as_str),
            case.contact_info.as_deref(),
            "case {label:?}: expected the contactInfo sent, got {group:?}"
        );
        assert_eq!(
            group.get("description").and_then(Value::as_str),
            case.description.as_deref(),
            "case {label:?}: expected the description sent, got {group:?}"
        );
        assert!(
            group
                .get("createdTime")
                .and_then(Value::as_f64)
                .is_some_and(|epoch_seconds| epoch_seconds > 0.0),
            "case {label:?}: expected an epoch-seconds createdTime, got {group:?}"
        );
        assert_inherits_allow_from_root(label, group);

        // RULE 3, read back IMMEDIATELY: the create answer is the stored record, so the
        // read of it — taken before any later row changes the hierarchy — is the same
        // object.
        let described = describe_package_group(&router, domain, &case.pattern).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: expected 200 from the read, got {described:?}"
        );
        assert_eq!(
            described.group(),
            created.group(),
            "case {label:?}: the read must answer with the record the create stored"
        );
    }

    // With every row created, each group's parent is fixed by the PATTERNS held, not by
    // the order they arrived in.
    for case in &cases {
        let label = case.label;
        let described = describe_package_group(&router, domain, &case.pattern).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: expected 200 from the read, got {described:?}"
        );
        let group = described.group();

        // RULE 1.
        assert_eq!(
            parent_pattern(group),
            Some(case.parent_when_complete),
            "case {label:?}: expected parent {:?}, got {:?}",
            case.parent_when_complete,
            parent_pattern(group)
        );
        assert_eq!(
            group
                .get("parent")
                .and_then(|parent| parent.get("arn"))
                .and_then(Value::as_str),
            Some(expected_arn(domain, case.parent_when_complete).as_str()),
            "case {label:?}: expected the parent's own ARN, got {group:?}"
        );

        // RULE 2: the effective mode is still the root group's ALLOW even where the
        // direct parent is an intermediate group that inherits too.
        assert_inherits_allow_from_root(label, group);
    }
}

/// One leaf pattern plus the groups inserted above it afterwards.
struct ReparentCase {
    label: &'static str,
    leaf: &'static str,
    /// Each pattern to create above the leaf, in order, paired with the parent the leaf
    /// must report once that pattern exists.
    insertions: Vec<(&'static str, &'static str)>,
}

/// RULE 1, isolated: the parent is derived, so inserting a group between a leaf and the
/// root reparents the leaf — and only the MOST SPECIFIC insertion wins.
#[tokio::test]
async fn inserting_a_group_above_a_leaf_reparents_it() {
    let router = emulator();
    let domain = "pg-reparent";
    create_domain(&router, domain, None).await;

    // Rows differ along the leaf's terminator, how many levels the insertion adds, and
    // the ORDER of the insertions — shallow-then-deep against deep-then-shallow.
    let cases = [
        ReparentCase {
            label: "one level inserted above a wildcard leaf",
            leaf: "/npm/@scope/*",
            insertions: vec![("/npm/*", "/npm/*")],
        },
        ReparentCase {
            label: "one level inserted above an exact leaf",
            leaf: "/maven/com.example/artifact$",
            insertions: vec![("/maven/com.example/*", "/maven/com.example/*")],
        },
        ReparentCase {
            label: "shallow then deep: the deeper insertion takes the leaf",
            leaf: "/generic/team/service$",
            insertions: vec![
                ("/generic/*", "/generic/*"),
                ("/generic/team/*", "/generic/team/*"),
            ],
        },
        ReparentCase {
            label: "deep then shallow: the shallower insertion does not steal the leaf",
            leaf: "/ruby/group/gem$",
            insertions: vec![
                ("/ruby/group/*", "/ruby/group/*"),
                ("/ruby/*", "/ruby/group/*"),
            ],
        },
    ];

    for case in cases {
        let label = case.label;
        let created_leaf = create_package_group(&router, domain, case.leaf, None, None).await;
        assert_eq!(
            created_leaf.status, 200,
            "case {label:?}: expected 200, got {created_leaf:?}"
        );
        assert_eq!(
            parent_pattern(created_leaf.group()),
            Some(ROOT),
            "case {label:?}: with nothing in between, the root is the parent, got {:?}",
            created_leaf.group()
        );

        for (inserted, expected_parent) in case.insertions {
            let created = create_package_group(&router, domain, inserted, None, None).await;
            assert_eq!(
                created.status, 200,
                "case {label:?}: expected 200 creating {inserted}, got {created:?}"
            );

            let reread = describe_package_group(&router, domain, case.leaf).await;
            assert_eq!(
                parent_pattern(reread.group()),
                Some(expected_parent),
                "case {label:?}: after creating {inserted}, expected the leaf's parent to be {expected_parent:?}, got {:?}",
                reread.group()
            );
            // The chain now runs through a group that inherits, and the mode still comes
            // from the first ancestor that does not — the root.
            assert_inherits_allow_from_root(label, reread.group());
        }
    }
}

/// RULE 2, at the top of the chain: the root group exists in a domain nobody has created
/// a group in, has no parent, and allows every restriction type outright.
#[tokio::test]
async fn every_domain_holds_a_root_package_group_that_allows_everything() {
    let router = emulator();

    // Rows differ along the domain name, at the extremes the model allows (`DomainName`
    // is len 2..50, pattern `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$`).
    let domains = [
        "ab".to_string(),
        "pg-root-defaults".to_string(),
        format!("d{}e", "x".repeat(48)),
    ];

    for domain in domains {
        let created = create_domain(&router, &domain, None).await;
        assert_eq!(
            created.status, 200,
            "case {domain:?}: domain setup expected 200, got {created:?}"
        );

        let root = describe_package_group(&router, &domain, ROOT).await;
        assert_eq!(
            root.status, 200,
            "case {domain:?}: the root group must exist without being created, got {root:?}"
        );
        let group = root.group();
        assert_eq!(
            group.get("pattern").and_then(Value::as_str),
            Some(ROOT),
            "case {domain:?}: expected pattern {ROOT}, got {group:?}"
        );
        assert_eq!(
            group.get("arn").and_then(Value::as_str),
            Some(expected_arn(&domain, ROOT).as_str()),
            "case {domain:?}: expected the root group's ARN, got {group:?}"
        );
        assert!(
            group.get("parent").is_none(),
            "case {domain:?}: the root group has nothing above it, got {group:?}"
        );
        for restriction_type in RESTRICTION_TYPES {
            let restriction = restriction(&domain, group, restriction_type);
            assert_eq!(
                restriction.get("mode").and_then(Value::as_str),
                Some("ALLOW"),
                "case {domain:?}/{restriction_type}: expected mode ALLOW, got {restriction:?}"
            );
            assert_eq!(
                restriction.get("effectiveMode").and_then(Value::as_str),
                Some("ALLOW"),
                "case {domain:?}/{restriction_type}: expected effectiveMode ALLOW, got {restriction:?}"
            );
            assert!(
                restriction.get("inheritedFrom").is_none(),
                "case {domain:?}/{restriction_type}: a mode that does not inherit is inherited from nothing, got {restriction:?}"
            );
        }
    }
}

/// RULE 4: the pattern is the identifier. A repeat conflicts, an absent pattern and an
/// absent (or differently owned) domain are not found, and a pattern that is not a path
/// from the domain root is refused.
#[tokio::test]
async fn package_group_identity_refusals() {
    let router = emulator();
    let domain = "pg-refusals";
    create_domain(&router, domain, None).await;
    let held = "/npm/*";
    assert_eq!(
        create_package_group(&router, domain, held, None, None)
            .await
            .status,
        200,
        "setup: creating {held} must succeed"
    );

    // A second create of a pattern already held — including the root group, which the
    // domain holds without anyone creating it.
    for pattern in [held, ROOT] {
        let repeat = create_package_group(&router, domain, pattern, None, None).await;
        assert_eq!(
            (repeat.status, repeat.code()),
            (409, "ConflictException".to_string()),
            "case {pattern:?}: expected 409 ConflictException, got {repeat:?}"
        );
        assert!(
            repeat.message().contains(pattern),
            "case {pattern:?}: the refusal must name the pattern, got {:?}",
            repeat.message()
        );
    }

    // A pattern the domain does not hold. Rows differ in depth and terminator, and one is
    // a pattern that is a PREFIX of a held one rather than unrelated.
    for pattern in ["/npm$", "/maven/*", "/npm/@scope/*", "/npm/@scope/package$"] {
        let missing = describe_package_group(&router, domain, pattern).await;
        assert_eq!(
            (missing.status, missing.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {pattern:?}: expected 404 ResourceNotFoundException, got {missing:?}"
        );
    }

    // A domain the caller does not hold: absent, and pinned to another account.
    for (label, uri_domain, owner) in [
        ("absent domain", "pg-absent", None),
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
        let created = call(
            &router,
            "POST",
            &format!("/v1/package-group?{query}"),
            Some(json!({ "packageGroup": "/npm/@scope/*" })),
        )
        .await;
        assert_eq!(
            (created.status, created.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected 404 from the create, got {created:?}"
        );
        let described = call(
            &router,
            "GET",
            &format!("/v1/package-group?{query}&package-group={held}"),
            None,
        )
        .await;
        assert_eq!(
            (described.status, described.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected 404 from the read, got {described:?}"
        );
    }

    // A pattern that is not a path from the domain root, and one outside the modelled
    // length bounds (`PackageGroupPattern` is len 2..520) — 400 `ValidationException`
    // either way, and nothing is stored. `refused_by_handler` marks the rows this
    // operation refuses itself; the others never reach it, because the modelled
    // constraint is checked at the boundary first.
    for (label, pattern, refused_by_handler) in [
        ("no leading slash", "npm/*".to_string(), true),
        ("relative exact pattern", "npm/pkg$".to_string(), true),
        ("bare wildcard, one under the modelled minimum", "*".to_string(), false),
        (
            "one character past the modelled maximum",
            format!("/npm/{}/*", "a".repeat(514)),
            false,
        ),
    ] {
        let refused = create_package_group(&router, domain, &pattern, None, None).await;
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
        let described = describe_package_group(&router, domain, &pattern).await;
        assert_ne!(
            described.status, 200,
            "case {label:?}: a refused create must store nothing, got {described:?}"
        );
    }
}

/// A package group is freed only by an explicit delete, so the store is bounded: at the
/// per-domain ceiling the next create is refused with `ServiceQuotaExceededException`
/// (402) — the refusal the real service answers a quota with — and nothing is stored.
///
/// The ceiling counts the root group the domain is created with, so a domain accepts one
/// fewer created group than the ceiling.
#[tokio::test]
async fn package_groups_are_bounded_by_the_per_domain_ceiling() {
    /// `crate::state::MAX_PACKAGE_GROUPS_PER_DOMAIN`, restated so the refusal is
    /// asserted against a number this test states rather than one it imports.
    const CEILING: usize = 100;

    let router = emulator();
    let domain = "pg-quota";
    create_domain(&router, domain, None).await;

    // The root group already occupies one slot, so this fills the domain exactly.
    for index in 1..CEILING {
        let pattern = format!("/npm/pkg-{index}$");
        let created = create_package_group(&router, domain, &pattern, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {pattern:?}: group {index} of {CEILING} must fit, got {created:?}"
        );
    }

    let over = format!("/npm/pkg-{CEILING}$");
    let refused = create_package_group(&router, domain, &over, None, None).await;
    assert_eq!(
        (refused.status, refused.code()),
        (402, "ServiceQuotaExceededException".to_string()),
        "case {over:?}: past the ceiling expected 402 ServiceQuotaExceededException, got {refused:?}"
    );
    let described = describe_package_group(&router, domain, &over).await;
    assert_eq!(
        (described.status, described.code()),
        (404, "ResourceNotFoundException".to_string()),
        "case {over:?}: a refused create must store nothing, got {described:?}"
    );

    // Another domain has its own ceiling: the bound is per domain, not per account.
    let other = "pg-quota-two";
    create_domain(&router, other, None).await;
    let elsewhere = create_package_group(&router, other, &over, None, None).await;
    assert_eq!(
        elsewhere.status, 200,
        "case {over:?}: the ceiling is per domain, got {elsewhere:?}"
    );
}

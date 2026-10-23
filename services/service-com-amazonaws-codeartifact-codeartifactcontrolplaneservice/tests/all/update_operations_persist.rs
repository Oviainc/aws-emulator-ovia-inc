// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the CodeArtifact operations that MUTATE an existing
//! record: `UpdateRepository`, `UpdatePackageGroup`, `UpdatePackageVersionsStatus` and
//! `PutPackageOriginConfiguration`.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code and body — not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! example:
//!
//! 1. A successful mutating call COMMITS the members the request carries to the stored
//!    record: the answer it gives and every later read of the same resource — including the
//!    echo of the call that REMOVES it — are the same projection of one record. A member the
//!    request omits is not a value, so the stored one stands.
//! 2. A list member whose order carries meaning is stored and reported in the order
//!    SUBMITTED. `upstreams` is the case here: "the order of the upstream repositories in
//!    the list determines their priority order when CodeArtifact looks for a requested
//!    package version" (`UpdateRepositoryRequest$upstreams`).
//! 3. A refused mutation changes nothing: a request outside a quota, a constraint or a
//!    per-version precondition leaves the record exactly as the last accepted call left it.
//! 4. `UpdatePackageVersionsStatus` decides each named version on its own and answers 200
//!    with a per-version outcome; the versions it moved carry the status and the revision a
//!    later read of them reports.
//!
//! The ceilings and value sets these cases span are read from the service's own constants
//! or from the model's generated ones, so a change to either moves the table rather than
//! leaving a hand-copied list behind.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The `AllowPublish`/`AllowUpstream` catalogs come from the model's own generated
// constants; the upstream ceiling and the set of statuses this operation can move a version
// to come from the service's own constants, which are what the handlers enforce.
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::ops::types::{
    ALLOW_PUBLISH_VALUES, ALLOW_UPSTREAM_VALUES, PACKAGE_VERSION_STATUS_VALUES,
};
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::state::{
    MAX_UPSTREAMS_PER_REPOSITORY, UPDATABLE_PACKAGE_VERSION_STATUSES,
};

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer};
use crate::package_group_hierarchy::{create_package_group, describe_package_group};
use crate::package_version_publish::{
    binary_content, describe, emulator_with_repository, publish, Publish,
};

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// `UpdateRepository`: the domain and repository are `httpQuery`-bound, `description` and
/// `upstreams` travel in the BODY. A member passed as `None` is NOT sent at all, which is
/// what "the caller omitted it" means on the wire.
async fn update_repository(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    description: Option<&str>,
    upstreams: Option<&[&str]>,
) -> Answer {
    let mut body = json!({});
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    if let Some(upstreams) = upstreams {
        body["upstreams"] = Value::Array(
            upstreams
                .iter()
                .map(|name| json!({ "repositoryName": name }))
                .collect(),
        );
    }
    call(
        router,
        "PUT",
        &format!("/v1/repository?domain={domain}&repository={repository}"),
        Some(body),
    )
    .await
}

async fn describe_repository(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/repository?domain={domain}&repository={repository}"),
        None,
    )
    .await
}

async fn delete_repository(router: &MultiServiceRouter, domain: &str, repository: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/repository?domain={domain}&repository={repository}"),
        None,
    )
    .await
}

async fn list_repositories_in_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/domain/repositories?domain={domain}"),
        Some(json!({})),
    )
    .await
}

/// `UpdatePackageGroup`: the domain is `httpQuery`-bound, the pattern and the metadata
/// travel in the BODY (`packageGroup` carries no `httpQuery` binding).
async fn update_package_group(
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
        "PUT",
        &format!("/v1/package-group?domain={domain}"),
        Some(body),
    )
    .await
}

/// `UpdatePackageVersionsStatus` for the package a [`Publish`] addresses: the coordinate is
/// `httpQuery`-bound, the versions and the preconditions travel in the BODY.
async fn update_versions_status(
    router: &MultiServiceRouter,
    request: &Publish,
    versions: &[&str],
    target_status: &str,
    expected_status: Option<&str>,
    version_revisions: Option<&[(&str, &str)]>,
) -> Answer {
    let mut uri = format!(
        "/v1/package/versions/update_status?domain={}&repository={}&format={}&package={}",
        request.domain, request.repository, request.format, request.package
    );
    if let Some(namespace) = &request.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    let mut body = json!({
        "versions": versions,
        "targetStatus": target_status,
    });
    if let Some(expected_status) = expected_status {
        body["expectedStatus"] = json!(expected_status);
    }
    if let Some(revisions) = version_revisions {
        body["versionRevisions"] = Value::Object(
            revisions
                .iter()
                .map(|(version, revision)| ((*version).to_string(), json!(revision)))
                .collect(),
        );
    }
    call(router, "POST", &uri, Some(body)).await
}

/// `DisposePackageVersions` for the package a [`Publish`] addresses: the coordinate is
/// `httpQuery`-bound, the versions and the preconditions travel in the BODY.
async fn dispose_versions(
    router: &MultiServiceRouter,
    request: &Publish,
    versions: &[&str],
    expected_status: Option<&str>,
    version_revisions: Option<&[(&str, &str)]>,
) -> Answer {
    let mut uri = format!(
        "/v1/package/versions/dispose?domain={}&repository={}&format={}&package={}",
        request.domain, request.repository, request.format, request.package
    );
    if let Some(namespace) = &request.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    let mut body = json!({ "versions": versions });
    if let Some(expected_status) = expected_status {
        body["expectedStatus"] = json!(expected_status);
    }
    if let Some(revisions) = version_revisions {
        body["versionRevisions"] = Value::Object(
            revisions
                .iter()
                .map(|(version, revision)| ((*version).to_string(), json!(revision)))
                .collect(),
        );
    }
    call(router, "POST", &uri, Some(body)).await
}

/// `PutPackageOriginConfiguration` for the package a [`Publish`] addresses.
async fn put_origin_configuration(
    router: &MultiServiceRouter,
    request: &Publish,
    publish_mode: &str,
    upstream_mode: &str,
) -> Answer {
    let mut uri = format!(
        "/v1/package?domain={}&repository={}&format={}&package={}",
        request.domain, request.repository, request.format, request.package
    );
    if let Some(namespace) = &request.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    call(
        router,
        "POST",
        &uri,
        Some(json!({
            "restrictions": { "publish": publish_mode, "upstream": upstream_mode }
        })),
    )
    .await
}

/// `DescribePackage` for the package a [`Publish`] addresses.
async fn describe_package(router: &MultiServiceRouter, request: &Publish) -> Answer {
    let mut uri = format!(
        "/v1/package?domain={}&repository={}&format={}&package={}",
        request.domain, request.repository, request.format, request.package
    );
    if let Some(namespace) = &request.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    call(router, "GET", &uri, None).await
}

/// `ListPackages` in the repository a [`Publish`] addresses.
async fn list_packages(router: &MultiServiceRouter, request: &Publish) -> Answer {
    call(
        router,
        "POST",
        &format!(
            "/v1/packages?domain={}&repository={}",
            request.domain, request.repository
        ),
        Some(json!({})),
    )
    .await
}

// ---------------------------------------------------------------------------
// Readers
// ---------------------------------------------------------------------------

/// The `description` a repository answer reports, or `None` when it carries none.
fn repository_description(answer: &Answer) -> Option<String> {
    answer.body["repository"]["description"]
        .as_str()
        .map(str::to_string)
}

/// The `repositoryName` of every upstream a repository answer reports, in the order
/// returned.
fn upstream_names(answer: &Answer) -> Vec<String> {
    answer.body["repository"]["upstreams"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    entry["repositoryName"]
                        .as_str()
                        .unwrap_or("<missing>")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `description` `ListRepositoriesInDomain` reports for one repository.
fn listed_description(answer: &Answer, repository: &str) -> Option<String> {
    answer.body["repositories"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["name"].as_str() == Some(repository))
        })
        .and_then(|entry| entry["description"].as_str())
        .map(str::to_string)
}

/// One string member of a package-group answer.
fn group_member(answer: &Answer, member: &str) -> Option<String> {
    answer.body["packageGroup"][member]
        .as_str()
        .map(str::to_string)
}

/// The `status` and `revision` `DescribePackageVersion` reports for a version.
fn described_status_and_revision(answer: &Answer) -> (Option<String>, Option<String>) {
    (
        answer.body["packageVersion"]["status"]
            .as_str()
            .map(str::to_string),
        answer.body["packageVersion"]["revision"]
            .as_str()
            .map(str::to_string),
    )
}

/// The `assetSizeBytes` `DescribeDomain` reports — the total size of every asset the
/// domain's repositories hold, which is how a caller observes assets appearing and going.
async fn domain_asset_size_bytes(router: &MultiServiceRouter, domain: &str) -> i64 {
    let answer = call(router, "GET", &format!("/v1/domain?domain={domain}"), None).await;
    answer.body["domain"]["assetSizeBytes"]
        .as_i64()
        .unwrap_or_else(|| panic!("no assetSizeBytes in {:?}", answer.body))
}

/// The `originConfiguration.restrictions` of a `DescribePackage` answer.
fn described_restrictions(answer: &Answer) -> Value {
    answer.body["package"]["originConfiguration"]["restrictions"].clone()
}

/// The `originConfiguration.restrictions` `ListPackages` reports for one package.
fn listed_restrictions(answer: &Answer, package: &str) -> Value {
    answer.body["packages"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["package"].as_str() == Some(package))
        })
        .map(|entry| entry["originConfiguration"]["restrictions"].clone())
        .unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// Rule 1: UpdateRepository commits `description`, and every reader agrees.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_repository_commits_description_to_every_reader() {
    let router = emulator();
    let domain = "upd-desc-domain";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "domain setup must succeed"
    );

    // Rows differ along what the rule generalises over: the shape and size of the new
    // value, including both published bounds of `Description` (@length 0..1000) and a
    // multi-byte value.
    let longest = "d".repeat(1000);
    let cases: Vec<(&str, &str)> = vec![
        ("plain replacement", "updated repo description"),
        ("comma and spaces", "sim2real probe repository, updated"),
        ("empty string (the 0 bound)", ""),
        ("one character", "x"),
        ("1000 characters (the upper bound)", longest.as_str()),
        ("multi-byte characters", "dépôt de paquets — révisé ✓"),
        ("punctuation a parser might choke on", "{\"a\": 1} & <b>/*!*/"),
    ];

    for (index, (label, updated)) in cases.iter().enumerate() {
        let repository = format!("upd-desc-repo-{index}");
        let created = create_repository(
            &router,
            domain,
            &repository,
            Some("initial repo description"),
            &[],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: repository setup must succeed, got {created:?}"
        );

        let answer = update_repository(&router, domain, &repository, Some(updated), None).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {label:?}: expected a 200 UpdateRepository, got {answer:?}"
        );
        assert_eq!(
            repository_description(&answer).as_deref(),
            Some(*updated),
            "case {label:?}: the update must answer with the description it stored, got {answer:?}"
        );

        // Rule 1: every reader is the same projection of the record the update committed.
        let described = describe_repository(&router, domain, &repository).await;
        assert_eq!(
            repository_description(&described).as_deref(),
            Some(*updated),
            "case {label:?}: expected DescribeRepository description {updated:?}, got {described:?}"
        );
        assert_eq!(
            described.body["repository"], answer.body["repository"],
            "case {label:?}: the update's answer and the later read must be identical, got {described:?}"
        );

        let listed = list_repositories_in_domain(&router, domain).await;
        assert_eq!(
            listed_description(&listed, &repository).as_deref(),
            Some(*updated),
            "case {label:?}: expected ListRepositoriesInDomain description {updated:?}, got {listed:?}"
        );

        // The echo of the call that REMOVES the repository reports the record it removed,
        // which is the updated one.
        let deleted = delete_repository(&router, domain, &repository).await;
        assert_eq!(
            repository_description(&deleted).as_deref(),
            Some(*updated),
            "case {label:?}: expected the DeleteRepository echo to report {updated:?}, got {deleted:?}"
        );
    }

    // One past the published upper bound of `Description` (@length 0..1000) is refused, and
    // the stored value is untouched (rule 3).
    let repository = "upd-desc-repo-bound";
    assert_eq!(
        create_repository(&router, domain, repository, Some("held"), &[])
            .await
            .status,
        200,
        "repository setup must succeed"
    );
    let refused = update_repository(
        &router,
        domain,
        repository,
        Some(&"d".repeat(1001)),
        None,
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case 1001 characters: expected a 400 for a description past the modelled bound, got {refused:?}"
    );
    let described = describe_repository(&router, domain, repository).await;
    assert_eq!(
        repository_description(&described).as_deref(),
        Some("held"),
        "case 1001 characters: a refused update must change nothing, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1, the other half: an omitted member keeps its stored value.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_repository_leaves_omitted_members_alone() {
    let router = emulator();
    let domain = "upd-partial-domain";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "domain setup must succeed"
    );
    for upstream in ["upd-partial-up-a", "upd-partial-up-b"] {
        assert_eq!(
            create_repository(&router, domain, upstream, None, &[])
                .await
                .status,
            200,
            "upstream setup {upstream} must succeed"
        );
    }
    let repository = "upd-partial-repo";
    assert_eq!(
        create_repository(
            &router,
            domain,
            repository,
            Some("held description"),
            &["upd-partial-up-a"],
        )
        .await
        .status,
        200,
        "repository setup must succeed"
    );

    // Each row sends one shape of request and states what every member must read as
    // afterwards: a member sent is committed, a member omitted keeps the value the row
    // before it left.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Option<&str>, Option<&[&str]>, &str, Vec<&str>)> = vec![
        (
            "description only",
            Some("new description"),
            None,
            "new description",
            vec!["upd-partial-up-a"],
        ),
        (
            "upstreams only",
            None,
            Some(&["upd-partial-up-b", "upd-partial-up-a"][..]),
            "new description",
            vec!["upd-partial-up-b", "upd-partial-up-a"],
        ),
        (
            "neither member",
            None,
            None,
            "new description",
            vec!["upd-partial-up-b", "upd-partial-up-a"],
        ),
        (
            "both members",
            Some("both at once"),
            Some(&["upd-partial-up-a"][..]),
            "both at once",
            vec!["upd-partial-up-a"],
        ),
        (
            "an empty upstream list clears them",
            None,
            Some(&[][..]),
            "both at once",
            vec![],
        ),
        (
            "an empty description is a value",
            Some(""),
            None,
            "",
            vec![],
        ),
    ];

    for (label, description, upstreams, expected_description, expected_upstreams) in &cases {
        let answer = update_repository(&router, domain, repository, *description, *upstreams).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {label:?}: expected a 200 UpdateRepository, got {answer:?}"
        );

        let described = describe_repository(&router, domain, repository).await;
        assert_eq!(
            repository_description(&described).as_deref(),
            Some(*expected_description),
            "case {label:?}: expected description {expected_description:?}, got {described:?}"
        );
        assert_eq!(
            upstream_names(&described),
            *expected_upstreams,
            "case {label:?}: expected upstreams {expected_upstreams:?}, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the submitted upstream order is the stored and reported order.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_repository_reports_upstreams_in_submitted_order() {
    let router = emulator();
    let domain = "upd-order-domain";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "domain setup must succeed"
    );

    // One upstream candidate per slot of the published ceiling, named so that creation
    // order, alphabetical order and the submitted order of the rows below all differ: a
    // reader that re-sorted the list cannot pass any row that submits a non-sorted order.
    let upstreams: Vec<String> = (0..MAX_UPSTREAMS_PER_REPOSITORY)
        .map(|index| format!("upd-order-up-{index}"))
        .collect();
    for upstream in &upstreams {
        assert_eq!(
            create_repository(&router, domain, upstream, None, &[])
                .await
                .status,
            200,
            "upstream setup {upstream} must succeed"
        );
    }
    let repository = "upd-order-repo";
    assert_eq!(
        create_repository(&router, domain, repository, None, &[])
            .await
            .status,
        200,
        "repository setup must succeed"
    );

    let names: Vec<&str> = upstreams.iter().map(String::as_str).collect();
    let reversed: Vec<&str> = names.iter().rev().copied().collect();
    // Rows differ along what the rule generalises over: the COUNT of upstreams and their
    // ORDER, up to and including the whole ceiling in reverse.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("one upstream", vec![names[0]]),
        ("two, reversed", vec![names[1], names[0]]),
        ("two, in order", vec![names[0], names[1]]),
        ("three, rotated", vec![names[2], names[0], names[1]]),
        (
            "four, interleaved",
            vec![names[3], names[1], names[2], names[0]],
        ),
        ("the whole ceiling, reversed", reversed.clone()),
    ];

    for (label, order) in &cases {
        let answer = update_repository(&router, domain, repository, None, Some(order)).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {label:?}: expected a 200 UpdateRepository, got {answer:?}"
        );
        assert_eq!(
            upstream_names(&answer),
            *order,
            "case {label:?}: the update must answer with the order it stored, got {answer:?}"
        );

        let described = describe_repository(&router, domain, repository).await;
        assert_eq!(
            upstream_names(&described),
            *order,
            "case {label:?}: expected DescribeRepository upstreams {order:?}, got {described:?}"
        );
    }

    // The order the rows above assert is NOT the sorted order, so a reader that sorted
    // could not have passed them: pinned here so the table cannot degrade into one that a
    // sorted projection satisfies.
    let mut sorted = reversed.clone();
    sorted.sort_unstable();
    assert_ne!(
        reversed, sorted,
        "the ceiling row must submit a non-sorted order, got {reversed:?}"
    );

    // Rule 3: each refusal leaves the stored list — the last accepted one — untouched.
    let over_ceiling: Vec<&str> = {
        let mut over = names.clone();
        over.push(names[0]);
        over
    };
    let refusals: Vec<(&str, Vec<&str>, u16)> = vec![
        ("one past the upstream ceiling", over_ceiling, 402),
        ("a repeated upstream", vec![names[0], names[0]], 400),
        ("an upstream the domain does not hold", vec!["upd-order-absent"], 404),
        ("the repository itself", vec![repository], 400),
    ];
    for (label, order, expected_status) in &refusals {
        let refused = update_repository(&router, domain, repository, None, Some(order)).await;
        assert_eq!(
            refused.status, *expected_status,
            "case {label:?}: expected HTTP {expected_status}, got {refused:?}"
        );
        let described = describe_repository(&router, domain, repository).await;
        assert_eq!(
            upstream_names(&described),
            reversed,
            "case {label:?}: a refused update must change nothing, got {described:?}"
        );
    }

    // The self-reference refusal names both ends of the illegal reference — the upstream
    // that was named and the repository that was being updated — so the caller can tell
    // which entry of the list was the problem. Asserted on a repository whose name is not a
    // substring of any other name in the row, so a message that named only one end would
    // fail.
    let refused = update_repository(&router, domain, repository, None, Some(&[repository])).await;
    let message = refused.message();
    assert!(
        message.contains(&format!("Upstream={repository}"))
            && message.contains(&format!("repository={repository}")),
        "case the repository itself: the refusal must name the upstream and the repository, got {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1 on a package group: contactInfo and description are committed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_package_group_commits_its_metadata() {
    let router = emulator();
    let domain = "upd-group-domain";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "domain setup must succeed"
    );
    let pattern = "/npm/widgets/*";
    let created = create_package_group(
        &router,
        domain,
        pattern,
        Some("team-widgets@example.com"),
        Some("widget scoped npm packages"),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "package group setup must succeed, got {created:?}"
    );

    // Rows differ along what the rule generalises over: WHICH members the request carries
    // and the shape of each value, including both published bounds of
    // `PackageGroupContactInfo`/`Description` (@length 0..1000).
    let longest = "c".repeat(1000);
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Option<&str>, Option<&str>, &str, &str)> = vec![
        (
            "both members",
            Some("new-team@example.com"),
            Some("updated widget npm packages"),
            "new-team@example.com",
            "updated widget npm packages",
        ),
        (
            "contactInfo only",
            Some("oncall@example.com"),
            None,
            "oncall@example.com",
            "updated widget npm packages",
        ),
        (
            "description only",
            None,
            Some("widgets, second revision"),
            "oncall@example.com",
            "widgets, second revision",
        ),
        ("neither member", None, None, "oncall@example.com", "widgets, second revision"),
        (
            "empty strings are values",
            Some(""),
            Some(""),
            "",
            "",
        ),
        (
            "1000 characters (the upper bound)",
            Some(longest.as_str()),
            Some(longest.as_str()),
            longest.as_str(),
            longest.as_str(),
        ),
        (
            "multi-byte characters",
            Some("équipe@example.com"),
            Some("paquets npm — révisés"),
            "équipe@example.com",
            "paquets npm — révisés",
        ),
    ];

    for (label, contact_info, description, expected_contact, expected_description) in &cases {
        let answer =
            update_package_group(&router, domain, pattern, *contact_info, *description).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {label:?}: expected a 200 UpdatePackageGroup, got {answer:?}"
        );
        assert_eq!(
            (
                group_member(&answer, "contactInfo").as_deref(),
                group_member(&answer, "description").as_deref()
            ),
            (Some(*expected_contact), Some(*expected_description)),
            "case {label:?}: the update must answer with what it stored, got {answer:?}"
        );

        let described = describe_package_group(&router, domain, pattern).await;
        assert_eq!(
            (
                group_member(&described, "contactInfo").as_deref(),
                group_member(&described, "description").as_deref()
            ),
            (Some(*expected_contact), Some(*expected_description)),
            "case {label:?}: expected DescribePackageGroup to report {expected_contact:?}/{expected_description:?}, got {described:?}"
        );
        assert_eq!(
            described.body["packageGroup"], answer.body["packageGroup"],
            "case {label:?}: the update's answer and the later read must be identical, got {described:?}"
        );
    }

    // Rule 3: a group the domain does not hold is 404 and a pattern that is not a path from
    // the domain root is 400; neither creates anything.
    let missing = update_package_group(&router, domain, "/npm/absent/*", Some("x"), None).await;
    assert_eq!(
        missing.status, 404,
        "case an absent package group: expected HTTP 404, got {missing:?}"
    );
    let malformed = update_package_group(&router, domain, "npm/widgets/*", Some("x"), None).await;
    assert_eq!(
        malformed.status, 400,
        "case a pattern that is not a path: expected HTTP 400, got {malformed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rules 1 and 4: a version status update is committed and read back.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_package_versions_status_commits_the_new_status() {
    let router = emulator_with_repository().await;

    // The statuses this operation can move a version to come from the service constant the
    // handler enforces, which quotes the operation's own documentation ("you can update the
    // status of package versions to Archived, Published, or Unlisted").
    assert_eq!(
        UPDATABLE_PACKAGE_VERSION_STATUSES,
        ["Archived", "Published", "Unlisted"],
        "the updatable status set must be the documented three, got {UPDATABLE_PACKAGE_VERSION_STATUSES:?}"
    );

    // Rows differ along what the rule generalises over: the target status, the namespace of
    // the package, and the shape of the version string.
    let cases: Vec<(&str, Publish)> = vec![
        (
            "default namespace",
            Publish::new("alpha", "1.0.0", "payload.bin", b"one".to_vec()),
        ),
        (
            "another namespace",
            Publish::new("beta", "2.0.0-rc.1", "notes.txt", b"two".to_vec()).namespace("team-a"),
        ),
        (
            "dotted name and date-like version",
            Publish::new("org.example.tool", "20240101", "tool.jar", b"three".to_vec())
                .namespace("com.example.tools"),
        ),
    ];

    for (label, request) in &cases {
        let published = publish(&router, request).await;
        assert_eq!(
            published.status, 200,
            "case {label:?}: publish setup must succeed, got {published:?}"
        );
        let (status, _) = described_status_and_revision(&describe(&router, request).await);
        assert_eq!(
            status.as_deref(),
            Some("Published"),
            "case {label:?}: a closed publish starts at Published, got {status:?}"
        );

        for target in UPDATABLE_PACKAGE_VERSION_STATUSES {
            let answer = update_versions_status(
                &router,
                request,
                &[request.version.as_str()],
                target,
                None,
                None,
            )
            .await;
            assert_eq!(
                (answer.status, answer.code()),
                (200, String::new()),
                "case {label:?} -> {target}: expected a 200 UpdatePackageVersionsStatus, got {answer:?}"
            );
            assert!(
                answer.body["failedVersions"]
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty),
                "case {label:?} -> {target}: expected no failed versions, got {answer:?}"
            );
            let reported = &answer.body["successfulVersions"][request.version.as_str()];
            assert_eq!(
                reported["status"].as_str(),
                Some(*target),
                "case {label:?} -> {target}: expected successfulVersions to report the new status, got {answer:?}"
            );

            // Rule 1: the move is committed, so the read reports it — with the SAME
            // revision the answer reported, because a version's revision identifies its
            // content and its status is part of that content.
            let (status, revision) =
                described_status_and_revision(&describe(&router, request).await);
            assert_eq!(
                status.as_deref(),
                Some(*target),
                "case {label:?} -> {target}: expected DescribePackageVersion to report {target}, got {status:?}"
            );
            assert_eq!(
                revision.as_deref(),
                reported["revision"].as_str(),
                "case {label:?} -> {target}: the reported revision must be the one a read reports, got {revision:?}"
            );
        }
    }
}

#[tokio::test]
async fn update_package_versions_status_refusals_change_nothing() {
    let router = emulator_with_repository().await;
    let request = Publish::new("gamma", "3.1.4", "payload.bin", b"gamma".to_vec());
    assert_eq!(
        publish(&router, &request).await.status,
        200,
        "publish setup must succeed"
    );
    let (_, held_revision) = described_status_and_revision(&describe(&router, &request).await);
    let held_revision = held_revision.expect("a published version reports a revision");

    // A `targetStatus` outside the documented three is a bad REQUEST, not a status change.
    // The catalog is the model's whole `PackageVersionStatus` enum, so the row set is
    // exactly "every modelled value this operation does not accept".
    let refused_targets: Vec<&&str> = PACKAGE_VERSION_STATUS_VALUES
        .iter()
        .filter(|status| !UPDATABLE_PACKAGE_VERSION_STATUSES.contains(status))
        .collect();
    assert_eq!(
        refused_targets.len(),
        3,
        "expected three non-updatable statuses, got {refused_targets:?}"
    );
    for target in &refused_targets {
        let refused = update_versions_status(
            &router,
            &request,
            &[request.version.as_str()],
            target,
            None,
            None,
        )
        .await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "ValidationException".to_string()),
            "case targetStatus {target}: expected a 400 ValidationException, got {refused:?}"
        );
        assert!(
            refused.message().contains("targetStatus"),
            "case targetStatus {target}: the refusal must name the member, got {refused:?}"
        );
        let (status, _) = described_status_and_revision(&describe(&router, &request).await);
        assert_eq!(
            status.as_deref(),
            Some("Published"),
            "case targetStatus {target}: a refused request must change nothing, got {status:?}"
        );
    }

    // A precondition that does not hold is reported PER VERSION, with the modelled error
    // code, and leaves the version exactly as it was. Rows differ along which precondition
    // fails and how.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, &str, Option<&str>, Option<Vec<(&str, &str)>>, &str)> = vec![
        (
            "a version the package does not hold",
            "9.9.9",
            None,
            None,
            "NOT_FOUND",
        ),
        (
            "an expectedStatus the version is not in",
            request.version.as_str(),
            Some("Unlisted"),
            None,
            "MISMATCHED_STATUS",
        ),
        (
            "a versionRevisions entry that pins another revision",
            request.version.as_str(),
            None,
            Some(vec![(request.version.as_str(), "not-the-held-revision")]),
            "MISMATCHED_REVISION",
        ),
    ];

    for (label, version, expected_status, revisions, expected_code) in &cases {
        let answer = update_versions_status(
            &router,
            &request,
            &[version],
            "Archived",
            *expected_status,
            revisions.as_deref(),
        )
        .await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {label:?}: a per-version outcome is answered 200, got {answer:?}"
        );
        assert_eq!(
            answer.body["failedVersions"][*version]["errorCode"].as_str(),
            Some(*expected_code),
            "case {label:?}: expected errorCode {expected_code}, got {answer:?}"
        );
        assert!(
            answer.body["successfulVersions"]
                .as_object()
                .is_some_and(serde_json::Map::is_empty),
            "case {label:?}: nothing must have succeeded, got {answer:?}"
        );
        let (status, revision) = described_status_and_revision(&describe(&router, &request).await);
        assert_eq!(
            (status.as_deref(), revision.as_deref()),
            (Some("Published"), Some(held_revision.as_str())),
            "case {label:?}: the version must be untouched, got {status:?}/{revision:?}"
        );
    }

    // The same precondition, satisfied: the revision the version actually holds lets the
    // move through.
    let answer = update_versions_status(
        &router,
        &request,
        &[request.version.as_str()],
        "Archived",
        Some("Published"),
        Some(&[(request.version.as_str(), held_revision.as_str())]),
    )
    .await;
    assert_eq!(
        answer.body["successfulVersions"][request.version.as_str()]["status"].as_str(),
        Some("Archived"),
        "case preconditions that hold: expected the move to be applied, got {answer:?}"
    );
    let (status, _) = described_status_and_revision(&describe(&router, &request).await);
    assert_eq!(
        status.as_deref(),
        Some("Archived"),
        "case preconditions that hold: expected DescribePackageVersion to report Archived, got {status:?}"
    );

    // A domain, repository or package that is not held is refused for the whole call rather
    // than reported per version.
    let absent = Publish::new("absent-package", "1.0.0", "payload.bin", b"x".to_vec());
    let refused = update_versions_status(&router, &absent, &["1.0.0"], "Archived", None, None).await;
    assert_eq!(
        refused.status, 404,
        "case an absent package: expected HTTP 404, got {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1 on origin control: the restrictions a put commits are what both readers report.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn put_package_origin_configuration_commits_restrictions_to_both_readers() {
    let router = emulator_with_repository().await;
    let request = Publish::new("delta", "1.0.0", "payload.bin", b"delta".to_vec());
    assert_eq!(
        publish(&router, &request).await.status,
        200,
        "publish setup must succeed"
    );

    // Before any put: the controls a package gets from HOW it was created. This one was
    // created by publishing a version directly into the repository, so publishing is
    // allowed and upstream ingestion is blocked.
    let created_by_publish = json!({ "publish": "ALLOW", "upstream": "BLOCK" });
    for (reader, answer) in [
        ("DescribePackage", describe_package(&router, &request).await),
        ("ListPackages", list_packages(&router, &request).await),
    ] {
        let restrictions = match reader {
            "DescribePackage" => described_restrictions(&answer),
            _ => listed_restrictions(&answer, &request.package),
        };
        assert_eq!(
            restrictions, created_by_publish,
            "case a package created by a direct publish, {reader}: expected {created_by_publish:?}, got {answer:?}"
        );
    }

    // The table is the model's own `AllowPublish` × `AllowUpstream` value sets, so every
    // combination a caller can set is a row — the recorded instance (upstream BLOCK) is one
    // of them.
    for publish_mode in ALLOW_PUBLISH_VALUES {
        for upstream_mode in ALLOW_UPSTREAM_VALUES {
            let label = format!("publish {publish_mode}, upstream {upstream_mode}");
            let expected = json!({ "publish": publish_mode, "upstream": upstream_mode });

            let answer = put_origin_configuration(&router, &request, publish_mode, upstream_mode)
                .await;
            assert_eq!(
                (answer.status, answer.code()),
                (200, String::new()),
                "case {label:?}: expected a 200 PutPackageOriginConfiguration, got {answer:?}"
            );
            assert_eq!(
                answer.body["originConfiguration"]["restrictions"], expected,
                "case {label:?}: the put must answer with what it stored, got {answer:?}"
            );

            let described = describe_package(&router, &request).await;
            assert_eq!(
                described_restrictions(&described),
                expected,
                "case {label:?}: expected DescribePackage to report {expected:?}, got {described:?}"
            );

            let listed = list_packages(&router, &request).await;
            assert_eq!(
                listed_restrictions(&listed, &request.package),
                expected,
                "case {label:?}: expected ListPackages to report {expected:?}, got {listed:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rules 1, 3 and 4 on DisposePackageVersions: the assets go, the status moves, and the
// status is terminal.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispose_package_versions_deletes_the_assets_and_commits_a_terminal_status() {
    let router = emulator_with_repository().await;

    // Rows differ along what the rule generalises over: the namespace, the size of the
    // assets disposed (so the derived total it frees differs), and the status the version is
    // in when it is disposed.
    let cases: Vec<(&str, Publish, Option<&str>)> = vec![
        (
            "a published version",
            Publish::new("alpha", "1.0.0", "payload.bin", b"one".to_vec()),
            None,
        ),
        (
            "an archived version, namespaced, 4 KiB",
            Publish::new("beta", "2.0.0", "bundle.tar.gz", binary_content(4096, 11))
                .namespace("team-a"),
            Some("Archived"),
        ),
        (
            "an unlisted version, one byte",
            Publish::new("gamma", "0.0.1", "p.bin", vec![0x7f]),
            Some("Unlisted"),
        ),
    ];

    for (label, request, retire_to) in &cases {
        let published = publish(&router, request).await;
        assert_eq!(
            published.status, 200,
            "case {label:?}: publish setup must succeed, got {published:?}"
        );
        if let Some(target) = retire_to {
            let retired = update_versions_status(
                &router,
                request,
                &[request.version.as_str()],
                target,
                None,
                None,
            )
            .await;
            assert_eq!(
                retired.status, 200,
                "case {label:?}: retiring to {target} must succeed, got {retired:?}"
            );
        }

        let held_bytes = domain_asset_size_bytes(&router, &request.domain).await;
        let (_, revision_before) =
            described_status_and_revision(&describe(&router, request).await);

        let answer = dispose_versions(&router, request, &[request.version.as_str()], None, None)
            .await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {label:?}: expected a 200 DisposePackageVersions, got {answer:?}"
        );
        let reported = &answer.body["successfulVersions"][request.version.as_str()];
        assert_eq!(
            reported["status"].as_str(),
            Some("Disposed"),
            "case {label:?}: expected successfulVersions to report Disposed, got {answer:?}"
        );

        // Rule 1: the move is committed, and the revision reported is the one a read of the
        // disposed version reports — the assets are part of what a revision identifies, so
        // deleting them moves it.
        let (status, revision) = described_status_and_revision(&describe(&router, request).await);
        assert_eq!(
            status.as_deref(),
            Some("Disposed"),
            "case {label:?}: expected DescribePackageVersion to report Disposed, got {status:?}"
        );
        assert_eq!(
            revision.as_deref(),
            reported["revision"].as_str(),
            "case {label:?}: the reported revision must be the one a read reports, got {revision:?}"
        );
        assert_ne!(
            revision, revision_before,
            "case {label:?}: disposing deletes the assets, so the revision must move, got {revision:?}"
        );

        // The assets really are gone: the domain's `assetSizeBytes` is summed from the
        // assets its repositories hold, so it drops by exactly what this version held.
        let freed = held_bytes - domain_asset_size_bytes(&router, &request.domain).await;
        assert_eq!(
            freed,
            request.content.len() as i64,
            "case {label:?}: expected the disposed assets ({} bytes) to be freed, got {freed}",
            request.content.len()
        );

        // Rule 3/4: the status is terminal — no later status move can bring the version
        // back, and the attempt is reported per version rather than failing the call.
        for target in UPDATABLE_PACKAGE_VERSION_STATUSES {
            let refused = update_versions_status(
                &router,
                request,
                &[request.version.as_str()],
                target,
                None,
                None,
            )
            .await;
            assert_eq!(
                refused.body["failedVersions"][request.version.as_str()]["errorCode"].as_str(),
                Some("NOT_ALLOWED"),
                "case {label:?} -> {target}: a disposed version cannot be moved, got {refused:?}"
            );
            let (status, _) = described_status_and_revision(&describe(&router, request).await);
            assert_eq!(
                status.as_deref(),
                Some("Disposed"),
                "case {label:?} -> {target}: the disposed version must stay Disposed, got {status:?}"
            );
        }
    }

    // Rule 3 for the dispose preconditions: each one that does not hold is reported per
    // version and leaves the version — assets included — exactly as it was.
    let request = Publish::new("delta", "5.0.0", "payload.bin", binary_content(128, 5));
    assert_eq!(
        publish(&router, &request).await.status,
        200,
        "publish setup must succeed"
    );
    let held_bytes = domain_asset_size_bytes(&router, &request.domain).await;
    let (_, held_revision) = described_status_and_revision(&describe(&router, &request).await);
    let held_revision = held_revision.expect("a published version reports a revision");

    #[allow(clippy::type_complexity)]
    let refusals: Vec<(&str, &str, Option<&str>, Option<Vec<(&str, &str)>>, &str)> = vec![
        (
            "a version the package does not hold",
            "9.9.9",
            None,
            None,
            "NOT_FOUND",
        ),
        (
            "an expectedStatus the version is not in",
            request.version.as_str(),
            Some("Archived"),
            None,
            "MISMATCHED_STATUS",
        ),
        (
            "a versionRevisions entry that pins another revision",
            request.version.as_str(),
            None,
            Some(vec![(request.version.as_str(), "not-the-held-revision")]),
            "MISMATCHED_REVISION",
        ),
    ];

    for (label, version, expected_status, revisions, expected_code) in &refusals {
        let answer = dispose_versions(
            &router,
            &request,
            &[version],
            *expected_status,
            revisions.as_deref(),
        )
        .await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {label:?}: a per-version outcome is answered 200, got {answer:?}"
        );
        assert_eq!(
            answer.body["failedVersions"][*version]["errorCode"].as_str(),
            Some(*expected_code),
            "case {label:?}: expected errorCode {expected_code}, got {answer:?}"
        );
        let (status, revision) = described_status_and_revision(&describe(&router, &request).await);
        assert_eq!(
            (status.as_deref(), revision.as_deref()),
            (Some("Published"), Some(held_revision.as_str())),
            "case {label:?}: the version must be untouched, got {status:?}/{revision:?}"
        );
        assert_eq!(
            domain_asset_size_bytes(&router, &request.domain).await,
            held_bytes,
            "case {label:?}: a refused dispose must not delete any asset"
        );
    }

    // A package that is not held is refused for the whole call rather than reported per
    // version.
    let absent = Publish::new("absent-package", "1.0.0", "payload.bin", b"x".to_vec());
    let refused = dispose_versions(&router, &absent, &["1.0.0"], None, None).await;
    assert_eq!(
        refused.status, 404,
        "case an absent package: expected HTTP 404, got {refused:?}"
    );
}

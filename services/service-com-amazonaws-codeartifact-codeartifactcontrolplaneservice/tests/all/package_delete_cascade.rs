// SPDX-License-Identifier: CC0-1.0

//! What `DeletePackage` removes, asserted end-to-end over the wire.
//!
//! Every case drives real `restJson1` requests (and, for the npm rows, real registry
//! requests) through the `MultiServiceRouter` the production emulator serves with, so what
//! is pinned here is the status, the body and the stored state a CodeArtifact client reads
//! — never a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of packages rather than one recorded
//! example:
//!
//! 1. Deleting a package removes the package AND every version of it ("Deletes a package
//!    and all associated package versions. A deleted package cannot be restored" —
//!    `smithy_query(query="operation",operation="DeletePackage")`): afterwards
//!    `DescribePackage` and `ListPackageVersions` answer `ResourceNotFoundException` (404),
//!    `DescribePackageVersion` answers it for every version the package held, and
//!    `ListPackages` no longer reports the coordinate.
//! 2. The call answers with the record it removed, projected as the same `PackageSummary`
//!    every package read reports — its format, its namespace, its name and the origin
//!    configuration it carried.
//! 3. A delete removes ONLY the package it addresses: a package of the same NAME under
//!    another format or namespace, and the same package in another repository, read back
//!    afterwards exactly as before — versions included.
//! 4. The domain's derived `assetSizeBytes` drops by exactly what the removed package's
//!    versions held, so the cascade is committed to the store rather than hidden from one
//!    read.
//! 5. A package the addressed repository does not hold is refused with the PACKAGE-scoped
//!    404 sentence, and the SCOPE the request is addressed to (the domain, the repository)
//!    is refused with the same 404 naming the kind it could not resolve — and neither
//!    removes anything.

use serde_json::{json, Value};
use sha2::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{auth_for, call, create_domain, emulator, Answer, REGION};
use crate::npm_registry_endpoint::{endpoint_host, endpoint_path, publish_document, registry};

/// The domain every case lives in.
const DOMAIN: &str = "delete-cascade";

/// The repository every row is seeded into and deleted from.
const REPOSITORY: &str = "Releases";

/// A second repository holding a COPY of one row's coordinate, so the rows can assert that
/// a delete reaches only the repository it addressed.
const OTHER_REPOSITORY: &str = "Staging";

/// A well-formed 12-digit account that is NOT the one this emulator serves (`AccountId` is
/// modelled `len 12..12, pattern ^[0-9]{12}$`), used to address a domain this account does
/// not hold.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// How a row's package comes to exist in the repository.
///
/// The three ways this emulator holds one, which is also what makes the rows differ in how
/// many versions the cascade has to remove: `PublishPackageVersion` serves the `generic`
/// format only ("Only generic packages can be published using this API" —
/// `smithy_query(query="operation",operation="PublishPackageVersion")`), an npm version
/// arrives over the repository endpoint, and `PutPackageOriginConfiguration` is documented
/// to create a package carrying no version at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seeded {
    /// `PublishPackageVersion`, one call per version.
    GenericPublish,
    /// An npm publish document over the repository's npm endpoint.
    NpmRegistry,
    /// `PutPackageOriginConfiguration`, which creates the package and no version.
    OriginConfigurationOnly,
}

/// One package a row addresses: its coordinate, how it is seeded and the versions it holds.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    format: &'static str,
    namespace: Option<&'static str>,
    package: &'static str,
    /// The path an npm client PUTs the publish document to — the package name as a URL
    /// path, with a scope's `/` percent-encoded. Unused by the other seeds.
    npm_path: &'static str,
    versions: &'static [&'static str],
    seeded: Seeded,
}

impl Row {
    /// The query members every package-scoped route carries for this coordinate.
    fn query(&self, repository: &str) -> String {
        let mut query = format!(
            "domain={DOMAIN}&repository={repository}&format={}&package={}",
            self.format, self.package
        );
        if let Some(namespace) = self.namespace {
            query.push_str(&format!("&namespace={namespace}"));
        }
        query
    }

    /// RULE 5: the sentence the PACKAGE-scoped operations state for this coordinate when the
    /// repository holds no such package. Written out here rather than imported, so a wrong
    /// sentence in the implementation cannot make this suite pass.
    fn package_family_message(&self) -> String {
        match self.namespace {
            Some(namespace) => format!(
                "Package not found in repository. Package = {{format={}, namespace={namespace}, \
                 name={}}}",
                self.format, self.package
            ),
            None => format!(
                "Package not found in repository. Package = {{format={}, name={}}}",
                self.format, self.package
            ),
        }
    }

    /// RULE 1: the sentence the VERSION-addressing operations state for the same absence,
    /// naming the repository the versions were looked for in.
    fn version_family_message(&self, repository: &str) -> String {
        match self.namespace {
            Some(namespace) => format!(
                "The {} package '{}' with namespace '{namespace}' does not exist in \
                 repository '{repository}'.",
                self.format, self.package
            ),
            None => format!(
                "The {} package '{}' does not exist in repository '{repository}'.",
                self.format, self.package
            ),
        }
    }

    /// The display name an npm publish document declares for this coordinate.
    fn npm_display_name(&self) -> String {
        match self.namespace {
            Some(scope) => format!("@{scope}/{}", self.package),
            None => self.package.to_string(),
        }
    }
}

/// The packages every rule is asserted over.
///
/// The rows differ along what the rules generalise over: the FORMAT (`generic`, `npm`,
/// `maven`, `pypi`), whether the coordinate carries a NAMESPACE, how many versions the
/// cascade has to remove (0, 1, 2, 3) and how the package came to exist. Three rows share
/// the name `widget` under three different coordinates, which is what makes rule 3
/// assertable: deleting one of them must leave the other two.
const ROWS: [Row; 6] = [
    Row {
        case: "generic, namespaced, one version",
        format: "generic",
        namespace: Some("team-core"),
        package: "release.bundle",
        npm_path: "",
        versions: &["1.0.0"],
        seeded: Seeded::GenericPublish,
    },
    Row {
        case: "generic, namespaced, three versions",
        format: "generic",
        namespace: Some("ops"),
        package: "toolchain",
        npm_path: "",
        versions: &["0.1.0", "0.2.0", "1.0.0"],
        seeded: Seeded::GenericPublish,
    },
    Row {
        case: "npm, unscoped, two versions",
        format: "npm",
        namespace: None,
        package: "widget",
        npm_path: "widget",
        versions: &["1.0.0", "1.1.0"],
        seeded: Seeded::NpmRegistry,
    },
    Row {
        case: "npm, scoped, one version",
        format: "npm",
        namespace: Some("acme"),
        package: "widget",
        npm_path: "@acme%2fwidget",
        versions: &["2.1.0"],
        seeded: Seeded::NpmRegistry,
    },
    Row {
        case: "maven, namespaced, no versions",
        format: "maven",
        namespace: Some("com.acme"),
        package: "widget",
        npm_path: "",
        versions: &[],
        seeded: Seeded::OriginConfigurationOnly,
    },
    Row {
        case: "pypi, no namespace, no versions",
        format: "pypi",
        namespace: None,
        package: "left-pad",
        npm_path: "",
        versions: &[],
        seeded: Seeded::OriginConfigurationOnly,
    },
];

// ---------------------------------------------------------------------------
// The requests this suite sends, each the modelled route of one operation.
// ---------------------------------------------------------------------------

/// `DeletePackage`: every member is `httpQuery`-bound, so the request carries no body.
async fn delete_package(router: &MultiServiceRouter, uri_query: &str) -> Answer {
    call(router, "DELETE", &format!("/v1/package?{uri_query}"), None).await
}

/// `DescribePackage` for one coordinate.
async fn describe_package(router: &MultiServiceRouter, row: &Row, repository: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/package?{}", row.query(repository)),
        None,
    )
    .await
}

/// `ListPackageVersions` for one coordinate.
async fn list_versions(router: &MultiServiceRouter, row: &Row, repository: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/package/versions?{}", row.query(repository)),
        Some(json!({})),
    )
    .await
}

/// `DescribePackageVersion` for one version of one coordinate.
async fn describe_version(
    router: &MultiServiceRouter,
    row: &Row,
    repository: &str,
    version: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/v1/package/version?{}&version={version}",
            row.query(repository)
        ),
        None,
    )
    .await
}

/// `ListPackageVersionAssets` for one version of one coordinate.
async fn list_assets(
    router: &MultiServiceRouter,
    row: &Row,
    repository: &str,
    version: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!(
            "/v1/package/version/assets?{}&version={version}",
            row.query(repository)
        ),
        Some(json!({})),
    )
    .await
}

/// `ListPackages` for one repository: the coordinates it holds, as `<format>/<namespace>/<name>`.
async fn held_coordinates(
    router: &MultiServiceRouter,
    repository: &str,
    case: &str,
) -> Vec<String> {
    let answer = call(
        router,
        "POST",
        &format!("/v1/packages?domain={DOMAIN}&repository={repository}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: ListPackages must succeed, got {answer:?}"
    );
    answer.body["packages"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a packages list, got {answer:?}"))
        .iter()
        .map(|summary| {
            format!(
                "{}/{}/{}",
                summary["format"].as_str().unwrap_or_default(),
                summary["namespace"].as_str().unwrap_or_default(),
                summary["package"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

/// The coordinate key [`held_coordinates`] reports a row under.
fn coordinate_key(row: &Row) -> String {
    format!(
        "{}/{}/{}",
        row.format,
        row.namespace.unwrap_or_default(),
        row.package
    )
}

/// `DescribeDomain`'s derived `assetSizeBytes`.
async fn domain_asset_bytes(router: &MultiServiceRouter, case: &str) -> i64 {
    let answer = call(router, "GET", &format!("/v1/domain?domain={DOMAIN}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: DescribeDomain must succeed, got {answer:?}"
    );
    answer.body["domain"]["assetSizeBytes"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: expected assetSizeBytes, got {answer:?}"))
}

/// `PutPackageOriginConfiguration`, which both creates a version-less package and pins the
/// origin configuration the delete has to report back.
async fn put_origin_configuration(
    router: &MultiServiceRouter,
    row: &Row,
    repository: &str,
    publish: &str,
    upstream: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/package?{}", row.query(repository)),
        Some(json!({ "restrictions": { "publish": publish, "upstream": upstream } })),
    )
    .await
}

/// `PublishPackageVersion`: the asset bytes are the HTTP payload and the declared digest
/// rides the `x-amz-content-sha256` header the model binds `assetSHA256` to, so this one
/// request cannot go through the suite's JSON [`call`].
async fn publish_generic(
    router: &MultiServiceRouter,
    row: &Row,
    repository: &str,
    version: &str,
    content: &[u8],
) -> u16 {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "/v1/package/version/publish?{}&version={version}&asset=bundle.bin",
            row.query(repository)
        ))
        .header("host", format!("codeartifact.{REGION}.amazonaws.com"))
        .header("authorization", auth_for(REGION))
        .header("content-type", "application/octet-stream")
        .header(
            "x-amz-content-sha256",
            hex::encode(sha2::Sha256::digest(content)),
        )
        .body(bytes::Bytes::copy_from_slice(content))
        .expect("build request");
    router.dispatch(request).await.status().as_u16()
}

/// Seed one row into `repository`, however that row's packages come to exist.
async fn given_package(router: &MultiServiceRouter, row: &Row, repository: &str, token: &str) {
    let case = row.case;
    match row.seeded {
        Seeded::GenericPublish => {
            for version in row.versions {
                // The bytes differ per version, so the asset sizes a row's versions hold
                // differ too and rule 4's arithmetic is not satisfied by a constant.
                let content = format!("{} {version} payload", row.package).into_bytes();
                let status = publish_generic(router, row, repository, version, &content).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: PublishPackageVersion {version} must succeed, got {status}"
                );
            }
        }
        Seeded::NpmRegistry => {
            let published = registry(
                router,
                "PUT",
                &endpoint_host(DOMAIN),
                &format!("{}/{}", endpoint_path(repository), row.npm_path),
                Some(token),
                Some(publish_document(
                    &row.npm_display_name(),
                    row.package,
                    row.versions,
                )),
            )
            .await;
            assert_eq!(
                published.status, 200,
                "case {case:?}: the npm publish must succeed, got {published:?}"
            );
        }
        Seeded::OriginConfigurationOnly => {
            let created = put_origin_configuration(router, row, repository, "ALLOW", "BLOCK").await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: PutPackageOriginConfiguration must succeed, got {created:?}"
            );
        }
    }
}

/// The domain, its two repositories and every row seeded into [`REPOSITORY`]; the row the
/// isolation rule needs is seeded into [`OTHER_REPOSITORY`] too.
async fn given_repositories(router: &MultiServiceRouter) {
    assert_eq!(
        create_domain(router, DOMAIN, None).await.status,
        200,
        "setup: CreateDomain must succeed"
    );
    for repository in [REPOSITORY, OTHER_REPOSITORY] {
        let created = create_repository(router, DOMAIN, repository, None, &[]).await;
        assert_eq!(
            created.status, 200,
            "setup: CreateRepository {repository:?} must succeed, got {created:?}"
        );
    }
    let token = crate::npm_registry_endpoint::token_for(router, DOMAIN).await;
    for row in &ROWS {
        given_package(router, row, REPOSITORY, &token).await;
    }
    // The same coordinate as the first row, in the other repository.
    given_package(router, &ROWS[0], OTHER_REPOSITORY, &token).await;
}

/// The `PackageSummary` the delete must answer with for `row`, read off the package as the
/// LAST read before the call reported it: the same projection, with `name` spelled
/// `package` as `PackageSummary` spells it
/// (`smithy_query(query="shape",shape="PackageSummary")`).
fn expected_summary(described: &Value) -> Value {
    let mut summary = json!({
        "format": described["format"].clone(),
        "package": described["name"].clone(),
        "originConfiguration": described["originConfiguration"].clone(),
    });
    if let Some(namespace) = described.get("namespace") {
        if !namespace.is_null() {
            summary["namespace"] = namespace.clone();
        }
    }
    summary
}

/// The total size of every asset the versions of `row` hold in `repository`, summed off the
/// listings a client reads — which is what the domain's derived `assetSizeBytes` counts.
async fn held_asset_bytes(router: &MultiServiceRouter, row: &Row, repository: &str) -> i64 {
    let case = row.case;
    let mut total = 0i64;
    for version in row.versions {
        let listed = list_assets(router, row, repository, version).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListPackageVersionAssets {version} must succeed, got {listed:?}"
        );
        for asset in listed.body["assets"].as_array().into_iter().flatten() {
            total += asset["size"].as_i64().unwrap_or_default();
        }
    }
    total
}

/// RULES 1, 2, 3 and 4: one delete per row, each asserted against the state before and
/// after it.
#[tokio::test]
async fn deleting_a_package_removes_it_with_every_version_it_held() {
    let router = emulator();
    given_repositories(&router).await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;

        // The last read before the delete: what the answer has to report, and what the
        // domain's derived size has to drop by.
        let read = describe_package(&router, row, REPOSITORY).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the seeded package must be readable, got {read:?}"
        );
        let expected = expected_summary(&read.body["package"]);
        let held_bytes = held_asset_bytes(&router, row, REPOSITORY).await;
        let bytes_before = domain_asset_bytes(&router, case).await;

        // RULE 2: the delete answers with the record it removed.
        let deleted = delete_package(&router, &row.query(REPOSITORY)).await;
        assert_eq!(
            (deleted.status, deleted.body["deletedPackage"].clone()),
            (200, expected.clone()),
            "case {case:?}: expected the removed package reported as {expected}, got {deleted:?}"
        );

        // RULE 1: the package is gone, and so is every version it held.
        let gone = describe_package(&router, row, REPOSITORY).await;
        assert_eq!(
            (gone.status, gone.code(), gone.message()),
            (
                404,
                "ResourceNotFoundException".to_string(),
                row.package_family_message()
            ),
            "case {case:?}: a deleted package must be gone from DescribePackage, got {gone:?}"
        );
        let listed = list_versions(&router, row, REPOSITORY).await;
        assert_eq!(
            (listed.status, listed.code(), listed.message()),
            (
                404,
                "ResourceNotFoundException".to_string(),
                row.version_family_message(REPOSITORY)
            ),
            "case {case:?}: ListPackageVersions must not answer for a deleted package, got \
             {listed:?}"
        );
        for version in row.versions {
            let version_gone = describe_version(&router, row, REPOSITORY, version).await;
            assert_eq!(
                (version_gone.status, version_gone.code()),
                (404, "ResourceNotFoundException".to_string()),
                "case {case:?}: version {version} must go with the package, got {version_gone:?}"
            );
        }
        let coordinates = held_coordinates(&router, REPOSITORY, case).await;
        assert!(
            !coordinates.contains(&coordinate_key(row)),
            "case {case:?}: a deleted package must be gone from ListPackages, got \
             {coordinates:?}"
        );

        // RULE 4: the domain's derived size dropped by exactly what the package held.
        let bytes_after = domain_asset_bytes(&router, case).await;
        assert_eq!(
            bytes_after,
            bytes_before - held_bytes,
            "case {case:?}: expected assetSizeBytes to drop by {held_bytes} from \
             {bytes_before}, got {bytes_after}"
        );

        // RULE 3: nothing else moved. Every row not yet deleted still reads back with its
        // versions, including the two that share this row's package NAME under another
        // coordinate.
        for other in ROWS.iter().skip(index + 1) {
            let survivor = describe_package(&router, other, REPOSITORY).await;
            assert_eq!(
                survivor.status, 200,
                "case {case:?}: {:?} must survive the delete, got {survivor:?}",
                other.case
            );
            for version in other.versions {
                let held = describe_version(&router, other, REPOSITORY, version).await;
                assert_eq!(
                    held.status, 200,
                    "case {case:?}: {:?} version {version} must survive, got {held:?}",
                    other.case
                );
            }
        }

        // RULE 3: and the same coordinate in the OTHER repository is untouched.
        let elsewhere = describe_package(&router, &ROWS[0], OTHER_REPOSITORY).await;
        assert_eq!(
            elsewhere.status, 200,
            "case {case:?}: the copy in {OTHER_REPOSITORY:?} must survive, got {elsewhere:?}"
        );
        for version in ROWS[0].versions {
            let held = describe_version(&router, &ROWS[0], OTHER_REPOSITORY, version).await;
            assert_eq!(
                held.status, 200,
                "case {case:?}: the copy in {OTHER_REPOSITORY:?} must keep version {version}, \
                 got {held:?}"
            );
        }
    }
}

/// RULE 1: the identifier a delete freed can be used again — the package record is gone,
/// not emptied, so the coordinate can be created afresh with its own configuration and
/// holds no version of the package that was deleted.
#[tokio::test]
async fn a_deleted_coordinate_can_be_held_again() {
    let router = emulator();
    given_repositories(&router).await;

    for row in &ROWS {
        let case = row.case;
        assert_eq!(
            delete_package(&router, &row.query(REPOSITORY)).await.status,
            200,
            "case {case:?}: the delete must succeed"
        );
        let recreated = put_origin_configuration(&router, row, REPOSITORY, "BLOCK", "ALLOW").await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the freed coordinate must be holdable again, got {recreated:?}"
        );
        let read = describe_package(&router, row, REPOSITORY).await;
        assert_eq!(
            (
                read.status,
                read.body["package"]["originConfiguration"]["restrictions"].clone()
            ),
            (200, json!({ "publish": "BLOCK", "upstream": "ALLOW" })),
            "case {case:?}: the new record must carry its own configuration, got {read:?}"
        );
        let listed = list_versions(&router, row, REPOSITORY).await;
        assert_eq!(
            (listed.status, listed.body["versions"].clone()),
            (200, json!([])),
            "case {case:?}: the new record must hold none of the deleted versions, got \
             {listed:?}"
        );
    }
}

/// RULE 5: an absence that is not the target's is a refusal, and the refusals name what
/// they could not resolve. Nothing is removed by any of them.
#[tokio::test]
async fn an_unresolvable_target_or_scope_is_refused_and_removes_nothing() {
    let router = emulator();
    given_repositories(&router).await;

    // The package-scoped sentence, for a package the repository does not hold: a coordinate
    // never seeded, and a seeded row addressed under the WRONG format or namespace — which
    // addresses a different package.
    let absent: Vec<Row> = vec![
        Row {
            case: "a coordinate never held",
            package: "no-such-package",
            ..ROWS[0].clone()
        },
        Row {
            case: "a held name under another format",
            format: "nuget",
            namespace: None,
            ..ROWS[2].clone()
        },
        Row {
            case: "a held name under another namespace",
            namespace: Some("other"),
            ..ROWS[3].clone()
        },
        Row {
            case: "a held name with its namespace omitted",
            namespace: None,
            ..ROWS[0].clone()
        },
    ];
    for row in &absent {
        let case = row.case;
        let refused = delete_package(&router, &row.query(REPOSITORY)).await;
        assert_eq!(
            (refused.status, refused.code(), refused.message()),
            (
                404,
                "ResourceNotFoundException".to_string(),
                row.package_family_message()
            ),
            "case {case:?}: an absent package must be refused with the package-scoped \
             sentence, got {refused:?}"
        );
        assert_eq!(
            refused.body["resourceType"].as_str(),
            Some("package"),
            "case {case:?}: the refusal must name the kind it could not resolve, got {refused:?}"
        );
    }

    // The SCOPE: an unknown domain, a domain pinned to an account that does not own it, and
    // an unknown repository.
    assert_ne!(
        FOREIGN_ACCOUNT,
        crate::harness::ACCOUNT,
        "the wrong-owner row needs an account the emulator does not present"
    );
    let scopes = [
        (
            "a domain that is not held",
            format!(
                "domain=no-such-domain&repository={REPOSITORY}&format=npm&package={}",
                ROWS[2].package
            ),
            "domain",
        ),
        (
            "a domain pinned to another account",
            format!("{}&domain-owner={FOREIGN_ACCOUNT}", ROWS[2].query(REPOSITORY)),
            "domain",
        ),
        (
            "a repository that is not held",
            ROWS[2].query("NoSuchRepo"),
            "repository",
        ),
    ];
    for (case, query, resource_type) in scopes {
        let refused = delete_package(&router, &query).await;
        assert_eq!(
            (refused.status, refused.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: an unresolvable scope must be not found, got {refused:?}"
        );
        assert_eq!(
            refused.body["resourceType"].as_str(),
            Some(resource_type),
            "case {case:?}: the refusal must name the kind it could not resolve, got {refused:?}"
        );
    }

    // None of those calls removed anything: every seeded row still reads back with its
    // versions.
    for row in &ROWS {
        let case = row.case;
        let read = describe_package(&router, row, REPOSITORY).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a refused delete must remove nothing, got {read:?}"
        );
        for version in row.versions {
            let held = describe_version(&router, row, REPOSITORY, version).await;
            assert_eq!(
                held.status, 200,
                "case {case:?}: version {version} must survive a refused delete, got {held:?}"
            );
        }
    }
}

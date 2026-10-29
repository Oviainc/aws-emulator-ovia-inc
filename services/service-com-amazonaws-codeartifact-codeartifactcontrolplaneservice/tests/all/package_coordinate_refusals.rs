// SPDX-License-Identifier: CC0-1.0

//! Which sentence a package the addressed repository does not hold is refused with, over
//! the wire.
//!
//! Every case drives the production front door — a `MultiServiceRouter` with this service
//! mounted — against a repository that exists and holds nothing, so the only thing under
//! test is the refusal.
//!
//! The rules under test, each over a TABLE rather than one example:
//!
//! 1. Every operation that addresses a package's VERSIONS — one version, a batch of them or
//!    all of them — refuses a package the addressed repository does not hold with
//!    `ResourceNotFoundException` (404) whose sentence names the FORMAT, the package (and its
//!    namespace where the request carried one) and the REPOSITORY the versions were looked
//!    for in. `CopyPackageVersions` names its SOURCE repository, the one the versions were
//!    looked for in.
//! 2. The PACKAGE-scoped `DescribePackage` states the other sentence for the same absence —
//!    the `(format, namespace, name)` triple with no repository — so the two families stay
//!    distinguishable.
//! 3. Both sentences are built from the REQUEST: the rows differ along the format, along
//!    whether the coordinate carries a namespace, and along the package and repository names.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer};

/// The domain every case below lives in.
const DOMAIN: &str = "coordinate-refusals";

/// The repository every request addresses: it exists, and it holds nothing.
const REPOSITORY: &str = "empty-repo";

/// A second repository, so a copy can name a destination that is not the source.
const DESTINATION: &str = "copy-destination";

/// A version every request names. Nothing is held, so which version it is never decides the
/// answer — the PACKAGE is what is missing.
const VERSION: &str = "1.0.0";

/// One coordinate a request addresses: a format, the namespace the request carries (or not)
/// and the package name.
#[derive(Debug)]
struct Coordinate {
    case: &'static str,
    format: &'static str,
    namespace: Option<&'static str>,
    package: &'static str,
}

impl Coordinate {
    /// The query members that identify the PACKAGE itself for this coordinate — everything
    /// but the repository, which each operation names in its own member.
    fn members(&self) -> String {
        let mut query = format!("domain={DOMAIN}&format={}", self.format);
        if let Some(namespace) = self.namespace {
            query.push_str(&format!("&namespace={namespace}"));
        }
        query.push_str(&format!("&package={}", self.package));
        query
    }

    /// The query every repository-addressing operation carries for this coordinate.
    fn query(&self, repository: &str) -> String {
        format!("{}&repository={repository}", self.members())
    }

    /// RULE 1: the sentence the VERSION-addressing operations state for this coordinate,
    /// against `repository`.
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

    /// RULE 2: the sentence the PACKAGE-scoped operations state for this coordinate.
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
}

/// The coordinates every rule is asserted over: the rows differ along the format, along
/// whether the coordinate carries a namespace (`npm` writes its as a scope, `maven` and
/// `generic` as a coordinate of their own) and along the package name.
const COORDINATES: [Coordinate; 4] = [
    Coordinate {
        case: "npm, no namespace",
        format: "npm",
        namespace: None,
        package: "left-side",
    },
    Coordinate {
        case: "npm, scoped",
        format: "npm",
        namespace: Some("acme"),
        package: "widgets",
    },
    Coordinate {
        case: "maven, namespaced",
        format: "maven",
        namespace: Some("com.acme"),
        package: "widget-core",
    },
    Coordinate {
        case: "generic, namespaced",
        format: "generic",
        namespace: Some("acme"),
        package: "release.bundle",
    },
];

/// One VERSION-addressing operation, as the wire reaches it.
#[derive(Debug)]
struct VersionOperation {
    name: &'static str,
    method: &'static str,
    path: &'static str,
    /// The query members this operation carries beyond the coordinate's own.
    extra_query: &'static str,
    /// Whether this operation takes a body, and what it carries beyond nothing.
    body: Option<fn() -> Value>,
    /// Whether this operation names its SOURCE and DESTINATION repositories rather than one
    /// addressed repository: a copy reads from the source, so that is the repository its
    /// refusal names.
    names_source: bool,
}

/// The versions a batch operation asks about.
fn batch_versions() -> Value {
    json!({ "versions": [VERSION] })
}

/// The body `UpdatePackageVersionsStatus` requires: the versions and the status to move
/// them to.
fn status_move() -> Value {
    json!({ "versions": [VERSION], "targetStatus": "Archived" })
}

/// An empty JSON object, which the listing operations take as their body.
fn empty_object() -> Value {
    json!({})
}

/// Every operation that addresses a package's VERSIONS: one version, a batch, or all of
/// them.
const VERSION_OPERATIONS: [VersionOperation; 9] = [
    VersionOperation {
        name: "ListPackageVersions",
        method: "POST",
        path: "/v1/package/versions",
        extra_query: "",
        body: Some(empty_object),
        names_source: false,
    },
    VersionOperation {
        name: "ListPackageVersionAssets",
        method: "POST",
        path: "/v1/package/version/assets",
        extra_query: "&version=1.0.0",
        body: Some(empty_object),
        names_source: false,
    },
    VersionOperation {
        name: "ListPackageVersionDependencies",
        method: "POST",
        path: "/v1/package/version/dependencies",
        extra_query: "&version=1.0.0",
        body: Some(empty_object),
        names_source: false,
    },
    VersionOperation {
        name: "DescribePackageVersion",
        method: "GET",
        path: "/v1/package/version",
        extra_query: "&version=1.0.0",
        body: None,
        names_source: false,
    },
    VersionOperation {
        name: "GetPackageVersionAsset",
        method: "GET",
        path: "/v1/package/version/asset",
        extra_query: "&version=1.0.0&asset=package.tgz",
        body: None,
        names_source: false,
    },
    VersionOperation {
        name: "DeletePackageVersions",
        method: "POST",
        path: "/v1/package/versions/delete",
        extra_query: "",
        body: Some(batch_versions),
        names_source: false,
    },
    VersionOperation {
        name: "DisposePackageVersions",
        method: "POST",
        path: "/v1/package/versions/dispose",
        extra_query: "",
        body: Some(batch_versions),
        names_source: false,
    },
    VersionOperation {
        name: "UpdatePackageVersionsStatus",
        method: "POST",
        path: "/v1/package/versions/update_status",
        extra_query: "",
        body: Some(status_move),
        names_source: false,
    },
    VersionOperation {
        name: "CopyPackageVersions",
        method: "POST",
        path: "/v1/package/versions/copy",
        extra_query: "",
        body: Some(batch_versions),
        names_source: true,
    },
];

/// The domain and the two repositories every case addresses.
async fn given_repositories(router: &MultiServiceRouter) {
    assert_eq!(
        create_domain(router, DOMAIN, None).await.status,
        200,
        "CreateDomain must succeed"
    );
    for repository in [REPOSITORY, DESTINATION] {
        let created = create_repository(router, DOMAIN, repository, None, &[]).await;
        assert_eq!(
            created.status, 200,
            "CreateRepository {repository:?} must succeed, got {created:?}"
        );
    }
}

/// Send one version-addressing operation for one coordinate.
async fn refused(
    router: &MultiServiceRouter,
    operation: &VersionOperation,
    coordinate: &Coordinate,
) -> Answer {
    let addressing = if operation.names_source {
        // A copy addresses the source it reads from and the destination it writes to; it
        // carries no `repository` member of its own.
        format!("source-repository={REPOSITORY}&destination-repository={DESTINATION}")
    } else {
        format!("repository={REPOSITORY}")
    };
    let uri = format!(
        "{}?{}&{addressing}{}",
        operation.path,
        coordinate.members(),
        operation.extra_query
    );
    call(router, operation.method, &uri, operation.body.map(|body| body())).await
}

/// RULE 1 and RULE 3: every version-addressing operation refuses an absent package with the
/// repository-qualified sentence, built from the request.
#[tokio::test]
async fn every_version_addressing_operation_names_the_repository_it_looked_in() {
    let router = emulator();
    given_repositories(&router).await;

    for operation in &VERSION_OPERATIONS {
        for coordinate in &COORDINATES {
            let case = format!("{}/{}", operation.name, coordinate.case);
            let answer = refused(&router, operation, coordinate).await;
            assert_eq!(
                (answer.status, answer.code()),
                (404, "ResourceNotFoundException".to_string()),
                "case {case:?}: expected the modelled 404, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                coordinate.version_family_message(REPOSITORY),
                "case {case:?}: expected the repository-qualified sentence, got {:?}",
                answer.message()
            );
        }
    }
}

/// RULE 2 and RULE 3: the package-scoped `DescribePackage` states the other sentence for the
/// same absence, so one wording is never served for the other.
#[tokio::test]
async fn the_package_scoped_describe_states_the_package_triple_sentence() {
    let router = emulator();
    given_repositories(&router).await;

    for coordinate in &COORDINATES {
        let case = coordinate.case;
        let answer = call(
            &router,
            "GET",
            &format!("/v1/package?{}", coordinate.query(REPOSITORY)),
            None,
        )
        .await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: expected the modelled 404, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            coordinate.package_family_message(),
            "case {case:?}: expected the package-triple sentence, got {:?}",
            answer.message()
        );
        // The two families are different sentences for the same absence, which is the point
        // of asserting both here.
        assert_ne!(
            answer.message(),
            coordinate.version_family_message(REPOSITORY),
            "case {case:?}: the package-scoped sentence must not be the version-addressing one"
        );
    }
}

/// RULE 1: a repository that HOLDS the package is not refused at all — the sentence is about
/// an absence, not about the operation.
#[tokio::test]
async fn a_package_the_repository_holds_is_listed_rather_than_refused() {
    let router = emulator();
    given_repositories(&router).await;

    for coordinate in &COORDINATES {
        let case = coordinate.case;
        // `PutPackageOriginConfiguration` creates the package record with no versions, which
        // is the one way a repository holds a package and no version of it.
        let created = call(
            &router,
            "POST",
            &format!("/v1/package?{}", coordinate.query(REPOSITORY)),
            Some(json!({ "restrictions": { "publish": "ALLOW", "upstream": "ALLOW" } })),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: PutPackageOriginConfiguration must succeed, got {created:?}"
        );
        let listed = call(
            &router,
            "POST",
            &format!("/v1/package/versions?{}", coordinate.query(REPOSITORY)),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected the held package to list, got {listed:?}"
        );
        assert_eq!(
            listed.body.get("versions"),
            Some(&json!([])),
            "case {case:?}: expected an empty version list, got {listed:?}"
        );
    }
}

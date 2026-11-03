// SPDX-License-Identifier: CC0-1.0

//! Which sentence a package VERSION the addressed repository holds no record of is refused
//! with, over the wire.
//!
//! Every case drives the production front door — a `MultiServiceRouter` with this service
//! mounted — against a repository that EXISTS and holds the package but no version of the
//! name asked for, so the only thing under test is the refusal a version-absent lookup
//! gets. (A refusal for an absent PACKAGE is a different sentence, pinned by
//! `tests/all/package_coordinate_refusals.rs`.)
//!
//! The rules under test, each asserted over a TABLE rather than one example:
//!
//! 1. Every refusal for a version the addressed repository holds no record of names the
//!    repository by the opaque internal id this service knows it by — never by the name the
//!    request addressed it with — and renders the whole addressed coordinate as a
//!    `PackageVersionCoordinateEntity`: the package name, the `PackageFormat` upper-cased,
//!    the namespace where the coordinate carries one, the version, and the coordinate's
//!    `displayName`. It is one sentence, stated identically by the operations that refuse
//!    the lookup outright (`ResourceNotFoundException`, 404) and by the per-version
//!    `NOT_FOUND` entry of the batch operations (200 with a `failedVersions` map).
//! 2. The id is a property of the REPOSITORY, not of the request: every operation reports
//!    the same id for one repository, two repositories of one domain report two ids, and
//!    the same repository name in another domain is another id. `CopyPackageVersions`
//!    reports its SOURCE repository's id, because the source is where the versions were
//!    looked for.
//! 3. The sentence is built from the REQUEST's coordinate: the rows differ along the
//!    format, along whether the coordinate carries a namespace (so the entity carries a
//!    `namespace=` component and its `displayName` is the namespace path, against the
//!    version-pinned `<package>==<version>` form of a coordinate with none), and along the
//!    package and version names.
//! 4. The version a publish was refused for is exactly such an absent version: a package
//!    whose origin configuration blocks publishing stores nothing, so a later read of that
//!    version is this refusal — while the version the repository DOES hold still downloads
//!    byte for byte.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer};
use crate::package_version_publish::{binary_content, publish, Publish};

/// The domain every case below lives in.
const DOMAIN: &str = "version-refusals";

/// A second domain, so the SAME repository name can be held twice and the two ids
/// compared.
const OTHER_DOMAIN: &str = "version-refusals-two";

/// The repository every request addresses: it holds each coordinate's package, and no
/// version of it.
const REPOSITORY: &str = "held-repo";

/// A second repository in [`DOMAIN`], so two repositories of one domain can be compared.
const SIBLING_REPOSITORY: &str = "sibling-repo";

/// The destination a copy writes to — it is never where the versions are looked for.
const DESTINATION: &str = "copy-destination";

/// One coordinate a request addresses: the format, the namespace it carries (or not), the
/// package name and the version asked for. No repository holds any of these versions.
#[derive(Debug)]
struct Coordinate {
    case: &'static str,
    format: &'static str,
    namespace: Option<&'static str>,
    package: &'static str,
    version: &'static str,
}

impl Coordinate {
    /// The query members that identify the PACKAGE itself inside `domain` — everything but
    /// the repository, which each operation names in its own member.
    fn members(&self, domain: &str) -> String {
        let mut query = format!("domain={domain}&format={}", self.format);
        if let Some(namespace) = self.namespace {
            query.push_str(&format!("&namespace={namespace}"));
        }
        query.push_str(&format!("&package={}", self.package));
        query
    }

    /// RULE 1 and RULE 3: the sentence every version-absent refusal states for this
    /// coordinate in the repository `repository_id` names.
    fn expected_message(&self, repository_id: &str) -> String {
        format!(
            "Package version not found in repo. repoId = '{repository_id}', \
             packageCoordinate = '{}'.",
            self.expected_entity()
        )
    }

    /// The `PackageVersionCoordinateEntity` rendering of this coordinate: the namespace
    /// component appears exactly where the coordinate carries one, and the format is
    /// upper-cased.
    fn expected_entity(&self) -> String {
        let format = self.format.to_uppercase();
        match self.namespace {
            Some(namespace) => format!(
                "PackageVersionCoordinateEntity{{name={}, type={format}, namespace={namespace}, \
                 version={}, displayName={}}}",
                self.package,
                self.version,
                self.expected_display_name()
            ),
            None => format!(
                "PackageVersionCoordinateEntity{{name={}, type={format}, version={}, \
                 displayName={}}}",
                self.package,
                self.version,
                self.expected_display_name()
            ),
        }
    }

    /// The `displayName` of this coordinate: the `<namespace>/<package>/<version>` path for
    /// a coordinate that carries a namespace, and the version-pinned `<package>==<version>`
    /// for one that carries none.
    fn expected_display_name(&self) -> String {
        match self.namespace {
            Some(namespace) => format!("{namespace}/{}/{}", self.package, self.version),
            None => format!("{}=={}", self.package, self.version),
        }
    }
}

/// The coordinates every rule is asserted over. The rows differ along the format, along
/// whether the coordinate carries a namespace (`generic`, `maven` and `swift` versions
/// carry one; `pypi`, `nuget`, `ruby` and `cargo` versions carry none —
/// `smithy_query(query="doc",shape="PackageVersionDescription",member="namespace")`), and
/// along the package and version names.
const COORDINATES: [Coordinate; 5] = [
    Coordinate {
        case: "generic, namespaced",
        format: "generic",
        namespace: Some("sim-ns"),
        package: "release.bundle",
        version: "2.0.0",
    },
    Coordinate {
        case: "maven, namespaced",
        format: "maven",
        namespace: Some("com.acme"),
        package: "widget-core",
        version: "1.4.2-SNAPSHOT",
    },
    Coordinate {
        case: "npm, scoped",
        format: "npm",
        namespace: Some("acme"),
        package: "widgets",
        version: "0.0.1-rc.1",
    },
    Coordinate {
        case: "pypi, no namespace",
        format: "pypi",
        namespace: None,
        package: "six",
        version: "1.16.0",
    },
    Coordinate {
        case: "nuget, no namespace",
        format: "nuget",
        namespace: None,
        package: "Newtonsoft.Json",
        version: "13.0.3",
    },
];

/// One operation that addresses ONE named version of a package, as the wire reaches it.
#[derive(Debug)]
struct VersionOperation {
    name: &'static str,
    method: &'static str,
    path: &'static str,
    /// The query members this operation carries beyond the coordinate's own; `{version}`
    /// stands for the version asked for.
    extra_query: &'static str,
    /// The body it takes, built around the version asked for.
    body: Option<fn(&str) -> Value>,
    /// Whether it answers 200 with a per-version `failedVersions` map rather than refusing
    /// the whole call.
    per_version: bool,
    /// Whether it names its SOURCE and DESTINATION repositories rather than one addressed
    /// repository.
    names_source: bool,
}

fn batch_versions(version: &str) -> Value {
    json!({ "versions": [version] })
}

fn status_move(version: &str) -> Value {
    json!({ "versions": [version], "targetStatus": "Archived" })
}

fn empty_object(_version: &str) -> Value {
    json!({})
}

/// Every operation that addresses one named version: the three that refuse the lookup
/// outright and the four that report it per version.
const VERSION_OPERATIONS: [VersionOperation; 7] = [
    VersionOperation {
        name: "GetPackageVersionAsset",
        method: "GET",
        path: "/v1/package/version/asset",
        extra_query: "&version={version}&asset=payload.bin",
        body: None,
        per_version: false,
        names_source: false,
    },
    VersionOperation {
        name: "ListPackageVersionAssets",
        method: "POST",
        path: "/v1/package/version/assets",
        extra_query: "&version={version}",
        body: Some(empty_object),
        per_version: false,
        names_source: false,
    },
    VersionOperation {
        name: "ListPackageVersionDependencies",
        method: "POST",
        path: "/v1/package/version/dependencies",
        extra_query: "&version={version}",
        body: Some(empty_object),
        per_version: false,
        names_source: false,
    },
    VersionOperation {
        name: "DeletePackageVersions",
        method: "POST",
        path: "/v1/package/versions/delete",
        extra_query: "",
        body: Some(batch_versions),
        per_version: true,
        names_source: false,
    },
    VersionOperation {
        name: "DisposePackageVersions",
        method: "POST",
        path: "/v1/package/versions/dispose",
        extra_query: "",
        body: Some(batch_versions),
        per_version: true,
        names_source: false,
    },
    VersionOperation {
        name: "UpdatePackageVersionsStatus",
        method: "POST",
        path: "/v1/package/versions/update_status",
        extra_query: "",
        body: Some(status_move),
        per_version: true,
        names_source: false,
    },
    VersionOperation {
        name: "CopyPackageVersions",
        method: "POST",
        path: "/v1/package/versions/copy",
        extra_query: "",
        body: Some(batch_versions),
        per_version: true,
        names_source: true,
    },
];

/// Hold `domain` and each of `repositories` in it.
async fn given_repositories(router: &MultiServiceRouter, domain: &str, repositories: &[&str]) {
    let created = create_domain(router, domain, None).await;
    assert_eq!(
        created.status, 200,
        "CreateDomain {domain:?} must succeed, got {created:?}"
    );
    for repository in repositories {
        let created = create_repository(router, domain, repository, None, &[]).await;
        assert_eq!(
            created.status, 200,
            "CreateRepository {repository:?} must succeed, got {created:?}"
        );
    }
}

/// Hold `coordinate`'s PACKAGE in `repository`, with no version of it:
/// `PutPackageOriginConfiguration` is documented to create exactly that for a package the
/// repository does not yet hold, which is what makes the VERSION the only thing missing.
async fn given_package(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    coordinate: &Coordinate,
) {
    let created = call(
        router,
        "POST",
        &format!(
            "/v1/package?{}&repository={repository}",
            coordinate.members(domain)
        ),
        Some(json!({ "restrictions": { "publish": "ALLOW", "upstream": "ALLOW" } })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: PutPackageOriginConfiguration must succeed, got {created:?}",
        coordinate.case
    );
}

/// Ask `operation` about `coordinate`'s version in `repository`.
async fn ask(
    router: &MultiServiceRouter,
    operation: &VersionOperation,
    domain: &str,
    repository: &str,
    coordinate: &Coordinate,
) -> Answer {
    let addressing = if operation.names_source {
        // A copy addresses the source it reads from and the destination it writes to; it
        // carries no `repository` member of its own.
        format!("source-repository={repository}&destination-repository={DESTINATION}")
    } else {
        format!("repository={repository}")
    };
    let uri = format!(
        "{}?{}&{addressing}{}",
        operation.path,
        coordinate.members(domain),
        operation.extra_query.replace("{version}", coordinate.version)
    );
    call(
        router,
        operation.method,
        &uri,
        operation.body.map(|body| body(coordinate.version)),
    )
    .await
}

/// The sentence `answer` states for the absent version, asserting the error identity it
/// rides first: the modelled 404 for the operations that refuse the call, and the
/// per-version `NOT_FOUND` entry for the batch operations.
fn stated_message(
    answer: &Answer,
    operation: &VersionOperation,
    version: &str,
    case: &str,
) -> String {
    if !operation.per_version {
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: expected the modelled 404, got {answer:?}"
        );
        return answer.message();
    }
    assert_eq!(
        (answer.status, answer.code()),
        (200, String::new()),
        "case {case:?}: expected a per-version answer, got {answer:?}"
    );
    assert_eq!(
        answer.body["failedVersions"][version]["errorCode"].as_str(),
        Some("NOT_FOUND"),
        "case {case:?}: expected a NOT_FOUND entry for {version}, got {answer:?}"
    );
    answer.body["failedVersions"][version]["errorMessage"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `repoId` `message` names, parsed strictly: the sentence has to open with the fixed
/// text, and the id has to be the opaque 8-4-4-4-12 hex id this service knows a repository
/// by rather than anything a caller sent.
fn repo_id_of(message: &str, case: &str) -> String {
    let opened = message
        .strip_prefix("Package version not found in repo. repoId = '")
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the version-not-found sentence, got {message:?}")
        });
    let (id, _rest) = opened.split_once('\'').unwrap_or_else(|| {
        panic!("case {case:?}: expected a quoted repoId, got {message:?}")
    });
    let groups: Vec<usize> = id.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected an opaque 8-4-4-4-12 repoId, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "case {case:?}: expected a hex repoId, got {id:?}"
    );
    id.to_string()
}

// ---------------------------------------------------------------------------
// Rules 1 and 3: one sentence, built from the request's coordinate.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_version_addressing_operation_states_the_coordinate_entity() {
    let router = emulator();
    given_repositories(&router, DOMAIN, &[REPOSITORY, DESTINATION]).await;
    for coordinate in &COORDINATES {
        given_package(&router, DOMAIN, REPOSITORY, coordinate).await;
    }

    // The one id every answer below must name, taken from the first answer and then
    // required of all the others: the id is the repository's, so no operation and no
    // coordinate may report another.
    let mut named_id: Option<String> = None;
    for operation in &VERSION_OPERATIONS {
        for coordinate in &COORDINATES {
            let case = format!("{}/{}", operation.name, coordinate.case);
            let answer = ask(&router, operation, DOMAIN, REPOSITORY, coordinate).await;
            let message = stated_message(&answer, operation, coordinate.version, &case);
            let id = repo_id_of(&message, &case);
            assert!(
                !message.contains(REPOSITORY),
                "case {case:?}: the sentence names the repository by its id, not by the name \
                 the request used, got {message:?}"
            );
            assert_eq!(
                message,
                coordinate.expected_message(&id),
                "case {case:?}: expected the coordinate-entity sentence, got {message:?}"
            );
            let expected_id = named_id.get_or_insert_with(|| id.clone());
            assert_eq!(
                &id, expected_id,
                "case {case:?}: every operation must name one id for {REPOSITORY}, got {id:?}"
            );
        }
    }
    assert!(
        named_id.is_some(),
        "the table must have produced at least one refusal"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the id belongs to the repository the versions were looked for in.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_repository_is_named_by_its_own_stable_id() {
    let router = emulator();
    given_repositories(&router, DOMAIN, &[REPOSITORY, SIBLING_REPOSITORY, DESTINATION]).await;
    // The SAME repository name in another domain: another repository, so another id.
    given_repositories(&router, OTHER_DOMAIN, &[REPOSITORY]).await;

    let coordinate = &COORDINATES[0];
    let read = &VERSION_OPERATIONS[0];
    let mut seen: Vec<(String, String)> = Vec::new();
    for (case, domain, repository) in [
        ("the addressed repository", DOMAIN, REPOSITORY),
        ("a sibling of the same domain", DOMAIN, SIBLING_REPOSITORY),
        ("the same name in another domain", OTHER_DOMAIN, REPOSITORY),
    ] {
        given_package(&router, domain, repository, coordinate).await;
        let answer = ask(&router, read, domain, repository, coordinate).await;
        let message = stated_message(&answer, read, coordinate.version, case);
        let id = repo_id_of(&message, case);
        // The same question asked twice reports the same id: it is derived from the
        // repository's identity, never minted per call.
        let again = ask(&router, read, domain, repository, coordinate).await;
        let again = stated_message(&again, read, coordinate.version, case);
        assert_eq!(
            again, message,
            "case {case:?}: the same lookup must state the same sentence, got {again:?}"
        );
        seen.push((case.to_string(), id));
    }

    for (left, right) in [(0, 1), (0, 2), (1, 2)] {
        assert_ne!(
            seen[left].1, seen[right].1,
            "{} and {} must report different ids, both reported {:?}",
            seen[left].0, seen[right].0, seen[left].1
        );
    }
}

#[tokio::test]
async fn a_copy_names_the_source_repository_it_looked_in() {
    let router = emulator();
    given_repositories(&router, DOMAIN, &[REPOSITORY, DESTINATION]).await;

    let coordinate = &COORDINATES[0];
    given_package(&router, DOMAIN, REPOSITORY, coordinate).await;
    given_package(&router, DOMAIN, DESTINATION, coordinate).await;

    // The id the source repository is known by, read off an operation that addresses it
    // directly.
    let read = &VERSION_OPERATIONS[0];
    let direct = ask(&router, read, DOMAIN, REPOSITORY, coordinate).await;
    let direct = stated_message(&direct, read, coordinate.version, "the source, addressed directly");
    let source_id = repo_id_of(&direct, "the source, addressed directly");
    let other = ask(&router, read, DOMAIN, DESTINATION, coordinate).await;
    let other = stated_message(&other, read, coordinate.version, "the destination");
    let destination_id = repo_id_of(&other, "the destination");

    let copy = &VERSION_OPERATIONS[6];
    assert_eq!(
        copy.name, "CopyPackageVersions",
        "the copy row must be the one asserted here"
    );
    let answer = ask(&router, copy, DOMAIN, REPOSITORY, coordinate).await;
    let message = stated_message(&answer, copy, coordinate.version, copy.name);
    assert_eq!(
        repo_id_of(&message, copy.name),
        source_id,
        "a copy must name the SOURCE repository it looked in, got {message:?}"
    );
    assert_ne!(
        repo_id_of(&message, copy.name),
        destination_id,
        "a copy must not name the destination it would have written to, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a publish origin control blocked stores nothing, so its version is absent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_version_a_blocked_publish_never_stored_reads_as_absent() {
    let router = emulator();
    given_repositories(&router, DOMAIN, &[REPOSITORY]).await;

    let namespace = "sim-ns";
    let package = "blocked-bundle";
    let held_content = binary_content(96, 7);
    let mut held = Publish::new(package, "1.0.0", "payload.bin", held_content.clone());
    held.domain = DOMAIN.to_string();
    held.repository = REPOSITORY.to_string();
    held = held.namespace(namespace);
    let published = publish(&router, &held).await;
    assert_eq!(
        published.status, 200,
        "the first publish must land, got {published:?}"
    );

    // Origin control now blocks new versions of this package.
    let blocked = call(
        &router,
        "POST",
        &format!(
            "/v1/package?domain={DOMAIN}&repository={REPOSITORY}&format=generic\
             &namespace={namespace}&package={package}"
        ),
        Some(json!({ "restrictions": { "publish": "BLOCK", "upstream": "ALLOW" } })),
    )
    .await;
    assert_eq!(
        blocked.status, 200,
        "PutPackageOriginConfiguration must succeed, got {blocked:?}"
    );

    let mut refused = held.clone();
    refused.version = "2.0.0".to_string();
    refused.content = binary_content(32, 9);
    let answer = publish(&router, &refused).await;
    assert_eq!(
        (answer.status, answer.code()),
        (400, "ValidationException".to_string()),
        "a blocked publish must be refused, got {answer:?}"
    );

    // The version was never stored, so reading its asset is the version-absent refusal.
    let absent = Coordinate {
        case: "the version a blocked publish never stored",
        format: "generic",
        namespace: Some("sim-ns"),
        package: "blocked-bundle",
        version: "2.0.0",
    };
    let read = &VERSION_OPERATIONS[0];
    let answer = ask(&router, read, DOMAIN, REPOSITORY, &absent).await;
    let message = stated_message(&answer, read, absent.version, absent.case);
    let id = repo_id_of(&message, absent.case);
    assert_eq!(
        message,
        absent.expected_message(&id),
        "case {:?}: expected the coordinate-entity sentence, got {message:?}",
        absent.case
    );

    // The version the repository DOES hold still downloads byte for byte, so the refusal
    // above is about that one version and not about the package.
    let download = call(
        &router,
        "GET",
        &format!(
            "/v1/package/version/asset?domain={DOMAIN}&repository={REPOSITORY}&format=generic\
             &namespace={namespace}&package={package}&version=1.0.0&asset=payload.bin"
        ),
        None,
    )
    .await;
    assert_eq!(
        download.status, 200,
        "the held version must still download, got {download:?}"
    );
}

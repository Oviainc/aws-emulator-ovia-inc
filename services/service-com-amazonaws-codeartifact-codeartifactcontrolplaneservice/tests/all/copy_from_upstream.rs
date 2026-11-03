// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for `CopyPackageVersions` with `includeFromUpstream`.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code, the two per-version maps and the copied
//! asset bytes — never a handler's return value.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one recorded example:
//! an operation asked to include the repositories upstream of the one it was addressed on
//! resolves the package and each named version through that repository's upstream chain,
//! nearest first, and acts on the first copy it finds; asked not to, it searches the
//! addressed repository's own records alone and refuses what it does not find there.
//!
//! 1. `includeFromUpstream: true` copies a version held only upstream of the source: HTTP
//!    200, one `successfulVersions` entry per requested version carrying the version's
//!    revision and `Published`, an empty `failedVersions`, and the copy is COMPLETE — the
//!    destination answers `DescribePackageVersion` and `GetPackageVersionAsset` with the
//!    upstream version's provenance, revision and exact asset bytes. Rows differ along the
//!    number of upstream hops between the source and the version (0, 1, 2) and along the
//!    number of versions the copy names (1, 2, 3).
//! 2. With the flag absent, `false`, or set on a source repository that reaches nothing,
//!    the whole call is `ResourceNotFoundException` (404) naming the SOURCE repository, and
//!    it materializes nothing in the destination.
//! 3. The chain is searched NEAREST FIRST: when two repositories of the chain hold the same
//!    version, the nearer one's record (its own asset bytes and provenance) is the one
//!    copied.
//! 4. An upstream-resolved copy still decides every named version on its own: a version no
//!    reachable repository holds is `NOT_FOUND` in `failedVersions` while its resolvable
//!    siblings are copied, a `versionRevisions` entry is a precondition on the resolved
//!    version's content (`MISMATCHED_REVISION` when it does not hold), and a version the
//!    destination already holds is `ALREADY_EXISTS` unless `allowOverwrite` is set.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{auth_for, call, create_domain, emulator, Answer, REGION};
use crate::package_version_publish::{
    binary_content, describe, publish, Publish, DOMAIN, NAMESPACE,
};

/// The repository every version below is PUBLISHED into: the back of every chain.
const BACK: &str = "upstream-back";

/// The middle repository of the chain — one hop from [`BACK`].
const MID: &str = "upstream-mid";

/// The front repository — two hops from [`BACK`].
const FRONT: &str = "upstream-front";

/// A repository with no upstream at all, which reaches nothing it does not hold.
const SOLO: &str = "upstream-solo";

/// The asset name every publish here carries.
const ASSET: &str = "payload.bin";

/// The `PackageVersionStatus` a published version carries, and the one every copy of it
/// carries too (`PackageVersionStatus` = [Published, Unfinished, Unlisted, Archived,
/// Disposed, Deleted]).
const PUBLISHED: &str = "Published";

/// `PackageVersionErrorCode$NOT_FOUND` / `$MISMATCHED_REVISION` / `$ALREADY_EXISTS`, the
/// per-version codes this suite asserts (`PackageVersionError$errorCode`).
const NOT_FOUND: &str = "NOT_FOUND";
const MISMATCHED_REVISION: &str = "MISMATCHED_REVISION";
const ALREADY_EXISTS: &str = "ALREADY_EXISTS";

/// A router holding [`DOMAIN`] with the chain `BACK <- MID <- FRONT`, a [`SOLO`]
/// repository that upstreams to nothing, and every destination in `destinations`.
async fn emulator_with_chain(destinations: &[&str]) -> MultiServiceRouter {
    let router = emulator();
    let created = create_domain(&router, DOMAIN, None).await;
    assert_eq!(created.status, 200, "CreateDomain failed: {created:?}");
    for (repository, upstreams) in [
        (BACK, &[] as &[&str]),
        (MID, &[BACK]),
        (FRONT, &[MID]),
        (SOLO, &[]),
    ] {
        let created = create_repository(&router, DOMAIN, repository, None, upstreams).await;
        assert_eq!(
            created.status, 200,
            "CreateRepository {repository:?} failed: {created:?}"
        );
    }
    for destination in destinations {
        let created = create_repository(&router, DOMAIN, destination, None, &[]).await;
        assert_eq!(
            created.status, 200,
            "CreateRepository {destination:?} failed: {created:?}"
        );
    }
    router
}

/// The publish of one version of `package` into `repository`, whose asset bytes are
/// derived from the coordinate so no two rows share content.
fn publication(repository: &str, package: &str, version: &str, seed: u8) -> Publish {
    Publish {
        repository: repository.to_string(),
        ..Publish::new(
            package,
            version,
            ASSET,
            binary_content(64 + usize::from(seed), seed),
        )
    }
}

/// `PublishPackageVersion`, asserted to succeed.
async fn given_published(router: &MultiServiceRouter, request: &Publish) {
    let answer = publish(router, request).await;
    assert_eq!(
        answer.status, 200,
        "publishing {}/{} into {} must succeed, got {answer:?}",
        request.package, request.version, request.repository
    );
}

/// `CopyPackageVersions` from `source` into `destination`, with the caller's own body —
/// which is what names the versions and carries `includeFromUpstream`.
async fn copy(
    router: &MultiServiceRouter,
    source: &str,
    destination: &str,
    package: &Publish,
    body: Value,
) -> Answer {
    let mut uri = format!(
        "/v1/package/versions/copy?domain={DOMAIN}&source-repository={source}\
         &destination-repository={destination}&format={}&package={}",
        package.format, package.package
    );
    if let Some(namespace) = &package.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    call(router, "POST", &uri, Some(body)).await
}

/// The `successfulVersions` map as `(version, status)` rows, sorted, asserting that every
/// entry reports a revision (the token a client pins content with).
fn successful(answer: &Answer, case: &str) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = answer
        .body
        .get("successfulVersions")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(version, info)| {
                    assert!(
                        info.get("revision")
                            .and_then(Value::as_str)
                            .is_some_and(|revision| !revision.is_empty()),
                        "case {case:?}: a copied version must report its revision, got {info:?}"
                    );
                    (
                        version.clone(),
                        info.get("status")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    rows.sort();
    rows
}

/// The `failedVersions` map as `(version, errorCode)` rows, sorted.
fn failed(answer: &Answer) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = answer
        .body
        .get("failedVersions")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(version, error)| {
                    (
                        version.clone(),
                        error
                            .get("errorCode")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    rows.sort();
    rows
}

/// The `errorMessage` `failedVersions` reports for one version.
fn failure_message(answer: &Answer, version: &str) -> String {
    answer.body["failedVersions"][version]["errorMessage"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `GetPackageVersionAsset` for the asset a [`Publish`] stored, read as RAW BYTES: the
/// asset is a streaming `httpPayload` blob, so the shared JSON [`call`] cannot express the
/// answer.
async fn download(router: &MultiServiceRouter, request: &Publish) -> (u16, Vec<u8>) {
    let mut uri = format!(
        "/v1/package/version/asset?domain={}&repository={}&format={}&package={}&version={}&asset={}",
        request.domain,
        request.repository,
        request.format,
        request.package,
        request.version,
        request.asset_name
    );
    if let Some(namespace) = &request.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    let built = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", format!("codeartifact.{REGION}.amazonaws.com"))
        .header("authorization", auth_for(REGION))
        .body(bytes::Bytes::new())
        .expect("build request");
    let response = router.dispatch(built).await;
    let status = response.status().as_u16();
    (status, response.into_body().to_vec())
}

/// `ListPackages` for one repository — how a client checks a refused copy left the
/// destination exactly as it was.
async fn listed_packages(router: &MultiServiceRouter, repository: &str) -> Vec<String> {
    let answer = call(
        router,
        "POST",
        &format!("/v1/packages?domain={DOMAIN}&repository={repository}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "ListPackages for {repository:?} must succeed, got {answer:?}"
    );
    answer.body["packages"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["package"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One row of RULE 1: the repository the copy is addressed on, how many upstream hops away
/// the version is, and the versions the copy names.
#[derive(Debug)]
struct Hop {
    case: &'static str,
    source: &'static str,
    /// The package this row publishes into [`BACK`], so rows do not share state.
    package: &'static str,
    versions: &'static [&'static str],
    destination: &'static str,
}

/// RULE 1: a copy that includes the upstreams resolves the version through the source
/// repository's chain and materializes it in the destination, completely.
#[tokio::test]
async fn an_upstream_including_copy_materializes_the_resolved_version_in_the_destination() {
    let rows: [Hop; 3] = [
        Hop {
            case: "the source repository holds the version itself",
            source: BACK,
            package: "hop-zero",
            versions: &["1.0.0"],
            destination: "dest-hop-zero",
        },
        Hop {
            case: "one hop upstream of the source",
            source: MID,
            package: "hop-one",
            versions: &["1.0.0", "2.0.0"],
            destination: "dest-hop-one",
        },
        Hop {
            case: "two hops upstream of the source",
            source: FRONT,
            package: "hop-two",
            versions: &["0.1.0", "1.2.3", "2.0.0"],
            destination: "dest-hop-two",
        },
    ];
    let destinations: Vec<&str> = rows.iter().map(|row| row.destination).collect();
    let router = emulator_with_chain(&destinations).await;

    for (seed, row) in rows.iter().enumerate() {
        let Hop {
            case,
            source,
            package,
            versions,
            destination,
        } = row;
        let published: Vec<Publish> = versions
            .iter()
            .enumerate()
            .map(|(index, version)| {
                publication(BACK, package, version, (seed * 8 + index) as u8)
            })
            .collect();
        for request in &published {
            given_published(&router, request).await;
        }

        let answer = copy(
            &router,
            source,
            destination,
            &published[0],
            json!({ "versions": versions, "includeFromUpstream": true }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {case:?}: expected the copy to answer 200, got {answer:?}"
        );
        let expected: Vec<(String, String)> = versions
            .iter()
            .map(|version| ((*version).to_string(), PUBLISHED.to_string()))
            .collect();
        assert_eq!(
            successful(&answer, case),
            expected,
            "case {case:?}: expected every named version copied as {PUBLISHED}, got {answer:?}"
        );
        assert_eq!(
            failed(&answer),
            Vec::<(String, String)>::new(),
            "case {case:?}: expected no failed version, got {answer:?}"
        );

        for request in &published {
            let in_destination = Publish {
                repository: destination.to_string(),
                ..request.clone()
            };
            // The copy is COMPLETE: the destination describes the resolved version with the
            // provenance it entered the domain with and the revision its content derives.
            let described = describe(&router, &in_destination).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}/{}: the destination must hold the resolved version, got {described:?}",
                request.version
            );
            assert_eq!(
                described.body["packageVersion"]["origin"]["domainEntryPoint"]
                    ["repositoryName"]
                    .as_str(),
                Some(BACK),
                "case {case:?}/{}: expected the provenance of the repository it was published \
                 into, got {described:?}",
                request.version
            );
            let from_source = describe(&router, request).await;
            assert_eq!(
                described.body["packageVersion"]["revision"],
                from_source.body["packageVersion"]["revision"],
                "case {case:?}/{}: expected the resolved version's own revision, got \
                 {described:?}",
                request.version
            );
            assert_eq!(
                answer.body["successfulVersions"][&request.version]["revision"],
                described.body["packageVersion"]["revision"],
                "case {case:?}/{}: expected the reported revision to be the one the \
                 destination now holds, got {answer:?}",
                request.version
            );

            // And the bytes: the asset the upstream held downloads from the destination.
            let (status, payload) = download(&router, &in_destination).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{}: expected the copied asset to download, got status {status}",
                request.version
            );
            assert_eq!(
                payload, request.content,
                "case {case:?}/{}: expected the upstream asset's exact bytes",
                request.version
            );

            // A copy writes the DESTINATION only: a source repository that merely reached
            // the version through its chain still does not hold it.
            if *source != BACK {
                let in_source = Publish {
                    repository: (*source).to_string(),
                    ..request.clone()
                };
                let described = describe(&router, &in_source).await;
                assert_eq!(
                    (described.status, described.code()),
                    (404, "ResourceNotFoundException".to_string()),
                    "case {case:?}/{}: the copy must not materialize anything in the source, \
                     got {described:?}",
                    request.version
                );
            }
        }
    }
}

/// One row of RULE 2: the source repository, the body the copy sends, and why the version
/// is out of reach.
#[derive(Debug)]
struct Unreachable {
    case: &'static str,
    source: &'static str,
    body: Value,
    destination: &'static str,
}

/// RULE 2: without the flag — or with it, on a repository whose chain reaches nothing — the
/// whole call is refused 404 naming the source repository, and nothing is materialized.
#[tokio::test]
async fn a_copy_that_cannot_reach_the_package_is_refused_naming_the_source_repository() {
    let destinations = ["dest-absent", "dest-false", "dest-solo", "dest-reachable"];
    let router = emulator_with_chain(&destinations).await;
    let published = publication(BACK, "only-upstream", "1.0.0", 3);
    given_published(&router, &published).await;

    let rows: [Unreachable; 4] = [
        Unreachable {
            case: "includeFromUpstream absent, one hop away",
            source: MID,
            body: json!({ "versions": ["1.0.0"] }),
            destination: "dest-absent",
        },
        Unreachable {
            case: "includeFromUpstream false, two hops away",
            source: FRONT,
            body: json!({ "versions": ["1.0.0"], "includeFromUpstream": false }),
            destination: "dest-false",
        },
        Unreachable {
            case: "includeFromUpstream true, but the source reaches nothing",
            source: SOLO,
            body: json!({ "versions": ["1.0.0"], "includeFromUpstream": true }),
            destination: "dest-solo",
        },
        Unreachable {
            case: "includeFromUpstream true, and the package is nowhere in the domain",
            source: FRONT,
            body: json!({ "versions": ["1.0.0"], "includeFromUpstream": true }),
            destination: "dest-reachable",
        },
    ];

    for row in &rows {
        let Unreachable {
            case,
            source,
            body,
            destination,
        } = row;
        // The last row asks for a package nothing in the domain holds; every other row asks
        // for the one published into BACK.
        let addressed = if *case == rows[3].case {
            publication(BACK, "nowhere-at-all", "1.0.0", 4)
        } else {
            published.clone()
        };
        let answer = copy(&router, source, destination, &addressed, body.clone()).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: expected the modelled 404, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            format!(
                "The {} package '{}' with namespace '{}' does not exist in repository '{source}'.",
                addressed.format,
                addressed.package,
                addressed.namespace.clone().unwrap_or_default()
            ),
            "case {case:?}: expected the sentence naming the source repository, got {:?}",
            answer.message()
        );
        assert_eq!(
            listed_packages(&router, destination).await,
            Vec::<String>::new(),
            "case {case:?}: a refused copy must leave the destination empty"
        );
    }
}

/// RULE 3: the chain is searched nearest first, so the nearer repository's record is the
/// one copied — content and provenance.
#[tokio::test]
async fn the_nearest_repository_holding_the_version_is_the_one_copied() {
    let router = emulator_with_chain(&["dest-nearest"]).await;
    let package = "two-holders";
    // The same version, with DIFFERENT asset bytes, in both repositories of the chain: the
    // copy addressed on FRONT reaches MID first.
    let far = publication(BACK, package, "1.0.0", 11);
    let near = publication(MID, package, "1.0.0", 42);
    given_published(&router, &far).await;
    given_published(&router, &near).await;
    assert_ne!(
        far.content, near.content,
        "the two holders must differ in content for this case to mean anything"
    );

    let answer = copy(
        &router,
        FRONT,
        "dest-nearest",
        &far,
        json!({ "versions": ["1.0.0"], "includeFromUpstream": true }),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (200, String::new()),
        "case \"two holders\": expected the copy to answer 200, got {answer:?}"
    );

    let copied = Publish {
        repository: "dest-nearest".to_string(),
        ..near.clone()
    };
    let (status, payload) = download(&router, &copied).await;
    assert_eq!(
        status, 200,
        "case \"two holders\": expected the copied asset to download, got status {status}"
    );
    assert_eq!(
        payload, near.content,
        "case \"two holders\": expected the NEAREST holder's bytes, got {} bytes",
        payload.len()
    );
    let described = describe(&router, &copied).await;
    assert_eq!(
        described.body["packageVersion"]["origin"]["domainEntryPoint"]["repositoryName"].as_str(),
        Some(MID),
        "case \"two holders\": expected the nearest holder's provenance, got {described:?}"
    );
}

/// One row of RULE 4: the body the copy sends, and the `(version, outcome)` rows each of
/// the two modelled maps must then carry.
#[derive(Debug)]
struct PerVersion {
    case: &'static str,
    body: Value,
    expected_successful: Vec<(&'static str, &'static str)>,
    expected_failed: Vec<(&'static str, &'static str)>,
}

/// RULE 4: every named version is still decided on its own when the source is resolved
/// through upstreams.
#[tokio::test]
async fn an_upstream_copy_reports_each_named_version_in_one_of_the_two_maps() {
    let router = emulator_with_chain(&["dest-per-version"]).await;
    let destination = "dest-per-version";
    let package = "per-version";
    let held = publication(BACK, package, "1.0.0", 7);
    given_published(&router, &held).await;
    let revision = describe(&router, &held).await.body["packageVersion"]["revision"]
        .as_str()
        .expect("the published revision")
        .to_string();

    let rows: Vec<PerVersion> = vec![
        PerVersion {
            case: "a version no reachable repository holds, beside one that is reachable",
            body: json!({ "versions": ["1.0.0", "9.9.9"], "includeFromUpstream": true }),
            expected_successful: vec![("1.0.0", PUBLISHED)],
            expected_failed: vec![("9.9.9", NOT_FOUND)],
        },
        PerVersion {
            case: "a revision pinned to the resolved version's own content",
            body: json!({
                "versionRevisions": { "1.0.0": revision },
                "includeFromUpstream": true,
                "allowOverwrite": true,
            }),
            expected_successful: vec![("1.0.0", PUBLISHED)],
            expected_failed: Vec::new(),
        },
        PerVersion {
            case: "a revision that is not the resolved version's",
            body: json!({
                "versionRevisions": { "1.0.0": "not-the-revision" },
                "includeFromUpstream": true,
                "allowOverwrite": true,
            }),
            expected_successful: Vec::new(),
            expected_failed: vec![("1.0.0", MISMATCHED_REVISION)],
        },
        PerVersion {
            case: "a version the destination already holds, without allowOverwrite",
            body: json!({ "versions": ["1.0.0"], "includeFromUpstream": true }),
            expected_successful: Vec::new(),
            expected_failed: vec![("1.0.0", ALREADY_EXISTS)],
        },
    ];

    for row in rows {
        let PerVersion {
            case,
            body,
            expected_successful,
            expected_failed,
        } = row;
        let answer = copy(&router, FRONT, destination, &held, body).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {case:?}: expected a per-version answer, got {answer:?}"
        );
        let expected: Vec<(String, String)> = expected_successful
            .iter()
            .map(|(version, status)| ((*version).to_string(), (*status).to_string()))
            .collect();
        assert_eq!(
            successful(&answer, case),
            expected,
            "case {case:?}: expected {expected:?} copied, got {answer:?}"
        );
        let expected: Vec<(String, String)> = expected_failed
            .iter()
            .map(|(version, code)| ((*version).to_string(), (*code).to_string()))
            .collect();
        assert_eq!(
            failed(&answer),
            expected,
            "case {case:?}: expected {expected:?} refused, got {answer:?}"
        );
    }

    // The `NOT_FOUND` entry states the coordinate the copy looked for and the repository
    // it looked in — the SOURCE — by that repository's own internal id, which is the
    // sentence every version-absent refusal of this service states
    // (`tests/all/package_version_not_found_coordinate.rs` pins it over the whole class).
    let answer = copy(
        &router,
        FRONT,
        destination,
        &held,
        json!({ "versions": ["9.9.9"], "includeFromUpstream": true }),
    )
    .await;
    let message = failure_message(&answer, "9.9.9");
    let expected_coordinate = format!(
        "packageCoordinate = 'PackageVersionCoordinateEntity{{name={package}, type=GENERIC, \
         namespace={NAMESPACE}, version=9.9.9, displayName={NAMESPACE}/{package}/9.9.9}}'."
    );
    assert!(
        message.starts_with("Package version not found in repo. repoId = '")
            && message.ends_with(&expected_coordinate),
        "expected the version-not-found sentence carrying {expected_coordinate:?}, got \
         {message:?}"
    );
}

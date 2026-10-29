// SPDX-License-Identifier: CC0-1.0

//! Upstream REPOSITORY resolution on the npm endpoint, end to end over the wire.
//!
//! Every case drives the production front door — a `MultiServiceRouter` with this service
//! mounted — with the requests an npm client sends to a repository endpoint, and then reads
//! the same records back through the CONTROL plane, so what is asserted is that resolving a
//! package through an upstream chain is a package version of the repository that resolved
//! it.
//!
//! The rules under test, each over a TABLE rather than one example:
//!
//! 1. A client connected to a repository resolves the versions its UPSTREAM repositories
//!    hold, transitively: the packument of a package held only by the back repository of a
//!    chain answers from the front one, with every `dist.tarball` pointing at the endpoint
//!    the client actually reached, and every advertised tarball downloads. The rows differ
//!    along the number of hops the version is away (0, 1, 2) and along the number of
//!    versions the package holds; a repository with no upstream still resolves nothing.
//! 2. What the chain resolves is RETAINED in the repository that served it, keeping the
//!    origin it entered the domain with — so the control plane lists it there with the
//!    `domainEntryPoint` of the repository it was published into — and a package record
//!    created by that retention carries `publish: BLOCK` with `upstream: ALLOW`, where one
//!    created by a direct publish carries the opposite pair.
//! 3. A package whose origin configuration blocks upstream ingestion resolves out of the
//!    addressed repository alone: nothing upstream is advertised, downloadable or retained,
//!    while a sibling package that allows it resolves normally.
//! 4. A publish into a package whose origin configuration blocks publishing is refused HTTP
//!    400 with the sentence that states the control, and the identical publish succeeds once
//!    the control is relaxed.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer};
use crate::npm_registry_endpoint::{
    endpoint_host, endpoint_path, list_versions, publish_document, registry, tarball_bytes,
    token_for, Raw,
};

/// The domain every case below lives in.
const DOMAIN: &str = "chain-domain";

/// The repository a version is PUBLISHED into — the entry point of every chain here.
const BACK: &str = "chain-back";

/// The middle repository of the chain: one hop from the version.
const MID: &str = "chain-mid";

/// The front repository: two hops from the version.
const FRONT: &str = "chain-front";

/// A repository with no upstream at all, which resolves nothing it does not hold.
const SOLO: &str = "chain-solo";

/// The refusal the npm endpoint states for a publish a package's origin configuration
/// blocks — fixed text, interpolating nothing.
const PUBLISH_BLOCKED: &str = "This package is configured to block new versions from being published.";

/// One row of RULE 1: which repository the client addresses, and how many hops upstream the
/// version it asks for is.
#[derive(Debug)]
struct Hop {
    case: &'static str,
    /// The repository whose endpoint the client is connected to.
    repository: &'static str,
    /// The package published into [`BACK`] for this row, so rows do not share state.
    package: &'static str,
    /// The versions that publish carries.
    versions: &'static [&'static str],
}

/// The domain, its chain of repositories and a token for it.
async fn given_chain(router: &MultiServiceRouter) -> String {
    assert_eq!(
        create_domain(router, DOMAIN, None).await.status,
        200,
        "CreateDomain must succeed"
    );
    for (repository, upstreams) in [
        (BACK, &[] as &[&str]),
        (MID, &[BACK]),
        (FRONT, &[MID]),
        (SOLO, &[]),
    ] {
        let created = create_repository(router, DOMAIN, repository, None, upstreams).await;
        assert_eq!(
            created.status, 200,
            "CreateRepository {repository:?} must succeed, got {created:?}"
        );
    }
    token_for(router, DOMAIN).await
}

/// Publish `versions` of `package` into [`BACK`] over its own npm endpoint.
async fn publish_into_back(
    router: &MultiServiceRouter,
    token: &str,
    package: &str,
    versions: &[&str],
) {
    let answer = registry(
        router,
        "PUT",
        &endpoint_host(DOMAIN),
        &format!("{}/{package}", endpoint_path(BACK)),
        Some(token),
        Some(publish_document(package, package, versions)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "publishing {package:?} into {BACK:?} must succeed, got {:?}",
        answer.json()
    );
}

/// `GET <endpoint>/<package>` — the packument, as the client connected to `repository`
/// reads it.
async fn packument(
    router: &MultiServiceRouter,
    token: &str,
    repository: &str,
    package: &str,
) -> Raw {
    registry(
        router,
        "GET",
        &endpoint_host(DOMAIN),
        &format!("{}/{package}", endpoint_path(repository)),
        Some(token),
        None,
    )
    .await
}

/// The `dist.tarball` URL a packument advertises for one version, or `None` when the
/// packument does not carry that version.
fn advertised_tarball(packument: &Value, version: &str) -> Option<String> {
    packument
        .get("versions")?
        .get(version)?
        .get("dist")?
        .get("tarball")?
        .as_str()
        .map(str::to_string)
}

/// Fetch the URL a packument advertised, as the client does: the path of that very URL on
/// the host it reached.
async fn fetch_advertised(router: &MultiServiceRouter, token: &str, url: &str) -> Raw {
    let host = endpoint_host(DOMAIN);
    let path = url
        .strip_prefix(&format!("https://{host}"))
        .unwrap_or_else(|| panic!("the advertised URL {url:?} must be on the host {host:?}"));
    registry(router, "GET", &host, path, Some(token), None).await
}

/// `DescribePackage` for an npm package of one repository.
async fn describe_package(
    router: &MultiServiceRouter,
    repository: &str,
    package: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/v1/package?domain={DOMAIN}&repository={repository}&format=npm&package={package}"
        ),
        None,
    )
    .await
}

/// `PutPackageOriginConfiguration` for an npm package of one repository.
async fn put_origin_configuration(
    router: &MultiServiceRouter,
    repository: &str,
    package: &str,
    publish: &str,
    upstream: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!(
            "/v1/package?domain={DOMAIN}&repository={repository}&format=npm&package={package}"
        ),
        Some(json!({ "restrictions": { "publish": publish, "upstream": upstream } })),
    )
    .await
}

/// The versions `ListPackageVersions` reports for a package of one repository, each with the
/// origin it reports, as `(version, originType, domainEntryPoint.repositoryName)`.
async fn listed_versions(
    router: &MultiServiceRouter,
    repository: &str,
    package: &str,
) -> Vec<(String, String, String)> {
    let answer = list_versions(router, DOMAIN, repository, None, package).await;
    assert_eq!(
        answer.status, 200,
        "ListPackageVersions for {package:?} in {repository:?} must succeed, got {answer:?}"
    );
    answer
        .body
        .get("versions")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    let text = |path: &[&str]| {
                        let mut at = entry;
                        for step in path {
                            at = at.get(*step).unwrap_or(&Value::Null);
                        }
                        at.as_str().unwrap_or_default().to_string()
                    };
                    (
                        text(&["version"]),
                        text(&["origin", "originType"]),
                        text(&["origin", "domainEntryPoint", "repositoryName"]),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE 1 and RULE 2: a version resolves through however many upstream hops separate it from
/// the client, is served from the endpoint the client reached, and is retained there with the
/// origin it entered the domain with.
#[tokio::test]
async fn a_version_resolves_through_the_upstream_chain_and_is_retained_where_it_was_served() {
    let router = emulator();
    let token = given_chain(&router).await;

    // Rows differ along the number of hops the version is away and the number of versions
    // the package holds.
    let rows: [Hop; 3] = [
        Hop {
            case: "the repository that holds the version",
            repository: BACK,
            package: "hop-zero",
            versions: &["1.0.0"],
        },
        Hop {
            case: "one hop upstream",
            repository: MID,
            package: "hop-one",
            versions: &["1.0.0", "2.0.0"],
        },
        Hop {
            case: "two hops upstream",
            repository: FRONT,
            package: "hop-two",
            versions: &["0.1.0", "1.2.3", "2.0.0"],
        },
    ];

    for row in &rows {
        let Hop {
            case,
            repository,
            package,
            versions,
        } = row;
        publish_into_back(&router, &token, package, versions).await;

        let answered = packument(&router, &token, repository, package).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected the packument to answer 200, got {:?}",
            answered.json()
        );
        let document = answered.json();
        for version in *versions {
            let Some(url) = advertised_tarball(&document, version) else {
                panic!("case {case:?}: expected {version:?} to be advertised, got {document:?}");
            };
            // RULE 1: the tarball URL names the endpoint the client reached, never the
            // repository the version was published into.
            assert!(
                url.starts_with(&format!(
                    "https://{}{}",
                    endpoint_host(DOMAIN),
                    endpoint_path(repository)
                )),
                "case {case:?}/{version:?}: expected the tarball URL on the addressed \
                 endpoint, got {url:?}"
            );
        }

        // RULE 1: every advertised version downloads from the endpoint that advertised it,
        // and the bytes are the ones published into the repository the chain resolved to.
        for version in *versions {
            let url = advertised_tarball(&document, version).expect("the advertised tarball");
            let fetched = fetch_advertised(&router, &token, &url).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}/{version:?}: expected the advertised tarball to answer 200, \
                 got {:?}",
                fetched.json()
            );
            assert_eq!(
                fetched.body,
                tarball_bytes(package, version),
                "case {case:?}/{version:?}: expected the published bytes"
            );
        }

        // RULE 2: what the chain resolved is a version OF the repository that served it,
        // carrying the origin it entered the domain with — the repository it was published
        // into, not the one that retained it.
        let listed = listed_versions(&router, repository, package).await;
        let expected: Vec<(String, String, String)> = versions
            .iter()
            .map(|version| {
                (
                    (*version).to_string(),
                    "INTERNAL".to_string(),
                    BACK.to_string(),
                )
            })
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {repository:?} to hold every resolved version, published \
             into {BACK:?}"
        );

        // RULE 2: how the package came to exist in that repository is what its origin
        // controls say — retained from an upstream here, published in the repository it came
        // from.
        let described = describe_package(&router, repository, package).await;
        let expected_restrictions = if *repository == BACK {
            json!({ "publish": "ALLOW", "upstream": "BLOCK" })
        } else {
            json!({ "publish": "BLOCK", "upstream": "ALLOW" })
        };
        assert_eq!(
            described.body["package"]["originConfiguration"]["restrictions"],
            expected_restrictions,
            "case {case:?}: expected the restrictions of a package created that way, got \
             {described:?}"
        );
    }

    // A repository with no upstream resolves nothing it does not hold: neither the data
    // plane nor the control plane reports the package there.
    for package in ["hop-zero", "hop-one", "hop-two"] {
        let answered = packument(&router, &token, SOLO, package).await;
        assert_eq!(
            answered.status, 404,
            "case \"no upstream at all\"/{package:?}: expected 404, got {:?}",
            answered.json()
        );
        let listed = list_versions(&router, DOMAIN, SOLO, None, package).await;
        assert_eq!(
            (listed.status, listed.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case \"no upstream at all\"/{package:?}: expected the modelled 404, got {listed:?}"
        );
        assert_eq!(
            listed.message(),
            format!("The npm package '{package}' does not exist in repository '{SOLO}'."),
            "case \"no upstream at all\"/{package:?}: expected the repository-qualified \
             sentence, got {:?}",
            listed.message()
        );
    }
}

/// RULE 3: the package's own origin configuration decides whether anything upstream is
/// reachable at all.
#[tokio::test]
async fn a_package_blocking_upstream_ingestion_resolves_nothing_upstream() {
    let router = emulator();
    let token = given_chain(&router).await;

    // (case, the `upstream` restriction set on the FRONT repository's record, whether the
    // version resolves there)
    let rows: [(&str, &str, bool); 2] = [
        ("upstream ingestion blocked", "BLOCK", false),
        ("upstream ingestion allowed", "ALLOW", true),
    ];

    for (case, upstream, resolves) in rows {
        let package = format!("origin-{}", upstream.to_ascii_lowercase());
        publish_into_back(&router, &token, &package, &["1.0.0"]).await;
        // The restriction is set BEFORE any fetch, which is what
        // `PutPackageOriginConfiguration` pre-creating the record is for.
        let configured =
            put_origin_configuration(&router, FRONT, &package, "ALLOW", upstream).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: PutPackageOriginConfiguration must succeed, got {configured:?}"
        );

        let answered = packument(&router, &token, FRONT, &package).await;
        let expected_status = if resolves { 200 } else { 404 };
        assert_eq!(
            answered.status, expected_status,
            "case {case:?}: expected the packument to answer {expected_status}, got {:?}",
            answered.json()
        );

        // A tarball URL the blocked package never advertised is still refused when a client
        // guesses it, and it ingests nothing.
        let url = format!(
            "https://{}{}/{package}/-/{package}-1.0.0.tgz",
            endpoint_host(DOMAIN),
            endpoint_path(FRONT)
        );
        let fetched = fetch_advertised(&router, &token, &url).await;
        assert_eq!(
            fetched.status, expected_status,
            "case {case:?}: expected the tarball fetch to answer {expected_status}, got {:?}",
            fetched.json()
        );

        let listed = listed_versions(&router, FRONT, &package).await;
        let expected: Vec<(String, String, String)> = if resolves {
            vec![(
                "1.0.0".to_string(),
                "INTERNAL".to_string(),
                BACK.to_string(),
            )]
        } else {
            Vec::new()
        };
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the retained versions {expected:?}, got {listed:?}"
        );
    }
}

/// RULE 4: a publish a package's origin configuration blocks is refused 400 with the
/// sentence that states the control, and the same publish succeeds once it is relaxed.
#[tokio::test]
async fn a_publish_into_a_package_that_blocks_publishing_is_refused_with_the_controls_sentence()
{
    let router = emulator();
    let token = given_chain(&router).await;

    // Rows differ along HOW the package came to block publishing: one created by ingesting a
    // version from upstream (whose default is `publish: BLOCK`), and one a caller blocked
    // explicitly.
    let ingested = "blocked-by-ingestion";
    publish_into_back(&router, &token, ingested, &["1.0.0"]).await;
    let document = packument(&router, &token, FRONT, ingested).await.json();
    let url = advertised_tarball(&document, "1.0.0").expect("the advertised tarball");
    assert_eq!(
        fetch_advertised(&router, &token, &url).await.status,
        200,
        "the pull that creates the ingested package must succeed"
    );

    let configured = "blocked-by-configuration";
    assert_eq!(
        put_origin_configuration(&router, FRONT, configured, "BLOCK", "ALLOW")
            .await
            .status,
        200,
        "PutPackageOriginConfiguration must succeed"
    );

    for (case, package) in [
        ("a package created by ingesting a version", ingested),
        ("a package a caller blocked", configured),
    ] {
        let refused = registry(
            &router,
            "PUT",
            &endpoint_host(DOMAIN),
            &format!("{}/{package}", endpoint_path(FRONT)),
            Some(&token),
            Some(publish_document(package, package, &["9.9.9"])),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {:?}",
            refused.json()
        );
        assert_eq!(
            refused.error(),
            PUBLISH_BLOCKED,
            "case {case:?}: expected the sentence that states the control, got {:?}",
            refused.json()
        );
        assert_eq!(
            refused.json().get("success"),
            Some(&Value::Bool(false)),
            "case {case:?}: expected the refusal to report success=false, got {:?}",
            refused.json()
        );

        // The control: relaxing the restriction makes the identical publish succeed, so the
        // refusal was the configuration and nothing else.
        assert_eq!(
            put_origin_configuration(&router, FRONT, package, "ALLOW", "ALLOW")
                .await
                .status,
            200,
            "case {case:?}: PutPackageOriginConfiguration must succeed"
        );
        let accepted = registry(
            &router,
            "PUT",
            &endpoint_host(DOMAIN),
            &format!("{}/{package}", endpoint_path(FRONT)),
            Some(&token),
            Some(publish_document(package, package, &["9.9.9"])),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected the relaxed publish to succeed, got {:?}",
            accepted.json()
        );
    }
}

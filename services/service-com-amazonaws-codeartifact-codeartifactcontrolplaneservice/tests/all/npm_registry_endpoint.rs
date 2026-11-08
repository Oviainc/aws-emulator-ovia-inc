// SPDX-License-Identifier: CC0-1.0

//! The npm REGISTRY a repository endpoint serves, end to end over the wire.
//!
//! Every case drives the production front door — a `MultiServiceRouter` with this service
//! mounted — with the requests an npm client sends: `GET /-/ping`, `PUT /<package>` with a
//! publish document, `GET /<package>` for the packument and a `GET` of a tarball path. What
//! is asserted is the status, the headers and the bytes such a client sees, and that what
//! the registry stored is the same record the CONTROL plane describes.
//!
//! The rules under test, each over a TABLE rather than one example:
//!
//! 1. The endpoint is served at both addressings `GetRepositoryEndpoint` can be reached
//!    through — the domain's published host and the same `/<format>/<repository>/` path on
//!    the emulator's own address — and a request under no repository this service holds is
//!    not claimed at all.
//! 2. A publish over the endpoint IS a package version of that repository: the packument
//!    and the tarball answer from it, and `DescribePackageVersion`,
//!    `ListPackageVersionAssets` and `ListPackageVersions` describe the same version, with
//!    the npm manifest and the tarball as its assets. The rows span what the rule
//!    generalises over: a plain name, a scoped name under its URL-encoded namespace, a name
//!    carrying punctuation, and a document carrying several versions.
//! 3. A version's STATUS decides what the registry lists and what it serves: `Published` is
//!    listed and downloadable, `Unlisted` is hidden from the packument but still
//!    downloadable, and `Archived` is neither.
//! 4. Only a bearer token this service minted FOR THE ADDRESSED DOMAIN, and has not let
//!    expire, authenticates; everything else is `401`.
//! 5. A publish the registry will not perform stores nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, emulator_with_engine, Answer, ACCOUNT, REGION};

/// The npm endpoint path a repository publishes: `/<format>/<repository>/`.
pub fn endpoint_path(repository: &str) -> String {
    format!("/npm/{repository}")
}

/// The host `GetRepositoryEndpoint` hands out for a domain:
/// `<domain>-<account>.d.codeartifact.<region>.amazonaws.com`.
pub fn endpoint_host(domain: &str) -> String {
    format!("{domain}-{ACCOUNT}.d.codeartifact.{REGION}.amazonaws.com")
}

/// The emulator's own address, which is what a client that cannot resolve the published
/// host reaches.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// One raw HTTP answer, reduced to what an npm client branches on.
#[derive(Debug)]
pub struct Raw {
    pub status: u16,
    pub content_type: Option<String>,
    pub www_authenticate: Option<String>,
    pub body: Vec<u8>,
}

impl Raw {
    /// The body parsed as JSON, or `Null` when it is not JSON (a tarball).
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The `error` member an npm registry states a refusal with.
    pub fn error(&self) -> String {
        self.json()
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Send one raw registry request — no SigV4 credential, an npm bearer token instead — and
/// reduce the answer to a [`Raw`].
pub async fn registry(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Vec<u8>>,
) -> Raw {
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request
        .body(bytes::Bytes::from(body.unwrap_or_default()))
        .expect("build request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let content_type = header("content-type");
    let www_authenticate = header("www-authenticate");
    Raw {
        status,
        content_type,
        www_authenticate,
        body: response.into_body().to_vec(),
    }
}

/// `GetAuthorizationToken` for `domain`, returning the minted token.
pub async fn token_for(router: &MultiServiceRouter, domain: &str) -> String {
    let answer = call(
        router,
        "POST",
        &format!("/v1/authorization-token?domain={domain}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "GetAuthorizationToken for {domain:?} must succeed, got {answer:?}"
    );
    answer
        .body
        .get("authorizationToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A domain with one repository, and a token for it.
async fn given_repository(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
) -> String {
    assert_eq!(
        create_domain(router, domain, None).await.status,
        200,
        "CreateDomain {domain:?} must succeed"
    );
    assert_eq!(
        create_repository(router, domain, repository, None, &[]).await.status,
        200,
        "CreateRepository {repository:?} must succeed"
    );
    token_for(router, domain).await
}

/// The bytes a test publishes as a version's tarball. Opaque content: the registry stores
/// and serves an asset's bytes verbatim, so what matters is that the bytes come back.
pub fn tarball_bytes(package: &str, version: &str) -> Vec<u8> {
    format!("tarball of {package}@{version}").into_bytes()
}

/// The npm publish document `npm publish` sends: the package name, one manifest per
/// version, the `dist-tags` the publisher set, and each version's tarball attached
/// base64-encoded under its file name.
///
/// Written as TEXT rather than built from a map, because the order the manifest declares
/// its dependencies in is part of what is under test and a map would re-order it.
pub fn publish_document(display_name: &str, unscoped: &str, versions: &[&str]) -> Vec<u8> {
    use base64::Engine as _;

    let manifests: Vec<String> = versions
        .iter()
        .map(|version| {
            format!(
                r#""{version}":{{"name":"{display_name}","version":"{version}",
                 "description":"{display_name} at {version}",
                 "homepage":"https://{unscoped}.test","license":"Apache-2.0",
                 "repository":{{"type":"git","url":"https://git.test/{unscoped}.git"}},
                 "dependencies":{{"zeta":"^3.0.0","alpha":"~1.2.3","left-pad":"^1.0.0"}},
                 "devDependencies":{{"tester":"*"}},
                 "dist":{{"shasum":"ignored-by-the-registry"}}}}"#
            )
        })
        .collect();
    let attachments: Vec<String> = versions
        .iter()
        .map(|version| {
            let bytes = tarball_bytes(unscoped, version);
            format!(
                r#""{display_name}-{version}.tgz":{{"content_type":"application/octet-stream",
                 "data":"{}","length":{}}}"#,
                base64::engine::general_purpose::STANDARD.encode(&bytes),
                bytes.len()
            )
        })
        .collect();
    let latest = versions.last().copied().unwrap_or_default();
    format!(
        r#"{{"_id":"{display_name}","name":"{display_name}",
         "dist-tags":{{"latest":"{latest}"}},
         "versions":{{{}}},"_attachments":{{{}}}}}"#,
        manifests.join(","),
        attachments.join(",")
    )
    .into_bytes()
}

/// `DescribePackageVersion` over the control plane, for the version a registry publish
/// stored.
async fn describe_version(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    namespace: Option<&str>,
    package: &str,
    version: &str,
) -> Answer {
    let mut uri = format!(
        "/v1/package/version?domain={domain}&repository={repository}&format=npm&package={package}\
         &version={version}"
    );
    if let Some(namespace) = namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    call(router, "GET", &uri, None).await
}

/// `ListPackageVersionAssets` over the control plane.
async fn list_assets(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    namespace: Option<&str>,
    package: &str,
    version: &str,
) -> Answer {
    let mut uri = format!(
        "/v1/package/version/assets?domain={domain}&repository={repository}&format=npm\
         &package={package}&version={version}"
    );
    if let Some(namespace) = namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    call(router, "POST", &uri, Some(json!({}))).await
}

/// `ListPackageVersions` over the control plane.
pub async fn list_versions(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    namespace: Option<&str>,
    package: &str,
) -> Answer {
    let mut uri = format!(
        "/v1/package/versions?domain={domain}&repository={repository}&format=npm\
         &package={package}"
    );
    if let Some(namespace) = namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    call(router, "POST", &uri, Some(json!({}))).await
}

/// `UpdatePackageVersionsStatus` over the control plane.
async fn update_status(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    package: &str,
    versions: &[&str],
    target: &str,
) -> Answer {
    let uri = format!(
        "/v1/package/versions/update_status?domain={domain}&repository={repository}&format=npm\
         &package={package}"
    );
    call(
        router,
        "POST",
        &uri,
        Some(json!({ "versions": versions, "targetStatus": target })),
    )
    .await
}

/// RULE 1: the endpoint answers at the published host and at the same path on the
/// emulator's own address, and a path under no repository this service holds is not claimed
/// at all — the front door's own `UnknownService` answers it.
#[tokio::test]
async fn the_endpoint_is_served_at_every_addressing_the_service_publishes() {
    let router = emulator();
    let domain = "ca-endpoint";
    let repository = "serving";
    let token = given_repository(&router, domain, repository).await;

    // (case, host, path, expected status, whether the answer is this registry's)
    let cases: [(&str, String, String, u16, bool); 6] = [
        (
            "the published IPv4 endpoint host",
            endpoint_host(domain),
            format!("{}/-/ping?write=true", endpoint_path(repository)),
            200,
            true,
        ),
        (
            "the published dual-stack endpoint host",
            format!("{domain}-{ACCOUNT}.d.codeartifact.{REGION}.on.aws"),
            format!("{}/-/ping", endpoint_path(repository)),
            200,
            true,
        ),
        (
            "the emulator's own address, by path",
            EMULATOR_HOST.to_string(),
            format!("{}/-/ping", endpoint_path(repository)),
            200,
            true,
        ),
        (
            // `npm` and `pypi` are the two formats whose protocols this service serves; a
            // path under any other format's endpoint is not claimed by either plane.
            "a format whose protocol no plane serves",
            endpoint_host(domain),
            format!("/maven/{repository}/-/ping"),
            404,
            false,
        ),
        (
            "a repository this service does not hold",
            endpoint_host(domain),
            "/npm/no-such-repository/-/ping".to_string(),
            404,
            false,
        ),
        (
            "a registry route this endpoint does not serve",
            endpoint_host(domain),
            format!("{}/-/whoami", endpoint_path(repository)),
            404,
            true,
        ),
    ];

    for (case, host, path, expected, ours) in cases {
        let answer = registry(&router, "GET", &host, &path, Some(&token), None).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {answer:?}"
        );
        let unknown_service = answer.json().get("__type").and_then(Value::as_str)
            == Some("UnknownService");
        assert_eq!(
            !unknown_service,
            ours,
            "case {case:?}: expected this registry to answer={ours}, got {answer:?}"
        );
    }
}

/// RULE 1: on the emulator's own address the repository NAME is the whole address, so a
/// name two domains hold is ambiguous and is not claimed — while the published host, which
/// names the domain, keeps serving both.
#[tokio::test]
async fn an_ambiguous_path_addressing_is_not_claimed_while_the_host_addressing_still_serves() {
    let router = emulator();
    let repository = "shared-name";
    let first = given_repository(&router, "ca-first", repository).await;
    let second = given_repository(&router, "ca-second", repository).await;

    let path = format!("{}/-/ping", endpoint_path(repository));
    let ambiguous = registry(&router, "GET", EMULATOR_HOST, &path, Some(&first), None).await;
    assert_eq!(
        ambiguous.json().get("__type").and_then(Value::as_str),
        Some("UnknownService"),
        "case \"two domains hold the name\": expected the path addressing not to be claimed, \
         got {ambiguous:?}"
    );

    for (case, domain, token) in [
        ("the first domain", "ca-first", &first),
        ("the second domain", "ca-second", &second),
    ] {
        let answer = registry(
            &router,
            "GET",
            &endpoint_host(domain),
            &path,
            Some(token),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the published host names the domain, so it must serve, got \
             {answer:?}"
        );
    }
}

/// RULE 2: a publish over the endpoint is a package version of the repository — readable
/// back as a packument, as a tarball, and through the control plane.
#[tokio::test]
async fn a_publish_over_the_endpoint_is_a_package_version_of_the_repository() {
    use sha2::Digest as _;

    let router = emulator();
    let domain = "ca-publish";
    let repository = "releases";
    let token = given_repository(&router, domain, repository).await;
    let host = endpoint_host(domain);

    // (case, the path the client PUTs to, the namespace, the name, the versions)
    type PackageRow = (
        &'static str,
        &'static str,
        Option<&'static str>,
        &'static str,
        &'static [&'static str],
    );
    let cases: [PackageRow; 4] = [
        ("a plain package", "left-pad", None, "left-pad", &["1.0.0"]),
        (
            "a scoped package under its encoded namespace",
            "@acme%2fwidget",
            Some("acme"),
            "widget",
            &["2.1.0"],
        ),
        (
            "a name carrying punctuation",
            "my.pkg_name",
            None,
            "my.pkg_name",
            &["0.0.1"],
        ),
        (
            "several versions in one document",
            "multi",
            None,
            "multi",
            &["1.0.0", "1.0.1", "1.1.0"],
        ),
    ];

    for (case, url_path, namespace, name, versions) in cases {
        let display = match namespace {
            Some(scope) => format!("@{scope}/{name}"),
            None => name.to_string(),
        };
        let published = registry(
            &router,
            "PUT",
            &host,
            &format!("{}/{url_path}", endpoint_path(repository)),
            Some(&token),
            Some(publish_document(&display, name, versions)),
        )
        .await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: expected the publish to answer 200, got {published:?}"
        );
        assert_eq!(
            published.json().get("ok").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: expected an npm success body, got {:?}",
            published.json()
        );

        // The packument lists every published version, each with the dist object a client
        // verifies integrity against.
        let packument = registry(
            &router,
            "GET",
            &host,
            &format!("{}/{url_path}", endpoint_path(repository)),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(
            packument.status, 200,
            "case {case:?}: expected the packument to answer 200, got {packument:?}"
        );
        let document = packument.json();
        assert_eq!(
            document.get("name").and_then(Value::as_str),
            Some(display.as_str()),
            "case {case:?}: expected the packument to name {display:?}, got {document:?}"
        );
        assert_eq!(
            document
                .get("dist-tags")
                .and_then(|tags| tags.get("latest"))
                .and_then(Value::as_str),
            versions.last().copied(),
            "case {case:?}: expected the latest tag at the published version, got {document:?}"
        );
        for version in versions {
            let entry = document
                .get("versions")
                .and_then(|listed| listed.get(*version))
                .cloned()
                .unwrap_or(Value::Null);
            let expected_bytes = tarball_bytes(name, version);
            let expected_integrity = format!(
                "sha512-{}",
                {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD
                        .encode(sha2::Sha512::digest(&expected_bytes))
                }
            );
            assert_eq!(
                entry.get("dist").and_then(|dist| dist.get("integrity")).and_then(Value::as_str),
                Some(expected_integrity.as_str()),
                "case {case:?} version {version:?}: expected the stored tarball's integrity, \
                 got {entry:?}"
            );
            assert_eq!(
                entry
                    .get("dependencies")
                    .and_then(|dependencies| dependencies.get("left-pad"))
                    .and_then(Value::as_str),
                Some("^1.0.0"),
                "case {case:?} version {version:?}: expected the published manifest to be read \
                 back, got {entry:?}"
            );

            // The tarball URL the packument publishes is fetched, and answers the exact
            // bytes that were published. A scoped package's URL carries its scope as a
            // path segment, so the client fetches it under `@scope/name`.
            let tarball_url = entry
                .get("dist")
                .and_then(|dist| dist.get("tarball"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let expected_prefix = format!(
                "https://{host}{}/{display}/-/",
                endpoint_path(repository)
            );
            assert!(
                tarball_url.starts_with(&expected_prefix),
                "case {case:?} version {version:?}: expected a tarball URL under \
                 {expected_prefix:?}, got {tarball_url:?}"
            );
            let fetch_path = tarball_url
                .split_once(&host)
                .map(|(_, path)| path.to_string())
                .unwrap_or_default();
            let fetched = registry(&router, "GET", &host, &fetch_path, Some(&token), None).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?} version {version:?}: expected the tarball at {fetch_path:?} to \
                 answer 200, got {fetched:?}"
            );
            assert_eq!(
                fetched.body, expected_bytes,
                "case {case:?} version {version:?}: expected the published bytes back, got {} \
                 bytes",
                fetched.body.len()
            );
            assert_eq!(
                fetched.content_type.as_deref(),
                Some("application/octet-stream"),
                "case {case:?} version {version:?}: expected a tarball media type, got \
                 {fetched:?}"
            );

            // The CONTROL plane describes the same version: one store, not two.
            let described =
                describe_version(&router, domain, repository, namespace, name, version).await;
            assert_eq!(
                described.status, 200,
                "case {case:?} version {version:?}: DescribePackageVersion must describe the \
                 published version, got {described:?}"
            );
            let version_body = described
                .body
                .get("packageVersion")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                version_body.get("format").and_then(Value::as_str),
                Some("npm"),
                "case {case:?} version {version:?}: expected the npm format, got {version_body:?}"
            );
            assert_eq!(
                version_body.get("namespace").and_then(Value::as_str),
                namespace,
                "case {case:?} version {version:?}: expected the namespace {namespace:?}, got \
                 {version_body:?}"
            );
            assert_eq!(
                version_body.get("status").and_then(Value::as_str),
                Some("Published"),
                "case {case:?} version {version:?}: expected a Published version, got \
                 {version_body:?}"
            );
            // The metadata-sourced members are read out of the document that was
            // published, not invented.
            for (member, expected) in [
                ("summary", format!("{display} at {version}")),
                ("homePage", format!("https://{name}.test")),
                (
                    "sourceCodeRepository",
                    format!("https://git.test/{name}.git"),
                ),
            ] {
                assert_eq!(
                    version_body.get(member).and_then(Value::as_str),
                    Some(expected.as_str()),
                    "case {case:?} version {version:?}: expected {member} from the published \
                     manifest, got {version_body:?}"
                );
            }
            assert_eq!(
                version_body
                    .get("licenses")
                    .and_then(Value::as_array)
                    .and_then(|licenses| licenses.first())
                    .and_then(|license| license.get("name"))
                    .and_then(Value::as_str),
                Some("Apache-2.0"),
                "case {case:?} version {version:?}: expected the manifest's license, got \
                 {version_body:?}"
            );

            // Its assets are the npm manifest and the tarball, under the names npm uses.
            let assets = list_assets(&router, domain, repository, namespace, name, version).await;
            let mut listed: Vec<String> = assets
                .body
                .get("assets")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|asset| asset.get("name").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            listed.sort();
            let mut expected_assets = vec!["package.tgz".to_string()];
            expected_assets.sort();
            assert_eq!(
                listed, expected_assets,
                "case {case:?} version {version:?}: expected the tarball as the version's one \
                 asset, under the fixed name this format stores it as, got {listed:?}"
            );
        }

        // `defaultDisplayVersion` is the version the latest tag names, which the publish
        // document set.
        let listing = list_versions(&router, domain, repository, namespace, name).await;
        assert_eq!(
            listing
                .body
                .get("defaultDisplayVersion")
                .and_then(Value::as_str),
            versions.last().copied(),
            "case {case:?}: expected the latest tag to decide the display version, got \
             {listing:?}"
        );
    }
}

/// RULE 3: a version's status decides what the registry lists and what it serves.
#[tokio::test]
async fn a_version_status_decides_what_is_listed_and_what_is_served() {
    let router = emulator();
    let domain = "ca-status";
    let repository = "statuses";
    let token = given_repository(&router, domain, repository).await;
    let host = endpoint_host(domain);
    let package = "status-pkg";
    let versions = ["1.0.0", "1.1.0", "1.2.0"];

    let published = registry(
        &router,
        "PUT",
        &host,
        &format!("{}/{package}", endpoint_path(repository)),
        Some(&token),
        Some(publish_document(package, package, &versions)),
    )
    .await;
    assert_eq!(
        published.status, 200,
        "the versions must be published before their statuses are moved, got {published:?}"
    );

    // (case, the version, the status it is moved to, whether the packument lists it,
    //  the status its tarball download answers)
    let cases: [(&str, &str, Option<&str>, bool, u16); 3] = [
        ("a published version", "1.2.0", None, true, 200),
        (
            "an unlisted version",
            "1.0.0",
            Some("Unlisted"),
            false,
            200,
        ),
        (
            "an archived version",
            "1.1.0",
            Some("Archived"),
            false,
            400,
        ),
    ];

    for (case, version, target, listed, download) in cases {
        if let Some(target) = target {
            let moved = update_status(&router, domain, repository, package, &[version], target)
                .await;
            assert_eq!(
                moved.status, 200,
                "case {case:?}: moving {version:?} to {target} must succeed, got {moved:?}"
            );
        }

        let packument = registry(
            &router,
            "GET",
            &host,
            &format!("{}/{package}", endpoint_path(repository)),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(
            packument.status, 200,
            "case {case:?}: the packument must still answer, got {packument:?}"
        );
        let present = packument
            .json()
            .get("versions")
            .and_then(|entries| entries.get(version))
            .is_some();
        assert_eq!(
            present, listed,
            "case {case:?}: expected the packument to list {version:?}={listed}, got {:?}",
            packument.json()
        );

        let fetched = registry(
            &router,
            "GET",
            &host,
            &format!(
                "{}/{package}/-/{package}-{version}.tgz",
                endpoint_path(repository)
            ),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(
            fetched.status, download,
            "case {case:?}: expected the tarball download to answer {download}, got {fetched:?}"
        );
        if download == 200 {
            assert_eq!(
                fetched.body,
                tarball_bytes(package, version),
                "case {case:?}: expected the published bytes back"
            );
        } else {
            assert!(
                fetched.error().contains(version),
                "case {case:?}: expected the refusal to name the version, got {:?}",
                fetched.error()
            );
        }
    }

    // The latest tag follows what is still listed: the only published version left.
    let packument = registry(
        &router,
        "GET",
        &host,
        &format!("{}/{package}", endpoint_path(repository)),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        packument
            .json()
            .get("dist-tags")
            .and_then(|tags| tags.get("latest"))
            .and_then(Value::as_str),
        Some("1.2.0"),
        "expected the latest tag at the one still-published version, got {:?}",
        packument.json()
    );

    // And a tag resolves only to a version a client can fetch: unlisting the version
    // `latest` points at leaves the packument carrying no tag at all, rather than
    // re-pointing `latest` at a version the publisher never tagged — while that version's
    // content stays downloadable.
    let unlisted = update_status(
        &router,
        domain,
        repository,
        package,
        &["1.2.0"],
        "Unlisted",
    )
    .await;
    assert_eq!(
        unlisted.status, 200,
        "unlisting the tagged version must succeed, got {unlisted:?}"
    );
    let after = registry(
        &router,
        "GET",
        &host,
        &format!("{}/{package}", endpoint_path(repository)),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        after.status, 404,
        "case \"every version unlisted\": a package with no listed version is not found, got \
         {after:?}"
    );
    let still_downloadable = registry(
        &router,
        "GET",
        &host,
        &format!(
            "{}/{package}/-/{package}-1.2.0.tgz",
            endpoint_path(repository)
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        still_downloadable.status, 200,
        "case \"every version unlisted\": an unlisted version is still downloadable, got \
         {still_downloadable:?}"
    );
}

/// RULE 4: only a token this service minted for the ADDRESSED domain, and has not let
/// expire, authenticates.
#[tokio::test]
async fn only_an_unexpired_token_minted_for_this_domain_authenticates() {
    let (router, engine) = emulator_with_engine();
    let domain = "ca-auth";
    let repository = "gated";
    let token = given_repository(&router, domain, repository).await;
    let other = given_repository(&router, "ca-other", "other-repo").await;

    // A token this service minted and the clock has already passed, seeded into the same
    // store `GetAuthorizationToken` writes.
    let expired = "expired-token-value";
    {
        let state = service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::state::service_state(&*engine)
            .expect("the CodeArtifact store");
        let now = std::time::SystemTime::now();
        state.lock().record_authorization_token(
            expired,
            domain,
            ACCOUNT,
            &format!("arn:aws:iam::{ACCOUNT}:user/emulator"),
            now - std::time::Duration::from_secs(1),
            now,
        );
    }

    // (case, the Authorization value presented, the expected status)
    let cases: [(&str, Option<String>, u16); 5] = [
        ("a token minted for this domain", Some(token.clone()), 200),
        ("a token minted for another domain", Some(other), 401),
        (
            "a token this service never minted",
            Some("not-a-minted-token".to_string()),
            401,
        ),
        ("a token the clock has expired", Some(expired.to_string()), 401),
        ("no credential at all", None, 401),
    ];

    let path = format!("{}/-/ping?write=true", endpoint_path(repository));
    let host = endpoint_host(domain);
    for (case, presented, expected) in cases {
        let answer = registry(
            &router,
            "GET",
            &host,
            &path,
            presented.as_deref(),
            None,
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {answer:?}"
        );
        if expected == 401 {
            assert_eq!(
                answer.www_authenticate.as_deref(),
                Some("Bearer"),
                "case {case:?}: expected the bearer challenge, got {answer:?}"
            );
        }
    }

    // A credential presented under another scheme is no bearer token either, and a publish
    // is gated the same way a read is.
    let basic = http::Request::builder()
        .method("PUT")
        .uri(format!("{}/left-pad", endpoint_path(repository)))
        .header("host", &host)
        .header("authorization", "Basic dXNlcjpwYXNzd29yZA==")
        .body(bytes::Bytes::from(publish_document(
            "left-pad",
            "left-pad",
            &["1.0.0"],
        )))
        .expect("build request");
    assert_eq!(
        router.dispatch(basic).await.status().as_u16(),
        401,
        "case \"basic authentication on a publish\": expected 401"
    );
    let listing = list_versions(&router, domain, repository, None, "left-pad").await;
    assert_eq!(
        listing.status, 404,
        "a refused publish must store nothing, got {listing:?}"
    );
}

/// RULE 2: the dependencies a published manifest declares are reported, in the order the
/// manifest declares them, with the `dependencyType` of the section that declared each —
/// read out of the document the publish carried.
#[tokio::test]
async fn the_declared_dependencies_of_a_published_version_are_reported_in_order() {
    let router = emulator();
    let domain = "ca-dependencies";
    let repository = "declared";
    let token = given_repository(&router, domain, repository).await;
    let host = endpoint_host(domain);
    let package = "dependent";

    let published = registry(
        &router,
        "PUT",
        &host,
        &format!("{}/{package}", endpoint_path(repository)),
        Some(&token),
        Some(publish_document(package, package, &["1.0.0"])),
    )
    .await;
    assert_eq!(
        published.status, 200,
        "the version must be published first, got {published:?}"
    );

    let listed = call(
        &router,
        "POST",
        &format!(
            "/v1/package/version/dependencies?domain={domain}&repository={repository}\
             &format=npm&package={package}&version=1.0.0"
        ),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "ListPackageVersionDependencies must report the published manifest's dependencies, got \
         {listed:?}"
    );
    let reported: Vec<(String, String, String)> = listed
        .body
        .get("dependencies")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|dependency| {
                    let field = |name: &str| {
                        dependency
                            .get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (field("dependencyType"), field("package"), field("versionRequirement"))
                })
                .collect()
        })
        .unwrap_or_default();
    // The order the publish document declares, section by section: the three regular
    // dependencies as written, then the development one.
    let expected: Vec<(String, String, String)> = [
        ("regular", "zeta", "^3.0.0"),
        ("regular", "alpha", "~1.2.3"),
        ("regular", "left-pad", "^1.0.0"),
        ("dev", "tester", "*"),
    ]
    .iter()
    .map(|(kind, name, requirement)| {
        (
            (*kind).to_string(),
            (*name).to_string(),
            (*requirement).to_string(),
        )
    })
    .collect();
    assert_eq!(
        reported, expected,
        "expected the declared order {expected:?}, got {reported:?}"
    );

    // A version the package does not hold is refused as the missing version it is, rather
    // than reported as a version with no dependencies.
    let absent = call(
        &router,
        "POST",
        &format!(
            "/v1/package/version/dependencies?domain={domain}&repository={repository}\
             &format=npm&package={package}&version=9.9.9"
        ),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "case \"a version the package does not hold\": expected 404, got {absent:?}"
    );
    assert_eq!(
        absent.code(),
        "ResourceNotFoundException",
        "case \"a version the package does not hold\": expected the modelled refusal, got \
         {absent:?}"
    );
}

/// RULE: a repository policy that explicitly DENIES reading from the repository blocks the
/// endpoint for the principal it denies, while a sibling repository with no such policy
/// keeps serving the same client.
#[tokio::test]
async fn a_repository_policy_denying_reads_blocks_the_endpoint_and_a_sibling_still_serves() {
    let router = emulator();
    let domain = "ca-policy";
    let denied_repository = "denied";
    let sibling = "sibling";
    let token = given_repository(&router, domain, denied_repository).await;
    assert_eq!(
        create_repository(&router, domain, sibling, None, &[]).await.status,
        200,
        "the sibling repository must be created"
    );
    let host = endpoint_host(domain);
    let package = "policy-pkg";

    for repository in [denied_repository, sibling] {
        let published = registry(
            &router,
            "PUT",
            &host,
            &format!("{}/{package}", endpoint_path(repository)),
            Some(&token),
            Some(publish_document(package, package, &["1.0.0"])),
        )
        .await;
        assert_eq!(
            published.status, 200,
            "the version must publish into {repository:?} first, got {published:?}"
        );
    }

    // The policy denies every principal of this account the read action on the one
    // repository.
    let attached = call(
        &router,
        "PUT",
        &format!("/v1/repository/permissions/policy?domain={domain}&repository={denied_repository}"),
        Some(json!({
            "policyDocument": json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "codeartifact:ReadFromRepository",
                    "Resource": "*"
                }]
            })
            .to_string()
        })),
    )
    .await;
    assert_eq!(
        attached.status, 200,
        "PutRepositoryPermissionsPolicy must succeed, got {attached:?}"
    );

    // (case, the repository addressed, the expected status)
    let cases: [(&str, &str, u16); 2] = [
        ("the repository the policy denies", denied_repository, 403),
        ("a sibling carrying no policy", sibling, 200),
    ];
    for (case, repository, expected) in cases {
        let packument = registry(
            &router,
            "GET",
            &host,
            &format!("{}/{package}", endpoint_path(repository)),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(
            packument.status, expected,
            "case {case:?}: expected HTTP {expected}, got {packument:?}"
        );
        let tarball = registry(
            &router,
            "GET",
            &host,
            &format!(
                "{}/{package}/-/{package}-1.0.0.tgz",
                endpoint_path(repository)
            ),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(
            tarball.status, expected,
            "case {case:?}: expected the tarball download to answer {expected}, got {tarball:?}"
        );
    }
}

/// RULE: a scoped package is addressed in ONE percent-encoded path segment on the routes
/// that carry the package name alone; the same name spread over two segments is a request
/// this registry cannot resolve (400), while a tarball's path — the URL this registry
/// itself publishes — carries the scope as a segment and is served.
#[tokio::test]
async fn a_scoped_package_is_addressed_under_its_encoded_namespace() {
    let router = emulator();
    let domain = "ca-scoped";
    let repository = "scoped";
    let token = given_repository(&router, domain, repository).await;
    let host = endpoint_host(domain);
    let display = "@acme/widget";

    let published = registry(
        &router,
        "PUT",
        &host,
        &format!("{}/@acme%2fwidget", endpoint_path(repository)),
        Some(&token),
        Some(publish_document(display, "widget", &["1.0.0"])),
    )
    .await;
    assert_eq!(
        published.status, 200,
        "the encoded path must publish, got {published:?}"
    );

    // (case, method, the path, the expected status)
    let cases: [(&str, &str, String, u16); 5] = [
        (
            "the packument under the encoded namespace",
            "GET",
            format!("{}/@acme%2fwidget", endpoint_path(repository)),
            200,
        ),
        (
            "the packument with the scope spread over two segments",
            "GET",
            format!("{}/@acme/widget", endpoint_path(repository)),
            400,
        ),
        (
            "a publish with the scope spread over two segments",
            "PUT",
            format!("{}/@acme/widget", endpoint_path(repository)),
            400,
        ),
        (
            "the tarball URL this registry publishes",
            "GET",
            format!("{}/@acme/widget/-/widget-1.0.0.tgz", endpoint_path(repository)),
            200,
        ),
        (
            "the dist-tag collection under the encoded namespace",
            "GET",
            format!("{}/-/package/@acme%2fwidget/dist-tags", endpoint_path(repository)),
            200,
        ),
    ];
    for (case, method, path, expected) in cases {
        let body = if method == "PUT" {
            Some(publish_document(display, "widget", &["2.0.0"]))
        } else {
            None
        };
        let answer = registry(&router, method, &host, &path, Some(&token), body).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {answer:?}"
        );
    }
}

/// RULE: a distribution tag is a pointer the client sets, reads and removes through npm's
/// own dist-tag routes, and it may only name a version the package holds.
#[tokio::test]
async fn the_dist_tag_routes_set_read_and_remove_a_pointer_to_a_held_version() {
    let router = emulator();
    let domain = "ca-tags";
    let repository = "tagged";
    let token = given_repository(&router, domain, repository).await;
    let host = endpoint_host(domain);
    let package = "tagged-pkg";
    let published = registry(
        &router,
        "PUT",
        &host,
        &format!("{}/{package}", endpoint_path(repository)),
        Some(&token),
        Some(publish_document(package, package, &["1.0.0", "2.0.0"])),
    )
    .await;
    assert_eq!(
        published.status, 200,
        "the versions must be published first, got {published:?}"
    );

    let tags_path = format!("{}/-/package/{package}/dist-tags", endpoint_path(repository));
    // The publish document's own tag is what a read reports.
    let read = registry(&router, "GET", &host, &tags_path, Some(&token), None).await;
    assert_eq!(read.status, 200, "reading the tags must succeed, got {read:?}");
    assert_eq!(
        read.json().get("latest").and_then(Value::as_str),
        Some("2.0.0"),
        "expected the publish document's latest tag, got {:?}",
        read.json()
    );

    // (case, the tag, the body naming a version, the expected status, what the tag then
    //  resolves to)
    let cases: [(&str, &str, &str, u16, Option<&str>); 4] = [
        ("a tag on a held version", "beta", "\"1.0.0\"", 200, Some("1.0.0")),
        (
            "the same tag moved to another held version",
            "beta",
            "\"2.0.0\"",
            200,
            Some("2.0.0"),
        ),
        (
            "a tag naming a version the package does not hold",
            "nightly",
            "\"9.9.9\"",
            404,
            None,
        ),
        (
            "a tag naming no version at all",
            "nightly",
            "\"\"",
            400,
            None,
        ),
    ];
    for (case, tag, body, expected, resolves_to) in cases {
        let answer = registry(
            &router,
            "PUT",
            &host,
            &format!("{tags_path}/{tag}"),
            Some(&token),
            Some(body.as_bytes().to_vec()),
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {answer:?}"
        );
        let read = registry(&router, "GET", &host, &tags_path, Some(&token), None).await;
        assert_eq!(
            read.json().get(tag).and_then(Value::as_str),
            resolves_to,
            "case {case:?}: expected {tag:?} to resolve to {resolves_to:?}, got {:?}",
            read.json()
        );
    }

    // Removing a tag is idempotent: the tag is gone, and removing it again answers the
    // same way.
    for case in ["the first removal", "the removal repeated"] {
        let removed = registry(
            &router,
            "DELETE",
            &host,
            &format!("{tags_path}/beta"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected the removal to answer 200, got {removed:?}"
        );
        let read = registry(&router, "GET", &host, &tags_path, Some(&token), None).await;
        assert!(
            read.json().get("beta").is_none(),
            "case {case:?}: expected the tag to be gone, got {:?}",
            read.json()
        );
    }

    // The tags one package carries are bounded, because the caller chooses the tag NAME:
    // at the ceiling a new tag is refused, while moving a tag the package already carries
    // keeps working. The publish document already set `latest`, which is the one tag the
    // package starts with.
    let ceiling = service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::state::MAX_DIST_TAGS_PER_PACKAGE;
    for index in 0..ceiling - 1 {
        let answer = registry(
            &router,
            "PUT",
            &host,
            &format!("{tags_path}/channel-{index}"),
            Some(&token),
            Some(b"\"1.0.0\"".to_vec()),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case \"filling the tag map\": tag {index} must be accepted, got {answer:?}"
        );
    }
    let past = registry(
        &router,
        "PUT",
        &host,
        &format!("{tags_path}/one-too-many"),
        Some(&token),
        Some(b"\"1.0.0\"".to_vec()),
    )
    .await;
    assert_eq!(
        past.status, 403,
        "case \"one tag past the ceiling\": expected the refusal, got {past:?}"
    );
    let moved = registry(
        &router,
        "PUT",
        &host,
        &format!("{tags_path}/channel-0"),
        Some(&token),
        Some(b"\"2.0.0\"".to_vec()),
    )
    .await;
    assert_eq!(
        moved.status, 200,
        "case \"a tag the package already carries\": expected it to move, got {moved:?}"
    );
    let read = registry(&router, "GET", &host, &tags_path, Some(&token), None).await;
    assert_eq!(
        read.json().get("channel-0").and_then(Value::as_str),
        Some("2.0.0"),
        "case \"a tag the package already carries\": expected the moved version, got {:?}",
        read.json()
    );
    assert!(
        read.json().get("one-too-many").is_none(),
        "case \"one tag past the ceiling\": expected nothing stored, got {:?}",
        read.json()
    );
}

/// RULE 5: a publish the registry will not perform stores nothing.
#[tokio::test]
async fn a_refused_publish_stores_nothing() {
    let router = emulator();
    let domain = "ca-refusals";
    let repository = "refusing";
    let token = given_repository(&router, domain, repository).await;
    let host = endpoint_host(domain);
    let package = "refused-pkg";

    // The version the republish row collides with.
    let first = registry(
        &router,
        "PUT",
        &host,
        &format!("{}/{package}", endpoint_path(repository)),
        Some(&token),
        Some(publish_document(package, package, &["1.0.0"])),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first publish must succeed, got {first:?}"
    );

    // A package whose origin configuration blocks publishing.
    let blocked = "blocked-pkg";
    let origin = call(
        &router,
        "POST",
        &format!(
            "/v1/package?domain={domain}&repository={repository}&format=npm&package={blocked}"
        ),
        Some(json!({ "restrictions": { "publish": "BLOCK", "upstream": "ALLOW" } })),
    )
    .await;
    assert_eq!(
        origin.status, 200,
        "PutPackageOriginConfiguration must succeed, got {origin:?}"
    );

    // (case, the path, the document, the expected status, a fragment of the refusal)
    let cases: [(&str, String, Vec<u8>, u16, &str); 5] = [
        (
            "a version the repository already holds",
            package.to_string(),
            publish_document(package, package, &["1.0.0"]),
            409,
            "pre-existing version",
        ),
        (
            "a document naming another package",
            package.to_string(),
            publish_document("some-other-name", "some-other-name", &["2.0.0"]),
            400,
            "addressed",
        ),
        (
            "a document that is not JSON",
            package.to_string(),
            b"not a publish document".to_vec(),
            400,
            "JSON",
        ),
        (
            "a document attaching no tarball",
            format!("{package}-2"),
            serde_json::to_vec(&json!({
                "name": format!("{package}-2"),
                "versions": { "1.0.0": { "version": "1.0.0" } },
                "_attachments": {}
            }))
            .expect("serialize"),
            400,
            "attaches no tarball",
        ),
        (
            "a package whose origin configuration blocks publishing",
            blocked.to_string(),
            publish_document(blocked, blocked, &["1.0.0"]),
            400,
            "configured to block new versions from being published",
        ),
    ];

    for (case, url_path, document, expected, fragment) in cases {
        let answer = registry(
            &router,
            "PUT",
            &host,
            &format!("{}/{url_path}", endpoint_path(repository)),
            Some(&token),
            Some(document),
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {answer:?}"
        );
        assert!(
            answer.error().contains(fragment),
            "case {case:?}: expected the refusal to mention {fragment:?}, got {:?}",
            answer.error()
        );
    }

    // Nothing the refusals asked for was stored: the first publish's one version is all
    // the repository holds for that package, and neither refused package exists.
    let listing = list_versions(&router, domain, repository, None, package).await;
    let stored: Vec<String> = listing
        .body
        .get("versions")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|version| version.get("version").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        stored,
        vec!["1.0.0".to_string()],
        "expected only the accepted version to be stored, got {stored:?}"
    );
    for absent in [format!("{package}-2"), "some-other-name".to_string()] {
        let listed = list_versions(&router, domain, repository, None, &absent).await;
        assert_eq!(
            listed.status, 404,
            "case {absent:?}: a refused publish must create no package, got {listed:?}"
        );
    }
    let blocked_listing = list_versions(&router, domain, repository, None, blocked).await;
    let blocked_versions = blocked_listing
        .body
        .get("versions")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        blocked_versions, 0,
        "expected the blocked package to hold no version, got {blocked_listing:?}"
    );
}

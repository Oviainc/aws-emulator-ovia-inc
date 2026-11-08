// SPDX-License-Identifier: CC0-1.0

//! The PyPI data plane a repository endpoint serves, end to end over the wire.
//!
//! Every case drives the production front door — a `MultiServiceRouter` with this service
//! mounted — with the requests a Python client sends: the `twine`-style
//! `multipart/form-data` POST of a distribution to the endpoint root, `GET
//! <endpoint>simple/<project>/` for the index in both of its renderings, and a `GET` of a
//! link for the file's bytes. What is asserted is the status, the document and the bytes
//! such a client sees, and that what the index served is the same record the CONTROL plane
//! describes.
//!
//! The rules under test, each over a TABLE rather than one example:
//!
//! 1. An upload over the endpoint IS a package version of that repository: it answers `201`,
//!    the project's index then lists the uploaded file pinning its SHA-256, a `GET` of that
//!    link returns the uploaded bytes byte for byte, and `ListPackageVersionAssets` /
//!    `ListPackageVersions` report the same asset name, digest and version. The rows span
//!    what PEP 503 normalisation generalises over — a name carrying underscores, a dotted
//!    name, a capitalised one — and both distribution kinds (a wheel and a source
//!    distribution), because the index path is the NORMALISED name while the file name keeps
//!    what the build wrote.
//! 2. The index is served in both renderings the Simple Repository API defines: the PEP 503
//!    HTML page by default and the PEP 691 JSON document for a client that asks for it by
//!    `Accept`, over the same files.
//! 3. A version's STATUS decides what the index lists and what the endpoint serves:
//!    `Published` is listed and downloadable, `Unlisted` is hidden from the index while its
//!    file still downloads.
//! 4. A file a version already holds is refused, and a second distribution of the same
//!    version is another asset of it.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer};
use crate::npm_registry_endpoint::{endpoint_host, token_for};

/// The pypi endpoint path a repository publishes: `/<format>/<repository>/`.
pub fn endpoint_path(repository: &str) -> String {
    format!("/pypi/{repository}")
}

/// The media type a PEP 691 JSON index carries, which is also how a client asks for it.
pub const JSON_SIMPLE_MEDIA_TYPE: &str = "application/vnd.pypi.simple.v1+json";

/// One raw HTTP answer, reduced to what a Python client branches on.
#[derive(Debug)]
pub struct Raw {
    pub status: u16,
    pub content_type: Option<String>,
    pub www_authenticate: Option<String>,
    pub body: Vec<u8>,
}

impl Raw {
    /// The body as text, for the HTML index.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The body parsed as JSON, or `Null` when it is not JSON (a distribution file).
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The `error` member a refusal of this plane states.
    pub fn error(&self) -> String {
        self.json()
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Send one raw data-plane request — no SigV4 credential, the repository's authorization
/// token instead — and reduce the answer to a [`Raw`].
pub async fn data_plane(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    uri: &str,
    headers: &[(&str, String)],
    body: Vec<u8>,
) -> Raw {
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host);
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let request = request
        .body(bytes::Bytes::from(body))
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

/// The `Authorization` header a `twine` or `pip` client presents: the token as the PASSWORD
/// of an HTTP Basic credential, under the user name CodeArtifact's documentation gives.
pub fn basic_credential(token: &str) -> String {
    use base64::Engine as _;

    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("aws:{token}"))
    )
}

/// The boundary every form this suite submits is delimited by.
const BOUNDARY: &str = "sim2realpypiboundary";

/// One `multipart/form-data` upload document, as the legacy PyPI upload API carries one:
/// `:action=file_upload`, the coordinate, the optional long description and digest, and the
/// distribution file itself.
pub fn upload_form(
    name: &str,
    version: &str,
    file_name: &str,
    content: &[u8],
    description: Option<&str>,
    sha256_digest: Option<&str>,
) -> Vec<u8> {
    let mut fields: Vec<(&str, String)> = vec![
        (":action", "file_upload".to_string()),
        ("protocol_version", "1".to_string()),
        ("name", name.to_string()),
        ("version", version.to_string()),
        ("metadata_version", "2.1".to_string()),
    ];
    if let Some(description) = description {
        fields.push(("description", description.to_string()));
    }
    if let Some(digest) = sha256_digest {
        fields.push(("sha256_digest", digest.to_string()));
    }
    let mut document: Vec<u8> = Vec::new();
    for (field, value) in fields {
        document.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    document.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"content\"; \
             filename=\"{file_name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    document.extend_from_slice(content);
    document.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    document
}

/// The `Content-Type` of such a document.
pub fn upload_media_type() -> String {
    format!("multipart/form-data; boundary={BOUNDARY}")
}

/// The lowercase-hex SHA-256 of `content` — the digest an index pins a file by and the one
/// `ListPackageVersionAssets` reports as `SHA-256`.
pub fn sha256_hex(content: &[u8]) -> String {
    use sha2::Digest as _;

    hex::encode(sha2::Sha256::digest(content))
}

/// One distribution a test uploads: the project as the build spells it, the version, the
/// file name the build wrote, its bytes and the long description declared beside it.
///
/// Passed as one value because it IS one — the form the upload API carries.
struct Distribution<'a> {
    project: &'a str,
    version: &'a str,
    file_name: &'a str,
    content: &'a [u8],
    description: Option<&'a str>,
}

/// POST one distribution to a repository's pypi endpoint.
async fn upload(
    router: &MultiServiceRouter,
    host: &str,
    repository: &str,
    token: &str,
    distribution: &Distribution<'_>,
) -> Raw {
    data_plane(
        router,
        "POST",
        host,
        &format!("{}/", endpoint_path(repository)),
        &[
            ("authorization", basic_credential(token)),
            ("content-type", upload_media_type()),
        ],
        upload_form(
            distribution.project,
            distribution.version,
            distribution.file_name,
            distribution.content,
            distribution.description,
            Some(&sha256_hex(distribution.content)),
        ),
    )
    .await
}

/// GET one path on a repository's pypi endpoint, presenting the token over HTTP Basic.
async fn fetch(
    router: &MultiServiceRouter,
    host: &str,
    uri: &str,
    token: &str,
    accept: Option<&str>,
) -> Raw {
    let mut headers: Vec<(&str, String)> = vec![("authorization", basic_credential(token))];
    if let Some(accept) = accept {
        headers.push(("accept", accept.to_string()));
    }
    data_plane(router, "GET", host, uri, &headers, Vec::new()).await
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

/// `ListPackageVersionAssets` over the control plane, for a pypi coordinate.
async fn list_assets(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    package: &str,
    version: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!(
            "/v1/package/version/assets?domain={domain}&repository={repository}&format=pypi\
             &package={package}&version={version}"
        ),
        Some(serde_json::json!({})),
    )
    .await
}

/// `ListPackageVersions` over the control plane, for a pypi coordinate.
async fn list_versions(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    package: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!(
            "/v1/package/versions?domain={domain}&repository={repository}&format=pypi\
             &package={package}"
        ),
        Some(serde_json::json!({})),
    )
    .await
}

/// `UpdatePackageVersionsStatus` over the control plane, for a pypi coordinate.
async fn update_status(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    package: &str,
    versions: &[&str],
    target: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!(
            "/v1/package/versions/update_status?domain={domain}&repository={repository}\
             &format=pypi&package={package}"
        ),
        Some(serde_json::json!({ "versions": versions, "targetStatus": target })),
    )
    .await
}

/// The `href` values a PEP 503 page carries, in document order.
fn hrefs(page: &str) -> Vec<String> {
    page.split("href=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(href, _)| href.to_string()))
        .collect()
}

/// The path of an absolute URL, which is what a request to this emulator carries.
fn path_of(url: &str) -> String {
    url.split_once("://")
        .and_then(|(_scheme, rest)| rest.find('/').map(|start| rest[start..].to_string()))
        .unwrap_or_else(|| url.to_string())
}

/// One row of the upload table: the case, the project name as the build spells it, the
/// normalised name the index is keyed by, the distribution's file name and its version.
type UploadRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

/// RULES 1 and 2: an upload over the endpoint is a package version of the repository, the
/// index lists its file under the normalised project name with its SHA-256 pinned in both
/// renderings, the link serves the uploaded bytes, and the control plane reports the same
/// asset, digest and version.
#[tokio::test]
async fn an_upload_is_a_package_version_the_index_and_the_control_plane_agree_on() {
    let router = emulator();
    let domain = "ca-pypi";
    let repository = "python";
    let token = given_repository(&router, domain, repository).await;
    let host = endpoint_host(domain);

    let rows: [UploadRow; 4] = [
        (
            "a wheel whose project name carries underscores",
            "sim2real_pkg",
            "sim2real-pkg",
            "sim2real_pkg-1.0.0-py3-none-any.whl",
            "1.0.0",
        ),
        (
            "a source distribution under a dotted name",
            "zope.interface",
            "zope-interface",
            "zope.interface-5.4.0.tar.gz",
            "5.4.0",
        ),
        (
            "a capitalised project name",
            "Django",
            "django",
            "Django-4.2.1-py3-none-any.whl",
            "4.2.1",
        ),
        (
            "a name already normalised",
            "requests",
            "requests",
            "requests-2.31.0.tar.gz",
            "2.31.0",
        ),
    ];

    for (case, declared, normalized, file_name, version) in rows {
        let content = format!("distribution of {declared} {version}\u{0}\u{1}binary")
            .into_bytes();
        let digest = sha256_hex(&content);
        let uploaded = upload(
            &router,
            &host,
            repository,
            &token,
            &Distribution {
                project: declared,
                version,
                file_name,
                content: &content,
                description: None,
            },
        )
        .await;
        assert_eq!(
            uploaded.status, 201,
            "case {case:?}: expected HTTP 201 for the upload, got {uploaded:?}"
        );

        // The PEP 503 page: titled for the project, carrying exactly one link to the file
        // with its digest pinned.
        let index_uri = format!("{}/simple/{normalized}/", endpoint_path(repository));
        let page = fetch(&router, &host, &index_uri, &token, None).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected HTTP 200 for the index, got {page:?}"
        );
        let text = page.text();
        assert!(
            text.contains(&format!("<title>Links for {normalized}</title>")),
            "case {case:?}: expected the index titled for {normalized:?}, got {text:?}"
        );
        let links = hrefs(&text);
        assert_eq!(
            links.len(),
            1,
            "case {case:?}: expected exactly one link, got {links:?}"
        );
        let link = links.first().cloned().unwrap_or_default();
        assert!(
            link.contains(file_name) && link.ends_with(&format!("#sha256={digest}")),
            "case {case:?}: expected a link to {file_name:?} pinning {digest:?}, got {link:?}"
        );

        // The file itself, byte for byte.
        let file = fetch(&router, &host, &path_of(&link), &token, None).await;
        assert_eq!(
            file.status, 200,
            "case {case:?}: expected HTTP 200 for the file, got {file:?}"
        );
        assert_eq!(
            file.body, content,
            "case {case:?}: expected the uploaded bytes back"
        );

        // The PEP 691 rendering of the same index.
        let document = fetch(
            &router,
            &host,
            &index_uri,
            &token,
            Some(JSON_SIMPLE_MEDIA_TYPE),
        )
        .await;
        assert_eq!(
            document.status, 200,
            "case {case:?}: expected HTTP 200 for the JSON index, got {document:?}"
        );
        assert_eq!(
            document.content_type.as_deref(),
            Some(JSON_SIMPLE_MEDIA_TYPE),
            "case {case:?}: expected the JSON index media type, got {document:?}"
        );
        let body = document.json();
        let files = body
            .get("files")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            files.len(),
            1,
            "case {case:?}: expected one file in the JSON index, got {body:?}"
        );
        let entry = files.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            entry.get("filename").and_then(Value::as_str),
            Some(file_name),
            "case {case:?}: expected the JSON index to name {file_name:?}, got {entry:?}"
        );
        assert_eq!(
            entry
                .pointer("/hashes/sha256")
                .and_then(Value::as_str),
            Some(digest.as_str()),
            "case {case:?}: expected the JSON index to pin {digest:?}, got {entry:?}"
        );
        assert_eq!(
            body.get("name").and_then(Value::as_str),
            Some(normalized),
            "case {case:?}: expected the JSON index named {normalized:?}, got {body:?}"
        );

        // And the control plane, which holds the same record.
        let assets = list_assets(&router, domain, repository, normalized, version).await;
        assert_eq!(
            assets.status, 200,
            "case {case:?}: ListPackageVersionAssets must succeed, got {assets:?}"
        );
        let listed = assets
            .body
            .get("assets")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected one asset, got {listed:?}"
        );
        let asset = listed.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            asset.get("name").and_then(Value::as_str),
            Some(file_name),
            "case {case:?}: expected the asset named {file_name:?}, got {asset:?}"
        );
        assert_eq!(
            asset.pointer("/hashes/SHA-256").and_then(Value::as_str),
            Some(digest.as_str()),
            "case {case:?}: expected the asset digest {digest:?}, got {asset:?}"
        );
        let versions = list_versions(&router, domain, repository, normalized).await;
        let reported: Vec<String> = versions
            .body
            .get("versions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|summary| {
                summary
                    .get("version")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(
            reported,
            vec![version.to_string()],
            "case {case:?}: expected the version listed, got {versions:?}"
        );
    }
}

/// RULES 3 and 4: a second distribution of one version is another asset of it, a file the
/// version already holds is refused, and the version's status decides what the index lists
/// against what the endpoint still serves.
#[tokio::test]
async fn a_version_holds_every_distribution_uploaded_for_it_and_its_status_decides_the_index() {
    let router = emulator();
    let domain = "ca-pypi-assets";
    let repository = "python";
    let token = given_repository(&router, domain, repository).await;
    let host = endpoint_host(domain);
    let project = "sim-dual";
    let version = "2.0.0";

    // (case, the file name, its bytes)
    let distributions: [(&str, &str, &[u8]); 2] = [
        ("the wheel", "sim_dual-2.0.0-py3-none-any.whl", b"wheel bytes"),
        ("the source distribution", "sim_dual-2.0.0.tar.gz", b"sdist bytes"),
    ];
    for (case, file_name, content) in distributions {
        let uploaded = upload(
            &router,
            &host,
            repository,
            &token,
            &Distribution {
                project,
                version,
                file_name,
                content,
                description: None,
            },
        )
        .await;
        assert_eq!(
            uploaded.status, 201,
            "case {case:?}: expected HTTP 201, got {uploaded:?}"
        );
    }

    let index_uri = format!("{}/simple/{project}/", endpoint_path(repository));
    let page = fetch(&router, &host, &index_uri, &token, None).await;
    let links = hrefs(&page.text());
    assert_eq!(
        links.len(),
        2,
        "case \"both distributions\": expected two links, got {links:?}"
    );

    // The same file again is refused, and the version keeps the two assets it had.
    let again = upload(
        &router,
        &host,
        repository,
        &token,
        &Distribution {
            project,
            version,
            file_name: "sim_dual-2.0.0.tar.gz",
            content: b"different bytes",
            description: None,
        },
    )
    .await;
    assert_eq!(
        again.status, 400,
        "case \"a file the version already holds\": expected HTTP 400, got {again:?}"
    );
    assert!(
        again.error().contains("already exists"),
        "case \"a file the version already holds\": expected the file-exists sentence, got {:?}",
        again.error()
    );
    let assets = list_assets(&router, domain, repository, project, version).await;
    let count = assets
        .body
        .get("assets")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        count, 2,
        "case \"a refused upload\": expected the two assets to be untouched, got {assets:?}"
    );

    // Unlisted hides the version from the index while its files stay downloadable.
    assert_eq!(
        update_status(
            &router,
            domain,
            repository,
            project,
            &[version],
            "Unlisted"
        )
        .await
        .status,
        200,
        "UpdatePackageVersionsStatus must succeed"
    );
    let hidden = fetch(&router, &host, &index_uri, &token, None).await;
    assert_eq!(
        hidden.status, 404,
        "case \"every version unlisted\": expected the index to list nothing, got {hidden:?}"
    );
    let still = fetch(
        &router,
        &host,
        &format!("{index_uri}sim_dual-2.0.0.tar.gz"),
        &token,
        None,
    )
    .await;
    assert_eq!(
        still.status, 200,
        "case \"every version unlisted\": an unlisted version is still downloadable, got \
         {still:?}"
    );
    assert_eq!(
        still.body,
        b"sdist bytes".to_vec(),
        "case \"every version unlisted\": expected the uploaded bytes back"
    );
}

/// RULE 1, on the refusals: an upload this plane cannot read, or one that declares a digest
/// its bytes do not hash to, stores nothing.
#[tokio::test]
async fn an_upload_the_plane_will_not_accept_stores_nothing() {
    let router = emulator();
    let domain = "ca-pypi-refusals";
    let repository = "python";
    let token = given_repository(&router, domain, repository).await;
    let host = endpoint_host(domain);
    let uri = format!("{}/", endpoint_path(repository));

    // A form whose `:action` is not an upload at all, which this endpoint does not serve.
    let other_action = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\":action\"\r\n\r\nremove_pkg\r\n\
         --{BOUNDARY}--\r\n"
    )
    .into_bytes();
    // (case, the content type, the document, the expected status)
    let rows: [(&str, String, Vec<u8>, u16); 4] = [
        (
            "a body that is not a multipart document",
            "application/json".to_string(),
            b"{}".to_vec(),
            400,
        ),
        (
            "a form asking for another action",
            upload_media_type(),
            other_action,
            400,
        ),
        (
            "a form attaching no distribution file",
            upload_media_type(),
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\":action\"\r\n\r\n\
                 file_upload\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; \
                 name=\"name\"\r\n\r\nsim-refused\r\n--{BOUNDARY}\r\nContent-Disposition: \
                 form-data; name=\"version\"\r\n\r\n1.0.0\r\n--{BOUNDARY}--\r\n"
            )
            .into_bytes(),
            400,
        ),
        (
            "a form declaring a digest the bytes do not hash to",
            upload_media_type(),
            upload_form(
                "sim-refused",
                "1.0.0",
                "sim_refused-1.0.0.tar.gz",
                b"content",
                None,
                Some(&"0".repeat(64)),
            ),
            400,
        ),
    ];
    for (case, content_type, document, expected) in rows {
        let answer = data_plane(
            &router,
            "POST",
            &host,
            &uri,
            &[
                ("authorization", basic_credential(&token)),
                ("content-type", content_type),
            ],
            document,
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {answer:?}"
        );
    }

    let index = fetch(
        &router,
        &host,
        &format!("{}/simple/sim-refused/", endpoint_path(repository)),
        &token,
        None,
    )
    .await;
    assert_eq!(
        index.status, 404,
        "case \"every upload refused\": expected nothing stored, got {index:?}"
    );
}

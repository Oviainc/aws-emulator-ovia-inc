// SPDX-License-Identifier: CC0-1.0

//! `GetPackageVersionReadme`: the README of a package version, end to end over the wire.
//!
//! The rule under test, over a TABLE rather than one example:
//!
//! 1. The readme a version reports is the text its PUBLISH carried, decoded as the UTF-8 it
//!    arrived as — so a multi-byte readme (CJK, Cyrillic, accented Latin, emoji) round-trips
//!    unchanged. The rows span where that text travels: inline in the npm publish document,
//!    inside the published tarball when the document declares none, and the long description
//!    a PyPI upload sends. A version whose publish carried no readme reports none rather
//!    than an empty document.
//! 2. The answer reports the coordinate of the version it read, with the same
//!    `versionRevision` `DescribePackageVersion` reports for it.
//! 3. A coordinate the repository does not hold is refused `ResourceNotFoundException`
//!    (404), naming what was missing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer};
use crate::npm_registry_endpoint::{endpoint_host, registry, token_for};
use crate::pypi_data_plane::{
    basic_credential, data_plane, sha256_hex, upload_form, upload_media_type,
};

/// A gzip-framed tar carrying one file at `path`, which is what an npm publish attaches.
fn tarball(path: &str, content: &[u8]) -> Vec<u8> {
    use std::io::Write as _;

    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, path, content)
        .expect("append the entry");
    let packed = builder.into_inner().expect("finish the archive");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&packed).expect("compress the archive");
    encoder.finish().expect("finish the compression")
}

/// The npm publish document `npm publish` sends for one version: the manifest, with the
/// `readme` member when the publisher declared one, and the tarball attached base64-encoded.
fn publish_document(package: &str, version: &str, readme: Option<&str>, tarball: &[u8]) -> Vec<u8> {
    use base64::Engine as _;

    let mut manifest = json!({
        "name": package,
        "version": version,
        "description": "a package with documentation",
    });
    if let Some(readme) = readme {
        manifest["readme"] = Value::String(readme.to_string());
    }
    serde_json::to_vec(&json!({
        "_id": package,
        "name": package,
        "dist-tags": { "latest": version },
        "versions": { version: manifest },
        "_attachments": {
            format!("{package}-{version}.tgz"): {
                "content_type": "application/octet-stream",
                "data": base64::engine::general_purpose::STANDARD.encode(tarball),
                "length": tarball.len(),
            }
        },
    }))
    .expect("serialize the publish document")
}

/// `GetPackageVersionReadme` over the control plane.
async fn get_readme(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    format: &str,
    package: &str,
    version: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/v1/package/version/readme?domain={domain}&repository={repository}&format={format}\
             &package={package}&version={version}"
        ),
        None,
    )
    .await
}

/// `DescribePackageVersion` over the control plane, for the revision the readme answer must
/// agree with.
async fn describe_version(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    format: &str,
    package: &str,
    version: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/v1/package/version?domain={domain}&repository={repository}&format={format}\
             &package={package}&version={version}"
        ),
        None,
    )
    .await
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

/// The `readme` of an answer, or `None` when it carries none.
fn readme_of(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("readme")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One row of the npm readme table: the case, the package, the readme the document declares
/// inline, the readme packed in the tarball, and the text the API must report.
type NpmRow = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

/// RULES 1 and 2: the readme an npm publish carried is reported byte for byte after UTF-8
/// decoding, whichever way the publish carried it, beside the version's own coordinate and
/// revision.
#[tokio::test]
async fn the_readme_an_npm_publish_carried_is_reported_unchanged() {
    let router = emulator();
    let domain = "ca-readme";
    let repository = "documented";
    let token = given_repository(&router, domain, repository).await;
    let host = endpoint_host(domain);
    let version = "1.0.0";

    // The rows span what the rule generalises over: where the text travelled, and every
    // multi-byte script a README carries.
    let rows: [NpmRow; 6] = [
        (
            "CJK declared inline",
            "readme-cjk",
            Some("# 概要\nこのパッケージは日本語の README を持つ。"),
            None,
            Some("# 概要\nこのパッケージは日本語の README を持つ。"),
        ),
        (
            "Cyrillic declared inline",
            "readme-cyrillic",
            Some("# Обзор\nПривет, мир!"),
            None,
            Some("# Обзор\nПривет, мир!"),
        ),
        (
            "accented Latin and emoji declared inline",
            "readme-emoji",
            Some("# Café ☕\nShip it 🚀 — naïve façade"),
            None,
            Some("# Café ☕\nShip it 🚀 — naïve façade"),
        ),
        (
            "declared only inside the tarball",
            "readme-packed",
            None,
            Some("# Packed\nünïcode from package/README.md"),
            Some("# Packed\nünïcode from package/README.md"),
        ),
        (
            "declared both ways, where the document is the publisher's own declaration",
            "readme-both",
            Some("# Inline wins"),
            Some("# Packed loses"),
            Some("# Inline wins"),
        ),
        ("no readme at all", "readme-absent", None, None, None),
    ];

    for (case, package, inline, packed, expected) in rows {
        let archive = match packed {
            Some(text) => tarball("package/README.md", text.as_bytes()),
            None => tarball("package/index.js", b"module.exports = 1;"),
        };
        let published = registry(
            &router,
            "PUT",
            &host,
            &format!("/npm/{repository}/{package}"),
            Some(&token),
            Some(publish_document(package, version, inline, &archive)),
        )
        .await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: the npm publish must succeed, got {published:?}"
        );

        let answer = get_readme(&router, domain, repository, "npm", package, version).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {answer:?}"
        );
        assert_eq!(
            readme_of(&answer).as_deref(),
            expected,
            "case {case:?}: expected the readme {expected:?}, got {:?}",
            readme_of(&answer)
        );
        // RULE 2: the coordinate and the revision of the version that was read.
        for (member, wanted) in [
            ("format", "npm"),
            ("package", package),
            ("version", version),
        ] {
            assert_eq!(
                answer.body.get(member).and_then(Value::as_str),
                Some(wanted),
                "case {case:?}: expected {member} {wanted:?}, got {answer:?}"
            );
        }
        let described =
            describe_version(&router, domain, repository, "npm", package, version).await;
        assert_eq!(
            answer.body.get("versionRevision").and_then(Value::as_str),
            described
                .body
                .pointer("/packageVersion/revision")
                .and_then(Value::as_str),
            "case {case:?}: expected the revision DescribePackageVersion reports, got \
             {answer:?}"
        );
    }
}

/// RULE 1, for the other format whose publish carries documentation: the long description a
/// PyPI upload sends is the readme the API reports.
#[tokio::test]
async fn the_long_description_a_pypi_upload_sent_is_its_readme() {
    let router = emulator();
    let domain = "ca-readme-pypi";
    let repository = "documented";
    let token = given_repository(&router, domain, repository).await;
    let host = endpoint_host(domain);

    // (case, the project as the build spells it, the normalised name, the description)
    let rows: [(&str, &str, &str, Option<&str>); 3] = [
        (
            "a multi-byte long description",
            "sim_doc",
            "sim-doc",
            Some("# Описание\n説明 — café ☕"),
        ),
        (
            "a plain long description",
            "Sim.Doc.Two",
            "sim-doc-two",
            Some("# Overview\nplain ASCII"),
        ),
        ("an upload declaring none", "sim-doc-bare", "sim-doc-bare", None),
    ];
    for (case, declared, normalized, description) in rows {
        let content = format!("wheel of {declared}").into_bytes();
        let uploaded = data_plane(
            &router,
            "POST",
            &host,
            &format!("/pypi/{repository}/"),
            &[
                ("authorization", basic_credential(&token)),
                ("content-type", upload_media_type()),
            ],
            upload_form(
                declared,
                "1.0.0",
                &format!("{declared}-1.0.0-py3-none-any.whl"),
                &content,
                description,
                Some(&sha256_hex(&content)),
            ),
        )
        .await;
        assert_eq!(
            uploaded.status, 201,
            "case {case:?}: the upload must succeed, got {uploaded:?}"
        );
        let answer = get_readme(&router, domain, repository, "pypi", normalized, "1.0.0").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {answer:?}"
        );
        assert_eq!(
            readme_of(&answer).as_deref(),
            description,
            "case {case:?}: expected the readme {description:?}, got {:?}",
            readme_of(&answer)
        );
    }
}

/// RULE 3: a coordinate the repository does not hold is refused `404`, naming what was
/// missing.
#[tokio::test]
async fn a_coordinate_the_repository_does_not_hold_is_refused() {
    let router = emulator();
    let domain = "ca-readme-refusals";
    let repository = "documented";
    let token = given_repository(&router, domain, repository).await;
    let host = endpoint_host(domain);
    let package = "readme-present";
    let archive = tarball("package/README.md", b"# present");
    assert_eq!(
        registry(
            &router,
            "PUT",
            &host,
            &format!("/npm/{repository}/{package}"),
            Some(&token),
            Some(publish_document(package, "1.0.0", None, &archive)),
        )
        .await
        .status,
        200,
        "the npm publish must succeed"
    );

    // (case, the domain, the repository, the package, the version, the sentence fragment)
    let rows: [(&str, &str, &str, &str, &str, &str); 4] = [
        (
            "a domain this service does not hold",
            "ca-no-such-domain",
            repository,
            package,
            "1.0.0",
            "Domain not found",
        ),
        (
            "a repository the domain does not hold",
            domain,
            "no-such-repository",
            package,
            "1.0.0",
            "Repository not found",
        ),
        (
            "a package the repository does not hold",
            domain,
            repository,
            "no-such-package",
            "1.0.0",
            "does not exist in repository",
        ),
        (
            "a version the package does not hold",
            domain,
            repository,
            package,
            "9.9.9",
            "Package version not found in repo",
        ),
    ];
    for (case, in_domain, in_repository, for_package, for_version, fragment) in rows {
        let answer = get_readme(
            &router,
            in_domain,
            in_repository,
            "npm",
            for_package,
            for_version,
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert!(
            answer.message().contains(fragment),
            "case {case:?}: expected a sentence carrying {fragment:?}, got {:?}",
            answer.message()
        );
    }
}

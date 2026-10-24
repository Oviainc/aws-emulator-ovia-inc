// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for `GetPackageVersionAsset`.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, headers, payload bytes and error body — not a
//! handler's return value. The asset is a streaming `httpPayload` blob and its metadata
//! rides response HEADERS (`X-AssetName`, `X-PackageVersion`,
//! `X-PackageVersionRevision`), which is why this suite reads the raw response rather
//! than the shared JSON `Answer`.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! example:
//!
//! 1. A download answers the bytes `PublishPackageVersion` stored for the addressed
//!    `(domain, repository, format, namespace, package, version, asset)` coordinate, byte
//!    for byte, with the stored asset name, the version and the version's current
//!    `versionRevision` in the headers. Rows differ along asset size (one byte to 64 KiB),
//!    byte values (cycling binary, not text), namespace, the shape of the version string
//!    and the asset name.
//! 2. A version holding several assets answers each one on its own NAME, and the name is
//!    matched exactly.
//! 3. `packageVersionRevision` is a PRECONDITION on the version's content: the version's
//!    current revision downloads, and any other revision is `ConflictException` (409).
//! 4. Every unresolvable step of the coordinate is `ResourceNotFoundException` (404)
//!    naming the resource that was missing in `resourceType`/`resourceId` — domain,
//!    repository, package, package-version or asset.
//! 5. Downloading is a READ: two downloads answer identically and nothing a later caller
//!    sees (the version's description and revision, the domain's `assetSizeBytes`) moves
//!    because an asset was downloaded.
//! 6. A version whose assets `DisposePackageVersions` deleted holds none, so the download
//!    that used to succeed is `ResourceNotFoundException` (404) on the asset.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The HTTP harness the crate's suites share, the publish helpers from the suite that
// owns `PublishPackageVersion`, and the query encoder from the suite that owns it: a
// `versionRevision` is base64, so it carries `+`, `/` and `=` and has to survive the
// server's query decode.
use crate::harness::{auth_for, call, REGION};
use crate::package_version_publish::{
    binary_content, describe, emulator_with_repository, publish, Publish, DOMAIN,
};
use crate::resource_tags::encode_query;

/// One `GetPackageVersionAsset` request, addressed by the whole coordinate an asset is
/// identified by.
#[derive(Debug, Clone)]
struct Get {
    domain: String,
    repository: String,
    format: String,
    namespace: Option<String>,
    package: String,
    version: String,
    asset: String,
    /// The `revision` precondition, sent only when a case pins one.
    revision: Option<String>,
}

impl Get {
    /// The download of exactly what a [`Publish`] stored.
    fn of(published: &Publish) -> Self {
        Self {
            domain: published.domain.clone(),
            repository: published.repository.clone(),
            format: published.format.clone(),
            namespace: published.namespace.clone(),
            package: published.package.clone(),
            version: published.version.clone(),
            asset: published.asset_name.clone(),
            revision: None,
        }
    }

    fn domain(mut self, domain: &str) -> Self {
        self.domain = domain.to_string();
        self
    }

    fn repository(mut self, repository: &str) -> Self {
        self.repository = repository.to_string();
        self
    }

    fn format(mut self, format: &str) -> Self {
        self.format = format.to_string();
        self
    }

    fn namespace(mut self, namespace: &str) -> Self {
        self.namespace = Some(namespace.to_string());
        self
    }

    fn without_namespace(mut self) -> Self {
        self.namespace = None;
        self
    }

    fn package(mut self, package: &str) -> Self {
        self.package = package.to_string();
        self
    }

    fn version(mut self, version: &str) -> Self {
        self.version = version.to_string();
        self
    }

    fn asset(mut self, asset: &str) -> Self {
        self.asset = asset.to_string();
        self
    }

    fn revision(mut self, revision: &str) -> Self {
        self.revision = Some(revision.to_string());
        self
    }

    fn uri(&self) -> String {
        let mut uri = format!(
            "/v1/package/version/asset?domain={}&repository={}&format={}&package={}&version={}&asset={}",
            encode_query(&self.domain),
            encode_query(&self.repository),
            encode_query(&self.format),
            encode_query(&self.package),
            encode_query(&self.version),
            encode_query(&self.asset),
        );
        if let Some(namespace) = &self.namespace {
            uri.push_str(&format!("&namespace={}", encode_query(namespace)));
        }
        if let Some(revision) = &self.revision {
            uri.push_str(&format!("&revision={}", encode_query(revision)));
        }
        uri
    }
}

/// One `GetPackageVersionAsset` answer: the status, the error code, the three metadata
/// headers and the RAW payload bytes.
#[derive(Debug)]
struct Download {
    status: u16,
    error_type: Option<String>,
    asset_name: Option<String>,
    package_version: Option<String>,
    revision: Option<String>,
    payload: Vec<u8>,
}

impl Download {
    /// The `x-amzn-errortype` error code, or `""` on a success.
    fn code(&self) -> String {
        self.error_type.clone().unwrap_or_default()
    }

    /// The refusal body, as the error shape a client parses.
    fn error(&self) -> Value {
        serde_json::from_slice(&self.payload).unwrap_or_else(|_| {
            json!({ "unparsed": String::from_utf8_lossy(&self.payload) })
        })
    }

    /// `resourceType`/`resourceId` of a refusal — which resource was missing.
    fn resource(&self) -> (String, String) {
        let body = self.error();
        (
            body["resourceType"].as_str().unwrap_or_default().to_string(),
            body["resourceId"].as_str().unwrap_or_default().to_string(),
        )
    }

    /// Whether the payload is the modelled error shape — a refusal answers the error,
    /// never asset bytes.
    fn payload_is_error(&self) -> bool {
        self.error()["message"].as_str().is_some()
    }

    /// The revision header, or `""` when the answer carries none.
    fn revision_header(&self) -> String {
        self.revision.clone().unwrap_or_default()
    }
}

/// Send one `GetPackageVersionAsset` and keep the raw response.
async fn download(router: &MultiServiceRouter, request: &Get) -> Download {
    let http_request = http::Request::builder()
        .method("GET")
        .uri(request.uri())
        .header("host", format!("codeartifact.{REGION}.amazonaws.com"))
        .header("authorization", auth_for(REGION))
        .body(bytes::Bytes::new())
        .expect("build request");

    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let error_type = header("x-amzn-errortype");
    let asset_name = header("X-AssetName");
    let package_version = header("X-PackageVersion");
    let revision = header("X-PackageVersionRevision");
    let payload = response.into_body().to_vec();
    Download {
        status,
        error_type,
        asset_name,
        package_version,
        revision,
        payload,
    }
}

/// `DescribeDomain`, for the derived `assetSizeBytes` a read must not move.
async fn describe_domain(router: &MultiServiceRouter, domain: &str) -> Value {
    let answer = call(router, "GET", &format!("/v1/domain?domain={domain}"), None).await;
    assert_eq!(
        answer.status, 200,
        "DescribeDomain {domain} must succeed, got {answer:?}"
    );
    answer.body
}

/// `DisposePackageVersions` for one version of the package a [`Publish`] addresses.
pub(crate) async fn dispose(router: &MultiServiceRouter, published: &Publish) -> Value {
    let mut uri = format!(
        "/v1/package/versions/dispose?domain={}&repository={}&format={}&package={}",
        published.domain, published.repository, published.format, published.package
    );
    if let Some(namespace) = &published.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    let answer = call(
        router,
        "POST",
        &uri,
        Some(json!({ "versions": [published.version] })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "DisposePackageVersions must succeed, got {answer:?}"
    );
    answer.body
}

/// Publish `case` and answer the `versionRevision` the publish reported.
async fn publish_ok(router: &MultiServiceRouter, case: &Publish, label: &str) -> String {
    let published = publish(router, case).await;
    assert_eq!(
        (published.status, published.code()),
        (200, String::new()),
        "case {label:?}: expected a 200 publish, got {published:?}"
    );
    published.body["versionRevision"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {label:?}: a publish must report a versionRevision, got {published:?}")
        })
        .to_string()
}

// ---------------------------------------------------------------------------
// Rule 1: the download answers the stored bytes and the stored metadata.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn published_asset_downloads_byte_for_byte() {
    let router = emulator_with_repository().await;

    // Rows differ along what the rule generalises over: the asset size (one byte up to
    // 64 KiB), whether the bytes are text or binary, the namespace, the shape of the
    // version string and the asset name's punctuation.
    let cases = vec![
        (
            "one byte",
            Publish::new("alpha", "1.0.0", "payload.bin", vec![0x00]),
        ),
        (
            "small text, another namespace",
            Publish::new("beta", "2.0.0-rc.1", "notes.txt", b"hello generic".to_vec())
                .namespace("team-a"),
        ),
        (
            "4 KiB binary",
            Publish::new("gamma", "20240101", "bundle.tar.gz", binary_content(4096, 7))
                .namespace("team-b"),
        ),
        (
            "64 KiB binary",
            Publish::new("delta", "0.0.1", "big.bin", binary_content(64 * 1024, 251)),
        ),
        (
            "punctuated asset name",
            Publish::new("epsilon", "9", "my~asset-1_v2.bin", binary_content(17, 3)),
        ),
    ];

    for (label, case) in &cases {
        let revision = publish_ok(&router, case, label).await;

        let answer = download(&router, &Get::of(case)).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {label:?}: expected a 200 download, got {answer:?}"
        );
        assert_eq!(
            answer.payload,
            case.content,
            "case {label:?}: expected the {} published bytes, got {} bytes",
            case.content.len(),
            answer.payload.len()
        );
        assert_eq!(
            answer.asset_name.as_deref(),
            Some(case.asset_name.as_str()),
            "case {label:?}: expected the stored asset name, got {answer:?}"
        );
        assert_eq!(
            answer.package_version.as_deref(),
            Some(case.version.as_str()),
            "case {label:?}: expected the stored version, got {answer:?}"
        );
        assert_eq!(
            answer.revision_header(),
            revision,
            "case {label:?}: expected the revision the publish reported, got {answer:?}"
        );

        // The revision the download reports is the one the version's own reader reports:
        // one record, two projections of it.
        let described = describe(&router, case).await;
        assert_eq!(
            described.body["packageVersion"]["revision"].as_str(),
            Some(revision.as_str()),
            "case {label:?}: DescribePackageVersion must agree on the revision, got \
             {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: each asset of a version answers on its own name, matched exactly.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_asset_of_a_version_downloads_on_its_own_name() {
    let router = emulator_with_repository().await;

    // Three assets in ONE version: the first two keep it open (`unfinished`), the third
    // closes it. Each carries different bytes, so a handler that answered the wrong
    // asset — or the first one it found — cannot pass.
    let assets: Vec<(&str, Vec<u8>)> = vec![
        ("lib.tar.gz", binary_content(2048, 11)),
        ("CHANGELOG.md", b"# 1.0.0\n\n- first release\n".to_vec()),
        ("checksums.txt", b"deadbeef  lib.tar.gz\n".to_vec()),
    ];

    let mut revision = String::new();
    for (index, (name, content)) in assets.iter().enumerate() {
        let last = index + 1 == assets.len();
        let mut case = Publish::new("multi", "1.0.0", name, content.clone());
        if !last {
            case = case.unfinished(true);
        }
        revision = publish_ok(&router, &case, name).await;
    }

    for (name, content) in &assets {
        let case = Publish::new("multi", "1.0.0", name, content.clone());
        let answer = download(&router, &Get::of(&case)).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {name:?}: expected a 200 download, got {answer:?}"
        );
        assert_eq!(
            answer.payload, *content,
            "case {name:?}: expected that asset's own bytes, got {:?}",
            String::from_utf8_lossy(&answer.payload)
        );
        assert_eq!(
            answer.asset_name.as_deref(),
            Some(*name),
            "case {name:?}: expected the asset name it was asked for, got {answer:?}"
        );
        assert_eq!(
            answer.revision_header(),
            revision,
            "case {name:?}: every asset of one version reports that version's current \
             revision, got {answer:?}"
        );
    }

    // The NAME is matched exactly: a different case, a trailing space or a prefix of a
    // held name is a name the version does not hold.
    for name in ["LIB.TAR.GZ", "lib.tar", "lib.tar.gz.bak", "Lib.Tar.Gz"] {
        let case = Publish::new("multi", "1.0.0", name, Vec::new());
        let answer = download(&router, &Get::of(&case)).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {name:?}: an asset name the version does not hold must be a 404, got \
             {answer:?}"
        );
        assert_eq!(
            answer.resource(),
            ("asset".to_string(), name.to_string()),
            "case {name:?}: the refusal must name the missing asset, got {:?}",
            answer.error()
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: `revision` is a precondition on the version's content.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revision_precondition_admits_only_the_current_revision() {
    let router = emulator_with_repository().await;

    let first = Publish::new("pinned", "1.0.0", "a.bin", binary_content(128, 5));
    let first_revision = publish_ok(&router, &first, "1.0.0").await;
    let second = Publish::new("pinned", "2.0.0", "a.bin", binary_content(128, 6));
    let second_revision = publish_ok(&router, &second, "2.0.0").await;

    // The version's OWN current revision downloads, for each version independently.
    for (label, case, revision) in [
        ("1.0.0", &first, &first_revision),
        ("2.0.0", &second, &second_revision),
    ] {
        let answer = download(&router, &Get::of(case).revision(revision)).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {label:?}: pinning the current revision must download, got {answer:?}"
        );
        assert_eq!(
            answer.payload, case.content,
            "case {label:?}: expected the stored bytes, got {} bytes",
            answer.payload.len()
        );
    }

    // Any OTHER revision is a precondition this version does not meet. Rows differ along
    // where the wrong token comes from: another version's revision (a real token for
    // other content), a shorter opaque token, and one carrying the base64 sigils a
    // revision is spelled with.
    let wrong = vec![
        ("another version's revision", second_revision.clone()),
        ("an opaque token", "r1".to_string()),
        ("base64-shaped token", "AA+B/C==".to_string()),
        (
            "the current revision with one character dropped",
            first_revision[..first_revision.len() - 1].to_string(),
        ),
    ];
    for (label, revision) in &wrong {
        let answer = download(&router, &Get::of(&first).revision(revision)).await;
        assert_eq!(
            (answer.status, answer.code()),
            (409, "ConflictException".to_string()),
            "case {label:?}: a revision the version is not at must be a 409, got {answer:?}"
        );
        let (resource_type, resource_id) = answer.resource();
        assert_eq!(
            (resource_type.as_str(), resource_id.as_str()),
            ("package-version", "team-core/pinned/1.0.0"),
            "case {label:?}: the conflict must name the package version, got {:?}",
            answer.error()
        );
        assert!(
            answer.error()["message"]
                .as_str()
                .unwrap_or_default()
                .contains(&first_revision),
            "case {label:?}: the conflict must state the revision the version IS at, got {:?}",
            answer.error()
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: every unresolvable step names the resource that is missing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unresolvable_coordinate_names_the_missing_resource() {
    let router = emulator_with_repository().await;

    let stored = Publish::new("walk", "1.2.3", "payload.bin", binary_content(64, 2));
    publish_ok(&router, &stored, "walk 1.2.3").await;

    let base = Get::of(&stored);
    // Each row changes ONE component of the coordinate to something the store does not
    // hold, and names the `resourceType`/`resourceId` the refusal must report.
    let cases: Vec<(&str, Get, &str, String)> = vec![
        (
            "unknown domain",
            base.clone().domain("no-such-domain"),
            "domain",
            "no-such-domain".to_string(),
        ),
        (
            "unknown repository",
            base.clone().repository("no-such-repo"),
            "repository",
            "no-such-repo".to_string(),
        ),
        (
            "unknown package name",
            base.clone().package("no-such-package"),
            "package",
            "no-such-package".to_string(),
        ),
        (
            "another format at the same name",
            base.clone().format("npm"),
            "package",
            stored.package.clone(),
        ),
        (
            "another namespace at the same name",
            base.clone().namespace("other-team"),
            "package",
            stored.package.clone(),
        ),
        (
            "no namespace at all",
            base.clone().without_namespace(),
            "package",
            stored.package.clone(),
        ),
        (
            "unknown version",
            base.clone().version("9.9.9"),
            "package-version",
            "team-core/walk/9.9.9".to_string(),
        ),
        (
            "unknown asset",
            base.clone().asset("no-such-asset.bin"),
            "asset",
            "no-such-asset.bin".to_string(),
        ),
    ];

    for (label, request, resource_type, resource_id) in &cases {
        let answer = download(&router, request).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected a 404 ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.resource(),
            ((*resource_type).to_string(), resource_id.clone()),
            "case {label:?}: expected the refusal to name {resource_type} {resource_id}, got \
             {:?}",
            answer.error()
        );
        assert!(
            answer.payload_is_error(),
            "case {label:?}: a refusal carries the error shape, never asset bytes, got {answer:?}"
        );
    }

    // The coordinate that IS held still downloads, so none of the rows above passed by
    // breaking the baseline.
    let answer = download(&router, &base).await;
    assert_eq!(
        (answer.status, answer.code()),
        (200, String::new()),
        "the stored coordinate must still download, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: downloading is a read.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn download_changes_nothing_a_later_caller_sees() {
    let router = emulator_with_repository().await;

    let cases = vec![
        ("one byte", Publish::new("ro", "1.0.0", "a.bin", vec![0x7f])),
        (
            "8 KiB binary",
            Publish::new("ro", "1.0.1", "b.bin", binary_content(8192, 13)),
        ),
    ];
    for (label, case) in &cases {
        publish_ok(&router, case, label).await;
    }

    let before_domain = describe_domain(&router, DOMAIN).await;

    for (label, case) in &cases {
        let before = describe(&router, case).await;
        // Read the same asset three times: a read that consumed, expired or bumped
        // anything would show up here.
        let first = download(&router, &Get::of(case)).await;
        let second = download(&router, &Get::of(case)).await;
        let third = download(&router, &Get::of(case).revision(&first.revision_header())).await;
        let after = describe(&router, case).await;

        for (index, answer) in [&first, &second, &third].iter().enumerate() {
            assert_eq!(
                (answer.status, answer.code()),
                (200, String::new()),
                "case {label:?} download {index}: expected a 200, got {answer:?}"
            );
            assert_eq!(
                answer.payload, case.content,
                "case {label:?} download {index}: expected the same bytes every time, got {} \
                 bytes",
                answer.payload.len()
            );
            assert_eq!(
                answer.revision_header(),
                first.revision_header(),
                "case {label:?} download {index}: the revision must not move, got {answer:?}"
            );
        }
        assert_eq!(
            before.body, after.body,
            "case {label:?}: DescribePackageVersion must report the same record after three \
             downloads"
        );
    }

    let after_domain = describe_domain(&router, DOMAIN).await;
    assert_eq!(
        before_domain["domain"]["assetSizeBytes"], after_domain["domain"]["assetSizeBytes"],
        "a download must not move the domain's assetSizeBytes: {before_domain:?} then \
         {after_domain:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: a disposed version holds no asset to download.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disposed_version_holds_no_asset_to_download() {
    let router = emulator_with_repository().await;

    let cases = vec![
        (
            "text asset",
            Publish::new("gone", "1.0.0", "notes.txt", b"bye".to_vec()),
        ),
        (
            "binary asset",
            Publish::new("gone", "2.0.0", "blob.bin", binary_content(512, 23)),
        ),
    ];

    for (label, case) in &cases {
        publish_ok(&router, case, label).await;
        let before = download(&router, &Get::of(case)).await;
        assert_eq!(
            (before.status, before.code()),
            (200, String::new()),
            "case {label:?}: the published asset must download before it is disposed, got \
             {before:?}"
        );

        dispose(&router, case).await;

        let after = download(&router, &Get::of(case)).await;
        assert_eq!(
            (after.status, after.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: a disposed version's assets are deleted, so the download is a \
             404, got {after:?}"
        );
        assert_eq!(
            after.resource(),
            ("asset".to_string(), case.asset_name.clone()),
            "case {label:?}: the refusal must name the asset that is gone, got {:?}",
            after.error()
        );

        // The VERSION is still held — disposal keeps the record — so the refusal above is
        // about the asset, not about a version that vanished.
        let described = describe(&router, case).await;
        assert_eq!(
            described.body["packageVersion"]["status"].as_str(),
            Some("Disposed"),
            "case {label:?}: the disposed version must still be described, got {described:?}"
        );
    }
}

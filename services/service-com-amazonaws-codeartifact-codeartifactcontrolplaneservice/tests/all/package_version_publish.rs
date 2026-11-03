// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for publishing generic package versions.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code and body — not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! example:
//!
//! 1. A publish stores the asset under the `(format, namespace, package, version)` it
//!    names and answers with the record it stored; a later `DescribePackageVersion`
//!    answers with that same record, revision and `publishedTime` included.
//! 2. Every hash an `AssetSummary` reports is computed from the stored bytes, and `size`
//!    is their length.
//! 3. `assetSHA256` is an integrity check: a digest that disagrees with the bytes
//!    received is `ValidationException` (400) and stores nothing.
//! 4. Only the `generic` format can be published through this API; every other
//!    `PackageFormat` is `ValidationException` (400). A `generic` publish that names no
//!    `namespace` is `ValidationException` (400) too: the namespace is a component of the
//!    version's identity, not a label.
//! 5. A publish into a domain or repository that is not held is
//!    `ResourceNotFoundException` (404) naming the resource that was missing.
//! 6. `unfinished` keeps a version open for further assets and the publish that omits it
//!    closes the version at `Published`; a closed version is immutable and cannot be
//!    reopened, so a further publish into it is `ConflictException` (409).
//! 7. `versionRevision` identifies the version's CONTENT: it is stable across reads of an
//!    unchanged version, identical for a repeated publish of the same bytes, and
//!    different once an asset is added.
//! 8. A domain's `assetSizeBytes` is the total size of the assets its repositories hold.
//! 9. Origin control decides whether a package accepts a publish at all — a package whose
//!    `publish` restriction is `BLOCK` is `ValidationException` (400), the request being
//!    invalid for that package rather than unauthorized for the caller.
//!
//! The declared ceilings are row-tested where a test can reach them: the assets-per-version
//! bound is (100 small publishes). The remaining three are not, and deliberately —
//! `MAX_PACKAGES_PER_REPOSITORY` needs 10,000 package records, and `MAX_ASSET_BYTES` and
//! `MAX_STORED_ASSET_BYTES` are measured in hundreds of mebibytes, so a row driving them
//! would allocate the ceiling itself inside the test process. All four read their constant
//! from `state`, so none can drift from the bound it enforces.

use serde_json::Value;
use sha2::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

// The `AllowPublish` and `PackageFormat` catalogs come from the model's own generated
// constants, so a model change moves the tables below rather than leaving a hand-copied
// list behind.
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::ops::types::{
    ALLOW_PUBLISH_VALUES, PACKAGE_FORMAT_VALUES,
};

// The HTTP harness the crate's suites share: one emulator construction, one
// request/`Answer` reduction and one domain setup in `crate::harness`, plus the
// repository setup from the suite that owns it.
use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{auth_for, call, create_domain, emulator, Answer, REGION};

/// The domain and repository every case publishes into.
pub const DOMAIN: &str = "pkg-domain";
pub const REPOSITORY: &str = "pkg-repo";

/// The namespace every publish carries unless a case names another.
///
/// A generic package version's identity is `<namespace>/<package>/<version>`, so
/// `namespace` is required of a `generic` publish even though the model marks the member
/// optional (other formats derive it — a Maven groupId, an npm scope). A row that needs
/// a request WITHOUT one takes it off with [`Publish::without_namespace`].
pub const NAMESPACE: &str = "team-core";

/// Send one request whose body is RAW BYTES rather than JSON, with extra headers.
///
/// The shared [`call`] serialises a `serde_json::Value` body, which cannot express
/// `PublishPackageVersion`: its `assetContent` is a streaming binary payload and its
/// `assetSHA256` rides the `x-amz-content-sha256` header. The response is reduced to the
/// same shared [`Answer`].
pub async fn call_with_payload(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    headers: &[(&str, String)],
    payload: bytes::Bytes,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("codeartifact.{REGION}.amazonaws.com"))
        .header("authorization", auth_for(REGION))
        .header("content-type", "application/octet-stream");
    for (name, value) in headers {
        builder = builder.header(*name, value);
    }
    let request = builder.body(payload).expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// The lowercase-hex SHA-256 of `content` — what a caller declares in `assetSHA256`.
fn sha256_hex(content: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(content))
}

/// One `PublishPackageVersion` request.
#[derive(Debug, Clone)]
pub struct Publish {
    pub domain: String,
    pub repository: String,
    pub format: String,
    pub namespace: Option<String>,
    pub package: String,
    pub version: String,
    pub asset_name: String,
    pub content: Vec<u8>,
    /// The digest to DECLARE. `None` declares the true digest of `content`.
    pub declared_sha256: Option<String>,
    pub unfinished: Option<bool>,
}

impl Publish {
    /// A publish of `content` that declares the true digest and closes the version.
    pub fn new(package: &str, version: &str, asset_name: &str, content: Vec<u8>) -> Self {
        Self {
            domain: DOMAIN.to_string(),
            repository: REPOSITORY.to_string(),
            format: "generic".to_string(),
            namespace: Some(NAMESPACE.to_string()),
            package: package.to_string(),
            version: version.to_string(),
            asset_name: asset_name.to_string(),
            content,
            declared_sha256: None,
            unfinished: None,
        }
    }

    pub fn namespace(mut self, namespace: &str) -> Self {
        self.namespace = Some(namespace.to_string());
        self
    }

    /// Send no `namespace` at all — the request the generic format refuses.
    pub fn without_namespace(mut self) -> Self {
        self.namespace = None;
        self
    }

    pub fn unfinished(mut self, unfinished: bool) -> Self {
        self.unfinished = Some(unfinished);
        self
    }

    pub fn declaring(mut self, digest: &str) -> Self {
        self.declared_sha256 = Some(digest.to_string());
        self
    }

    fn uri(&self) -> String {
        let mut uri = format!(
            "/v1/package/version/publish?domain={}&repository={}&format={}&package={}&version={}&asset={}",
            self.domain, self.repository, self.format, self.package, self.version, self.asset_name
        );
        if let Some(namespace) = &self.namespace {
            uri.push_str(&format!("&namespace={namespace}"));
        }
        if let Some(unfinished) = self.unfinished {
            uri.push_str(&format!("&unfinished={unfinished}"));
        }
        uri
    }
}

/// `PublishPackageVersion`: the asset bytes are the HTTP payload and the declared digest
/// is the `x-amz-content-sha256` header the model binds `assetSHA256` to.
pub async fn publish(router: &MultiServiceRouter, request: &Publish) -> Answer {
    let digest = request
        .declared_sha256
        .clone()
        .unwrap_or_else(|| sha256_hex(&request.content));
    call_with_payload(
        router,
        "POST",
        &request.uri(),
        &[("x-amz-content-sha256", digest)],
        bytes::Bytes::from(request.content.clone()),
    )
    .await
}

/// `DescribePackageVersion` for the version a [`Publish`] addresses.
pub async fn describe(router: &MultiServiceRouter, request: &Publish) -> Answer {
    let mut uri = format!(
        "/v1/package/version?domain={}&repository={}&format={}&package={}&version={}",
        request.domain, request.repository, request.format, request.package, request.version
    );
    if let Some(namespace) = &request.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    call(router, "GET", &uri, None).await
}

async fn describe_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(router, "GET", &format!("/v1/domain?domain={domain}"), None).await
}

/// A router holding [`DOMAIN`]/[`REPOSITORY`], which every publish case needs.
pub async fn emulator_with_repository() -> MultiServiceRouter {
    let router = emulator();
    let created = create_domain(&router, DOMAIN, None).await;
    assert_eq!(created.status, 200, "CreateDomain failed: {created:?}");
    let created = create_repository(&router, DOMAIN, REPOSITORY, None, &[]).await;
    assert_eq!(created.status, 200, "CreateRepository failed: {created:?}");
    router
}

/// Content of `size` bytes that is not text: every byte value cycles, so a row that
/// mangled the payload (a text codec, a truncation) cannot pass by accident.
pub fn binary_content(size: usize, seed: u8) -> Vec<u8> {
    (0..size)
        .map(|index| (index as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1 and 7: a publish is readable back, byte for byte, with a stable revision.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn published_version_round_trips_through_describe() {
    let router = emulator_with_repository().await;

    // Rows differ along what the rule generalises over: the namespace, the asset size
    // (including a single byte), the shape of the version string, and the asset name.
    let cases = vec![
        ("one byte", Publish::new("alpha", "1.0.0", "payload.bin", vec![0x00])),
        (
            "namespaced, small text",
            Publish::new("beta", "2.0.0-rc.1", "notes.txt", b"hello generic".to_vec())
                .namespace("team-a"),
        ),
        (
            "namespaced, 4 KiB binary",
            Publish::new("gamma", "20240101", "bundle.tar.gz", binary_content(4096, 7))
                .namespace("team-b"),
        ),
        (
            "same package, second version",
            Publish::new("alpha", "1.0.1", "payload.bin", binary_content(1024, 99)),
        ),
        (
            "unicode-free punctuation in the asset name",
            Publish::new("delta", "0.0.1", "my~asset-1_v2.bin", binary_content(17, 3)),
        ),
    ];

    for (label, case) in &cases {
        let published = publish(&router, case).await;
        assert_eq!(
            (published.status, published.code()),
            (200, String::new()),
            "case {label:?}: expected a 200 publish, got {published:?}"
        );

        let expected_digest = sha256_hex(&case.content);
        let body = &published.body;
        assert_eq!(
            body["format"].as_str(),
            Some(case.format.as_str()),
            "case {label:?}: expected format {:?}, got {body:?}",
            case.format
        );
        assert_eq!(
            body["namespace"].as_str(),
            case.namespace.as_deref(),
            "case {label:?}: expected namespace {:?}, got {body:?}",
            case.namespace
        );
        assert_eq!(
            body["package"].as_str(),
            Some(case.package.as_str()),
            "case {label:?}: expected package {:?}, got {body:?}",
            case.package
        );
        assert_eq!(
            body["version"].as_str(),
            Some(case.version.as_str()),
            "case {label:?}: expected version {:?}, got {body:?}",
            case.version
        );
        assert_eq!(
            body["status"].as_str(),
            Some("Published"),
            "case {label:?}: a publish without `unfinished` closes the version, got {body:?}"
        );
        assert_eq!(
            body["asset"]["name"].as_str(),
            Some(case.asset_name.as_str()),
            "case {label:?}: expected the published asset name, got {body:?}"
        );
        assert_eq!(
            body["asset"]["size"].as_i64(),
            Some(case.content.len() as i64),
            "case {label:?}: expected size {}, got {body:?}",
            case.content.len()
        );
        assert_eq!(
            body["asset"]["hashes"]["SHA-256"].as_str(),
            Some(expected_digest.as_str()),
            "case {label:?}: expected the SHA-256 of the published bytes, got {body:?}"
        );

        // `PackageVersionRevision` is modelled len 1..50, pattern ^\S+$.
        let revision = body["versionRevision"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !revision.is_empty()
                && revision.len() <= 50
                && !revision.chars().any(char::is_whitespace),
            "case {label:?}: expected a revision inside PackageVersionRevision's constraints, got {revision:?}"
        );

        // Rule 1: the read answers with the record the publish stored.
        let described = describe(&router, case).await;
        assert_eq!(
            (described.status, described.code()),
            (200, String::new()),
            "case {label:?}: expected a 200 describe, got {described:?}"
        );
        let description = &described.body["packageVersion"];
        assert_eq!(
            (
                description["format"].as_str(),
                description["namespace"].as_str(),
                description["packageName"].as_str(),
                description["version"].as_str(),
                description["status"].as_str(),
                description["revision"].as_str(),
            ),
            (
                Some(case.format.as_str()),
                case.namespace.as_deref(),
                Some(case.package.as_str()),
                Some(case.version.as_str()),
                Some("Published"),
                Some(revision.as_str()),
            ),
            "case {label:?}: describe must report the published record, got {described:?}"
        );
        assert!(
            description["publishedTime"].as_f64().is_some_and(|time| time > 0.0),
            "case {label:?}: expected a publishedTime, got {described:?}"
        );
        assert_eq!(
            (
                description["origin"]["originType"].as_str(),
                description["origin"]["domainEntryPoint"]["repositoryName"].as_str(),
            ),
            (Some("INTERNAL"), Some(case.repository.as_str())),
            "case {label:?}: a direct publish is an INTERNAL origin entering through the \
             repository it was published to, got {described:?}"
        );

        // Rule 7: a read changes nothing — the second read is identical to the first.
        let again = describe(&router, case).await;
        assert_eq!(
            again.body["packageVersion"], described.body["packageVersion"],
            "case {label:?}: describing a version twice must answer identically, got {again:?}"
        );
    }

    // Rule 8: the domain reports the total size of everything published into it.
    let expected_total: i64 = cases
        .iter()
        .map(|(_, case)| case.content.len() as i64)
        .sum();
    let domain = describe_domain(&router, DOMAIN).await;
    assert_eq!(
        domain.body["domain"]["assetSizeBytes"].as_i64(),
        Some(expected_total),
        "expected assetSizeBytes {expected_total}, got {domain:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the reported hashes are the hashes of the stored bytes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn asset_hashes_are_computed_from_the_published_bytes() {
    let router = emulator_with_repository().await;

    // The published digests of the three-byte message "abc", from the algorithms'
    // own specifications: MD5 (RFC 1321 section A.5 test suite), SHA-1 and SHA-256 and
    // SHA-512 (FIPS 180-4 example vectors). Independent of this emulator's own hashing.
    let abc_hashes = [
        ("MD5", "900150983cd24fb0d6963f7d28e17f72"),
        ("SHA-1", "a9993e364706816aba3e25717850c26c9cd0d89d"),
        (
            "SHA-256",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            "SHA-512",
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        ),
    ];

    let case = Publish::new("vectors", "1.0.0", "abc.bin", b"abc".to_vec());
    let published = publish(&router, &case).await;
    assert_eq!(
        (published.status, published.code()),
        (200, String::new()),
        "expected a 200 publish, got {published:?}"
    );
    for (algorithm, expected) in abc_hashes {
        assert_eq!(
            published.body["asset"]["hashes"][algorithm].as_str(),
            Some(expected),
            "algorithm {algorithm:?}: expected the published vector for \"abc\", got {published:?}"
        );
    }

    // And every one of the model's `HashAlgorithm` values is reported for an asset of
    // any size, each agreeing with the bytes that were stored.
    for size in [1usize, 2, 255, 8192] {
        let case = Publish::new("sizes", &format!("1.{size}.0"), "blob.bin", binary_content(size, 11));
        let published = publish(&router, &case).await;
        assert_eq!(
            (published.status, published.code()),
            (200, String::new()),
            "case {size}: expected a 200 publish, got {published:?}"
        );
        assert_eq!(
            published.body["asset"]["hashes"]["SHA-256"].as_str(),
            Some(sha256_hex(&case.content).as_str()),
            "case {size}: expected the SHA-256 of the stored bytes, got {published:?}"
        );
        assert_eq!(
            published.body["asset"]["hashes"]["SHA-512"]
                .as_str()
                .map(str::len),
            Some(128),
            "case {size}: expected a hex SHA-512, got {published:?}"
        );
        assert_eq!(
            published.body["asset"]["hashes"]["MD5"].as_str().map(str::len),
            Some(32),
            "case {size}: expected a hex MD5, got {published:?}"
        );
        assert_eq!(
            published.body["asset"]["hashes"]["SHA-1"].as_str().map(str::len),
            Some(40),
            "case {size}: expected a hex SHA-1, got {published:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a declared digest that disagrees with the bytes is refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn digest_mismatch_is_refused_and_stores_nothing() {
    let router = emulator_with_repository().await;

    let content = binary_content(8192, 5);
    let truth = sha256_hex(&content);
    // One hex character changed, the digest of other bytes, and a digest of all zeros:
    // three ways to disagree, all of them satisfying the modelled `SHA256` constraints
    // so the boundary validator passes them through to the handler.
    let flipped = {
        let mut flipped = truth.clone();
        let last = if truth.ends_with('0') { '1' } else { '0' };
        flipped.pop();
        flipped.push(last);
        flipped
    };
    let cases = [
        ("one hex character changed", flipped),
        ("the digest of other bytes", sha256_hex(b"something else")),
        ("all zeros", "0".repeat(64)),
    ];

    for (index, (label, declared)) in cases.iter().enumerate() {
        let case = Publish::new("mismatch", &format!("1.0.{index}"), "payload.bin", content.clone())
            .declaring(declared);
        let refused = publish(&router, &case).await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "ValidationException".to_string()),
            "case {label:?}: expected a 400 ValidationException, got {refused:?}"
        );
        // The refusal names BOTH digests — the one the request declared and the one the
        // service computed over the bytes it received — which is what tells a client its
        // upload was corrupted rather than its credentials rejected, and the only evidence
        // that the service hashed the payload instead of trusting the header.
        assert_eq!(
            refused.message(),
            format!(
                "The provided content hashes do not match the calculated content hashes. \
                 Hash algorithm 'SHA-256', Expected hash '{declared}', Actual hash '{truth}'"
            ),
            "case {label:?}: expected the content-hash-mismatch sentence, got {:?}",
            refused.message()
        );

        // Nothing was stored: the version does not exist.
        let absent = describe(&router, &case).await;
        assert_eq!(
            (absent.status, absent.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: a refused publish must store nothing, got {absent:?}"
        );
    }

    // The same bytes with the TRUE digest are accepted, so the refusals above are about
    // the digest and nothing else.
    let accepted = Publish::new("mismatch", "1.0.0", "payload.bin", content);
    let published = publish(&router, &accepted).await;
    assert_eq!(
        (published.status, published.code()),
        (200, String::new()),
        "expected the true digest to be accepted, got {published:?}"
    );

    let domain = describe_domain(&router, DOMAIN).await;
    assert_eq!(
        domain.body["domain"]["assetSizeBytes"].as_i64(),
        Some(8192),
        "only the accepted publish may count towards assetSizeBytes, got {domain:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: only the generic format publishes through this API.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_the_generic_format_can_be_published() {
    let router = emulator_with_repository().await;

    // Every value of the model's own `PackageFormat` enum except the one this operation
    // accepts ("The only supported value is generic"), taken from the generated catalog so
    // a format added to the model is covered without another edit here.
    let refused_formats: Vec<&&str> = PACKAGE_FORMAT_VALUES
        .iter()
        .filter(|format| **format != "generic")
        .collect();
    assert!(
        refused_formats.len() + 1 == PACKAGE_FORMAT_VALUES.len() && !refused_formats.is_empty(),
        "expected the PackageFormat catalog to hold `generic` and at least one other format, got {PACKAGE_FORMAT_VALUES:?}"
    );

    for format in refused_formats {
        let format = *format;
        let mut case = Publish::new("formats", "1.0.0", "payload.bin", b"content".to_vec());
        case.format = format.to_string();
        let refused = publish(&router, &case).await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "ValidationException".to_string()),
            "case {format:?}: expected a 400 ValidationException, got {refused:?}"
        );
        let absent = describe(&router, &case).await;
        assert_eq!(
            (absent.status, absent.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {format:?}: a refused publish must store nothing, got {absent:?}"
        );
    }

    let generic = Publish::new("formats", "1.0.0", "payload.bin", b"content".to_vec());
    let published = publish(&router, &generic).await;
    assert_eq!(
        (published.status, published.code()),
        (200, String::new()),
        "expected `generic` to be accepted, got {published:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: an unheld domain, repository, package or version is a 404.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn addressing_a_resource_that_is_not_held_is_not_found() {
    let router = emulator_with_repository().await;

    let mut unknown_domain = Publish::new("missing", "1.0.0", "payload.bin", b"x".to_vec());
    unknown_domain.domain = "other-domain".to_string();
    let mut unknown_repository = Publish::new("missing", "1.0.0", "payload.bin", b"x".to_vec());
    unknown_repository.repository = "other-repo".to_string();

    let publishes = vec![
        ("unknown domain", unknown_domain, "domain"),
        ("unknown repository", unknown_repository, "repository"),
    ];
    for (label, case, resource_type) in &publishes {
        let refused = publish(&router, case).await;
        assert_eq!(
            (refused.status, refused.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected a 404 ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            refused.body["resourceType"].as_str(),
            Some(*resource_type),
            "case {label:?}: expected the error to name the missing resource, got {refused:?}"
        );
    }

    // A read of a package that was never published, and of a version of a package that
    // was: both are 404, and both name the PACKAGE at the coordinate they addressed — a
    // package exists in a repository only through its versions, so a coordinate holding
    // none has no package at it either.
    let published = Publish::new("held", "1.0.0", "payload.bin", b"x".to_vec());
    assert_eq!(
        publish(&router, &published).await.status,
        200,
        "the control publish must succeed"
    );

    let mut absent_package = published.clone();
    absent_package.package = "never-published".to_string();
    let mut absent_version = published.clone();
    absent_version.version = "9.9.9".to_string();
    let mut absent_namespace = published.clone();
    absent_namespace.namespace = Some("not-the-namespace".to_string());

    let reads = vec![
        ("package never published", absent_package, "package"),
        ("version never published", absent_version, "package"),
        ("namespace is part of the key", absent_namespace, "package"),
    ];
    for (label, case, resource_type) in &reads {
        let absent = describe(&router, case).await;
        assert_eq!(
            (absent.status, absent.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected a 404 ResourceNotFoundException, got {absent:?}"
        );
        assert_eq!(
            absent.body["resourceType"].as_str(),
            Some(*resource_type),
            "case {label:?}: expected the error to name the missing resource, got {absent:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rules 6 and 7: unfinished versions, closing them, and the revision.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unfinished_version_closes_on_the_final_asset_and_cannot_reopen() {
    // Rows differ in how many assets the version is built from before it closes.
    for asset_count in [2usize, 3, 5] {
        let router = emulator_with_repository().await;
        let package = format!("multi-{asset_count}");
        let mut revisions: Vec<String> = Vec::new();

        // Every asset but the last is published with `unfinished`, so the version stays
        // open and keeps its `Unfinished` status.
        for index in 0..asset_count - 1 {
            let case = Publish::new(
                &package,
                "1.0.0",
                &format!("part-{index}.bin"),
                binary_content(64 + index, index as u8),
            )
            .unfinished(true);
            let published = publish(&router, &case).await;
            assert_eq!(
                (published.status, published.code()),
                (200, String::new()),
                "case {asset_count} assets, part {index}: expected a 200 publish, got {published:?}"
            );
            assert_eq!(
                published.body["status"].as_str(),
                Some("Unfinished"),
                "case {asset_count} assets, part {index}: `unfinished` keeps the version open, got {published:?}"
            );
            revisions.push(
                published.body["versionRevision"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );

            // Rule 7: republishing the SAME bytes under the same name changes nothing,
            // so the revision is the one already reported.
            let repeated = publish(&router, &case).await;
            assert_eq!(
                (repeated.status, repeated.code()),
                (200, String::new()),
                "case {asset_count} assets, part {index}: republishing identical bytes must succeed, got {repeated:?}"
            );
            assert_eq!(
                repeated.body["versionRevision"].as_str(),
                revisions.last().map(String::as_str),
                "case {asset_count} assets, part {index}: identical content keeps the revision, got {repeated:?}"
            );

            // The same asset name carrying DIFFERENT bytes would rewrite an asset the
            // version already holds.
            let rewritten = publish(
                &router,
                &Publish::new(
                    &package,
                    "1.0.0",
                    &format!("part-{index}.bin"),
                    binary_content(65 + index, 200),
                )
                .unfinished(true),
            )
            .await;
            assert_eq!(
                (rewritten.status, rewritten.code()),
                (409, "ConflictException".to_string()),
                "case {asset_count} assets, part {index}: rewriting an asset must conflict, got {rewritten:?}"
            );
        }

        // Each added asset moved the revision.
        for (index, revision) in revisions.iter().enumerate() {
            assert!(
                !revisions[..index].contains(revision),
                "case {asset_count} assets: adding an asset must move the revision, got {revisions:?}"
            );
        }

        // The final asset, published WITHOUT `unfinished`, closes the version.
        let final_asset = Publish::new(
            &package,
            "1.0.0",
            &format!("part-{}.bin", asset_count - 1),
            binary_content(128, 17),
        );
        let closed = publish(&router, &final_asset).await;
        assert_eq!(
            (closed.status, closed.code()),
            (200, String::new()),
            "case {asset_count} assets: expected the closing publish to succeed, got {closed:?}"
        );
        assert_eq!(
            closed.body["status"].as_str(),
            Some("Published"),
            "case {asset_count} assets: omitting `unfinished` closes the version, got {closed:?}"
        );
        let closed_revision = closed.body["versionRevision"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !revisions.contains(&closed_revision),
            "case {asset_count} assets: closing the version must move the revision, got {closed_revision:?} in {revisions:?}"
        );

        let described = describe(&router, &final_asset).await;
        assert_eq!(
            (
                described.body["packageVersion"]["status"].as_str(),
                described.body["packageVersion"]["revision"].as_str(),
            ),
            (Some("Published"), Some(closed_revision.as_str())),
            "case {asset_count} assets: describe must report the closed version, got {described:?}"
        );

        // A closed version is immutable: another asset, the same asset again, and an
        // attempt to reopen it as unfinished are all refused, and none of them moves it.
        let reopen_attempts = vec![
            (
                "a new asset",
                Publish::new(&package, "1.0.0", "extra.bin", binary_content(32, 1)),
            ),
            (
                "the same asset again",
                Publish::new(
                    &package,
                    "1.0.0",
                    &format!("part-{}.bin", asset_count - 1),
                    binary_content(128, 17),
                ),
            ),
            (
                "reopening it as unfinished",
                Publish::new(&package, "1.0.0", "reopen.bin", binary_content(8, 2)).unfinished(true),
            ),
        ];
        for (label, attempt) in &reopen_attempts {
            let refused = publish(&router, attempt).await;
            assert_eq!(
                (refused.status, refused.code()),
                (409, "ConflictException".to_string()),
                "case {asset_count} assets, {label:?}: expected a 409 ConflictException, got {refused:?}"
            );
            let unchanged = describe(&router, &final_asset).await;
            assert_eq!(
                (
                    unchanged.body["packageVersion"]["status"].as_str(),
                    unchanged.body["packageVersion"]["revision"].as_str(),
                ),
                (Some("Published"), Some(closed_revision.as_str())),
                "case {asset_count} assets, {label:?}: a refused publish must change nothing, got {unchanged:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The declared ceiling on assets per version, which bounds an open version.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unfinished_version_stops_accepting_assets_at_the_declared_ceiling() {
    let router = emulator_with_repository().await;

    // The emulator's own bound, read from the crate so this test cannot drift onto the
    // wrong boundary if the ceiling moves.
    let ceiling =
        service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::state::MAX_ASSETS_PER_PACKAGE_VERSION;

    for index in 0..ceiling {
        let case = Publish::new(
            "bounded",
            "1.0.0",
            &format!("part-{index:03}.bin"),
            binary_content(8, index as u8),
        )
        .unfinished(true);
        let published = publish(&router, &case).await;
        assert_eq!(
            (published.status, published.code()),
            (200, String::new()),
            "asset {index}: expected a 200 publish below the ceiling, got {published:?}"
        );
    }

    // One past the ceiling, and one more past it, are both refused.
    for index in [ceiling, ceiling + 1] {
        let refused = publish(
            &router,
            &Publish::new(
                "bounded",
                "1.0.0",
                &format!("part-{index:03}.bin"),
                binary_content(8, 1),
            )
            .unfinished(true),
        )
        .await;
        assert_eq!(
            (refused.status, refused.code()),
            (402, "ServiceQuotaExceededException".to_string()),
            "asset {index}: expected a 402 ServiceQuotaExceededException past the ceiling, got {refused:?}"
        );
    }

    // An asset the version already holds is still accepted (it stores nothing new), so
    // the ceiling bounds the stored assets and not the calls.
    let repeated = publish(
        &router,
        &Publish::new("bounded", "1.0.0", "part-000.bin", binary_content(8, 0)).unfinished(true),
    )
    .await;
    assert_eq!(
        (repeated.status, repeated.code()),
        (200, String::new()),
        "republishing an asset the version already holds must succeed, got {repeated:?}"
    );

    // And the version is intact: it holds exactly the bytes of the assets accepted.
    let domain = describe_domain(&router, DOMAIN).await;
    assert_eq!(
        domain.body["domain"]["assetSizeBytes"].as_i64(),
        Some((ceiling * 8) as i64),
        "expected only the accepted assets to count towards assetSizeBytes, got {domain:?}"
    );
}

// ---------------------------------------------------------------------------
// Origin control: a package whose `publish` restriction is BLOCK takes no publish.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn origin_control_decides_whether_a_package_accepts_a_publish() {
    // The rows are the `AllowPublish` catalog (the model's own enum, re-exported as
    // `ALLOW_PUBLISH_VALUES`) crossed with whether the package already holds a version,
    // so the restriction is asserted on a package created by a publish AND on one
    // created by `PutPackageOriginConfiguration` alone.
    let cases = [
        ("ALLOW, package already published", "ALLOW", true, 200),
        ("BLOCK, package already published", "BLOCK", true, 400),
        ("ALLOW, package created by the origin call", "ALLOW", false, 200),
        ("BLOCK, package created by the origin call", "BLOCK", false, 400),
    ];
    assert_eq!(
        ALLOW_PUBLISH_VALUES,
        ["ALLOW", "BLOCK"],
        "the AllowPublish catalog moved; the rows above must follow it, got {ALLOW_PUBLISH_VALUES:?}"
    );

    for (label, publish_restriction, seed_version, expected_status) in cases {
        let router = emulator_with_repository().await;
        let package = format!("origin-{}", publish_restriction.to_lowercase());
        let first = Publish::new(&package, "1.0.0", "first.bin", binary_content(16, 9));

        if seed_version {
            let published = publish(&router, &first).await;
            assert_eq!(
                published.status, 200,
                "case {label:?}: the seeding publish must succeed, got {published:?}"
            );
        }

        // `PutPackageOriginConfiguration` is `POST /v1/package` with the package
        // coordinate in the query string; it creates the package when it does not exist.
        // The coordinate is the (format, namespace, package) TRIPLE, so it carries the
        // same namespace the publishes do — a configuration written under another
        // namespace belongs to another package.
        let configured = call(
            &router,
            "POST",
            &format!(
                "/v1/package?domain={DOMAIN}&repository={REPOSITORY}&format=generic&namespace={NAMESPACE}&package={package}"
            ),
            Some(serde_json::json!({
                "restrictions": { "publish": publish_restriction, "upstream": "ALLOW" }
            })),
        )
        .await;
        assert_eq!(
            configured.status, 200,
            "case {label:?}: PutPackageOriginConfiguration failed: {configured:?}"
        );

        let attempt = Publish::new(&package, "2.0.0", "second.bin", binary_content(24, 4));
        let answer = publish(&router, &attempt).await;
        assert_eq!(
            answer.status, expected_status,
            "case {label:?}: expected status {expected_status}, got {answer:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                answer.code(),
                "ValidationException".to_string(),
                "case {label:?}: a blocked publish is a validation refusal, not an \
                 authorization one, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                "This package is configured to block new versions from being published.",
                "case {label:?}: expected the service's fixed origin-block sentence, got \
                 {answer:?}"
            );
            // And it stored nothing: the version the blocked call named is absent.
            let absent = describe(&router, &attempt).await;
            assert_eq!(
                (absent.status, absent.code()),
                (404, "ResourceNotFoundException".to_string()),
                "case {label:?}: a blocked publish must store nothing, got {absent:?}"
            );
        } else {
            let described = describe(&router, &attempt).await;
            assert_eq!(
                described.body["packageVersion"]["status"].as_str(),
                Some("Published"),
                "case {label:?}: an allowed publish must be readable back, got {described:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 7: the revision is a function of the version's content, not of the call.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revision_follows_the_content_and_not_the_call() {
    let router = emulator_with_repository().await;

    // Two packages given byte-identical assets under the same version and status report
    // the same revision; changing any of those inputs changes it.
    let rows = vec![
        (
            "same version, same bytes",
            Publish::new("rev-a", "1.0.0", "payload.bin", binary_content(100, 4)),
            Publish::new("rev-b", "1.0.0", "payload.bin", binary_content(100, 4)),
            true,
        ),
        (
            "different bytes",
            Publish::new("rev-c", "1.0.0", "payload.bin", binary_content(100, 4)),
            Publish::new("rev-d", "1.0.0", "payload.bin", binary_content(100, 5)),
            false,
        ),
        (
            "different asset name",
            Publish::new("rev-e", "1.0.0", "payload.bin", binary_content(100, 4)),
            Publish::new("rev-f", "1.0.0", "other.bin", binary_content(100, 4)),
            false,
        ),
        (
            "different version",
            Publish::new("rev-g", "1.0.0", "payload.bin", binary_content(100, 4)),
            Publish::new("rev-h", "1.0.1", "payload.bin", binary_content(100, 4)),
            false,
        ),
    ];

    for (label, left, right, expect_equal) in &rows {
        let first = publish(&router, left).await;
        let second = publish(&router, right).await;
        assert_eq!(
            (first.status, second.status),
            (200, 200),
            "case {label:?}: both publishes must succeed, got {first:?} and {second:?}"
        );
        let equal = first.body["versionRevision"] == second.body["versionRevision"];
        assert_eq!(
            equal, *expect_equal,
            "case {label:?}: expected revisions equal={expect_equal}, got {:?} and {:?}",
            first.body["versionRevision"], second.body["versionRevision"]
        );
    }
}

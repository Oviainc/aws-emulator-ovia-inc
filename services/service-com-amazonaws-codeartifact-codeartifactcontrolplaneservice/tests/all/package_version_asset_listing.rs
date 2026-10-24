// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for `ListPackageVersionAssets`.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code and body — not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! example:
//!
//! 1. The listing reports the assets the addressed package version HOLDS: one
//!    `AssetSummary` per stored asset, in asset-name order, each carrying that asset's own
//!    name, the length of its stored bytes and a hash per `HashAlgorithm` computed from
//!    them, beside the version's own `(format, namespace, package, version)` coordinate and
//!    its current `versionRevision`.
//! 2. The listing is paginated over the asset-name order: a page carries at most
//!    `maxResults` assets, a `nextToken` is reported exactly while assets are left, and
//!    following the cursor to exhaustion yields every asset exactly once, in order.
//! 3. A `nextToken` this service did not mint for THIS version's listing is
//!    `ValidationException` (400) and lists nothing.
//! 4. Every unresolvable step of the coordinate is `ResourceNotFoundException` (404)
//!    naming the resource that was missing in `resourceType`/`resourceId` — domain,
//!    repository, package or package-version.
//! 5. Listing is a READ: repeated listings answer identically, and nothing a later caller
//!    sees (the version's description and revision, the domain's `assetSizeBytes`) moves
//!    because a version's assets were listed.
//! 6. An UNFINISHED version lists the assets it has accepted so far, with the revision the
//!    publish reported; the asset that closes the version appears in the same listing, and
//!    a refused publish into the closed version leaves the listing alone.
//! 7. A version whose assets `DisposePackageVersions` deleted holds none, so the listing
//!    succeeds and carries an EMPTY `assets` list rather than an absent one.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The `HashAlgorithm` catalog comes from the model's own generated constants, so a model
// change moves the assertions below rather than leaving a hand-copied list behind.
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::ops::types::HASH_ALGORITHM_VALUES;

// The HTTP harness the crate's suites share, the publish helpers from the suite that owns
// `PublishPackageVersion`, the disposal helper from the suite that owns the asset
// download, and the query encoder from the tagging suite: a `nextToken` is opaque, so it
// is encoded rather than pasted into the query string raw.
use crate::harness::{call, Answer};
use crate::package_version_asset_download::dispose;
use crate::package_version_publish::{
    binary_content, describe, emulator_with_repository, publish, Publish, DOMAIN,
};
use crate::resource_tags::encode_query;

/// One asset of a version under test: the name it is published under and the exact bytes
/// published under it.
type AssetCase<'a> = (&'a str, Vec<u8>);

/// One row of a version table: the case label, the publish that addresses the version,
/// and the assets published into it, in PUBLISH order (which the listing's name order
/// must not depend on).
type VersionCase<'a> = (&'a str, Publish, Vec<AssetCase<'a>>);

/// One `ListPackageVersionAssets` request, addressed by the whole coordinate a package
/// version is identified by.
#[derive(Debug, Clone)]
struct List {
    domain: String,
    repository: String,
    format: String,
    namespace: Option<String>,
    package: String,
    version: String,
    max_results: Option<i32>,
    next_token: Option<String>,
}

impl List {
    /// The listing of the version a [`Publish`] stored into.
    fn of(published: &Publish) -> Self {
        Self {
            domain: published.domain.clone(),
            repository: published.repository.clone(),
            format: published.format.clone(),
            namespace: published.namespace.clone(),
            package: published.package.clone(),
            version: published.version.clone(),
            max_results: None,
            next_token: None,
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

    fn max_results(mut self, max_results: i32) -> Self {
        self.max_results = Some(max_results);
        self
    }

    fn next_token(mut self, next_token: &str) -> Self {
        self.next_token = Some(next_token.to_string());
        self
    }

    fn uri(&self) -> String {
        let mut uri = format!(
            "/v1/package/version/assets?domain={}&repository={}&format={}&package={}&version={}",
            encode_query(&self.domain),
            encode_query(&self.repository),
            encode_query(&self.format),
            encode_query(&self.package),
            encode_query(&self.version),
        );
        if let Some(namespace) = &self.namespace {
            uri.push_str(&format!("&namespace={}", encode_query(namespace)));
        }
        if let Some(max_results) = self.max_results {
            uri.push_str(&format!("&max-results={max_results}"));
        }
        if let Some(next_token) = &self.next_token {
            uri.push_str(&format!("&next-token={}", encode_query(next_token)));
        }
        uri
    }
}

/// Send one `ListPackageVersionAssets`.
async fn list(router: &MultiServiceRouter, request: &List) -> Answer {
    call(router, "POST", &request.uri(), Some(json!({}))).await
}

/// The `assets` entries of an answer, in the order returned.
fn entries(answer: &Answer) -> Vec<Value> {
    answer
        .body
        .get("assets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The asset NAMES of an answer, in the order returned.
fn names(answer: &Answer) -> Vec<String> {
    entries(answer)
        .iter()
        .map(|entry| {
            entry
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `nextToken` of an answer, or `None` on a last page.
fn next_token(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The hash a client verifies with, per `HashAlgorithm` value, computed from `content`.
///
/// Source: `HASH_ALGORITHM_VALUES`, the model's own `HashAlgorithm` enum
/// (`smithy_query(query="shape",shape="HashAlgorithm")`), so this covers every algorithm
/// the model declares — including the three the issue's own example never mentions.
fn expected_hashes(content: &[u8]) -> Value {
    use md5::Digest as _;

    let mut hashes = serde_json::Map::new();
    for algorithm in HASH_ALGORITHM_VALUES {
        let digest = match *algorithm {
            "MD5" => hex::encode(md5::Md5::digest(content)),
            "SHA-1" => hex::encode(sha1::Sha1::digest(content)),
            "SHA-256" => hex::encode(sha2::Sha256::digest(content)),
            "SHA-512" => hex::encode(sha2::Sha512::digest(content)),
            other => panic!("the model declares a HashAlgorithm this suite does not compute: {other}"),
        };
        hashes.insert((*algorithm).to_string(), Value::String(digest));
    }
    Value::Object(hashes)
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

/// Publish every `(asset name, bytes)` of `assets` into ONE version, keeping it open until
/// the last one closes it, and answer the version's revision after the final publish.
async fn publish_version(
    router: &MultiServiceRouter,
    base: &Publish,
    assets: &[AssetCase<'_>],
    label: &str,
) -> String {
    let mut revision = String::new();
    for (index, (name, content)) in assets.iter().enumerate() {
        let last = index + 1 == assets.len();
        let mut case = base.clone();
        case.asset_name = (*name).to_string();
        case.content = content.clone();
        case.unfinished = if last { None } else { Some(true) };
        revision = publish_ok(router, &case, &format!("{label} / {name}")).await;
    }
    revision
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

// ---------------------------------------------------------------------------
// Rule 1: the listing reports the stored assets, with their sizes and hashes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_reports_every_stored_asset_in_name_order() {
    let router = emulator_with_repository().await;

    // Rows differ along what the rule generalises over: how many assets the version
    // holds, the order they were PUBLISHED in against the order they must be LISTED in,
    // the size and byte values of each (one byte to 8 KiB, binary not text), the asset
    // names' punctuation, the namespace and the shape of the version string.
    let cases: Vec<VersionCase> = vec![
        (
            "one asset, one byte",
            Publish::new("solo", "1.0.0", "", Vec::new()),
            vec![("payload.bin", vec![0x00])],
        ),
        (
            "three assets published out of order",
            Publish::new("trio", "2.0.0-rc.1", "", Vec::new()),
            vec![
                ("zeta.bin", binary_content(31, 1)),
                ("alpha.txt", b"first by name, last published".to_vec()),
                ("middle.tar.gz", binary_content(2048, 11)),
            ],
        ),
        (
            "another namespace, numeric version, 8 KiB asset",
            Publish::new("bulky", "20240101", "", Vec::new()).namespace("team-b"),
            vec![
                ("big.bin", binary_content(8192, 251)),
                ("CHANGELOG.md", b"# 20240101\n".to_vec()),
            ],
        ),
        (
            "punctuated and case-varied asset names",
            Publish::new("punct", "9", "", Vec::new()).namespace("team-a"),
            vec![
                ("my~asset-1_v2.bin", binary_content(17, 3)),
                ("UPPER.BIN", binary_content(5, 9)),
                ("a.b.c.d", binary_content(1, 7)),
                ("checksums.txt", b"deadbeef  my~asset-1_v2.bin\n".to_vec()),
            ],
        ),
    ];

    for (label, base, assets) in &cases {
        let revision = publish_version(&router, base, assets, label).await;

        let answer = list(&router, &List::of(base)).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {label:?}: expected a 200 listing, got {answer:?}"
        );

        // The coordinate the answer echoes is the STORED one.
        assert_eq!(
            answer.body["format"].as_str(),
            Some(base.format.as_str()),
            "case {label:?}: expected format {:?}, got {answer:?}",
            base.format
        );
        assert_eq!(
            answer.body["namespace"].as_str(),
            base.namespace.as_deref(),
            "case {label:?}: expected namespace {:?}, got {answer:?}",
            base.namespace
        );
        assert_eq!(
            answer.body["package"].as_str(),
            Some(base.package.as_str()),
            "case {label:?}: expected package {:?}, got {answer:?}",
            base.package
        );
        assert_eq!(
            answer.body["version"].as_str(),
            Some(base.version.as_str()),
            "case {label:?}: expected version {:?}, got {answer:?}",
            base.version
        );
        assert_eq!(
            answer.body["versionRevision"].as_str(),
            Some(revision.as_str()),
            "case {label:?}: expected the revision the final publish reported, got {answer:?}"
        );
        assert_eq!(
            next_token(&answer),
            None,
            "case {label:?}: a listing that returned every asset reports no nextToken, got \
             {answer:?}"
        );

        // Every asset the version holds, in asset-name order.
        let mut expected_order: Vec<&str> = assets.iter().map(|(name, _)| *name).collect();
        expected_order.sort_unstable();
        assert_eq!(
            names(&answer),
            expected_order,
            "case {label:?}: expected the stored assets in name order, got {answer:?}"
        );

        // Each entry carries ITS OWN size and hashes, computed from the bytes that were
        // published under that name.
        for (name, content) in assets {
            let entry = entries(&answer)
                .into_iter()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(*name))
                .unwrap_or_else(|| {
                    panic!("case {label:?}: asset {name:?} must be listed, got {answer:?}")
                });
            assert_eq!(
                entry["size"].as_i64(),
                Some(content.len() as i64),
                "case {label:?} asset {name:?}: expected size {}, got {entry:?}",
                content.len()
            );
            assert_eq!(
                entry["hashes"],
                expected_hashes(content),
                "case {label:?} asset {name:?}: expected a hash per HashAlgorithm over the \
                 stored bytes, got {entry:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the listing pages over the asset-name order.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pagination_walks_every_asset_exactly_once() {
    let router = emulator_with_repository().await;

    // Seven assets in one version, named so publish order and name order differ.
    let assets: Vec<(&str, Vec<u8>)> = vec![
        ("g.bin", binary_content(7, 1)),
        ("c.bin", binary_content(6, 2)),
        ("a.bin", binary_content(5, 3)),
        ("f.bin", binary_content(4, 4)),
        ("b.bin", binary_content(3, 5)),
        ("e.bin", binary_content(2, 6)),
        ("d.bin", binary_content(1, 7)),
    ];
    let base = Publish::new("paged", "1.0.0", "", Vec::new());
    publish_version(&router, &base, &assets, "paged 1.0.0").await;

    let mut expected: Vec<String> = assets
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    expected.sort();

    // Rows differ along the page size: one at a time, a size that does not divide the
    // total, the exact total, one past it, and the modelled maximum.
    for max_results in [1_i32, 2, 3, 7, 8, 1000] {
        let label = format!("maxResults={max_results}");
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = List::of(&base).max_results(max_results);
            if let Some(token) = &token {
                request = request.next_token(token);
            }
            let answer = list(&router, &request).await;
            assert_eq!(
                (answer.status, answer.code()),
                (200, String::new()),
                "case {label:?} page {pages}: expected a 200 listing, got {answer:?}"
            );

            let page = names(&answer);
            let remaining = expected.len() - collected.len();
            let expected_page = remaining.min(max_results as usize);
            assert_eq!(
                page.len(),
                expected_page,
                "case {label:?} page {pages}: expected {expected_page} assets, got {page:?}"
            );
            collected.extend(page);

            token = next_token(&answer);
            assert_eq!(
                token.is_some(),
                collected.len() < expected.len(),
                "case {label:?} page {pages}: a nextToken is reported exactly while assets are \
                 left ({} of {} collected), got {answer:?}",
                collected.len(),
                expected.len()
            );

            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {label:?}: paging did not terminate after {pages} pages, collected \
                 {collected:?}"
            );
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            collected, expected,
            "case {label:?}: paging must yield every asset exactly once, in name order"
        );
        let unique: BTreeSet<&String> = collected.iter().collect();
        assert_eq!(
            unique.len(),
            collected.len(),
            "case {label:?}: no asset may appear on two pages, got {collected:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a token this listing did not mint is refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn foreign_pagination_token_is_refused() {
    let router = emulator_with_repository().await;

    let first = Publish::new("tokens", "1.0.0", "", Vec::new());
    let first_assets: Vec<(&str, Vec<u8>)> = vec![
        ("a.bin", binary_content(8, 1)),
        ("b.bin", binary_content(8, 2)),
    ];
    publish_version(&router, &first, &first_assets, "tokens 1.0.0").await;

    let second = Publish::new("tokens", "2.0.0", "", Vec::new());
    let second_assets: Vec<(&str, Vec<u8>)> = vec![
        ("a.bin", binary_content(8, 3)),
        ("b.bin", binary_content(8, 4)),
    ];
    publish_version(&router, &second, &second_assets, "tokens 2.0.0").await;

    // A cursor minted by ANOTHER version's asset listing, and one minted by the
    // repository's package listing: both are tokens this service issued, for a different
    // listing, so neither names a place in this one.
    let other_version_token = next_token(&list(&router, &List::of(&second).max_results(1)).await)
        .expect("a two-asset version paged at one must report a nextToken");
    let package_listing = call(
        &router,
        "POST",
        &format!(
            "/v1/packages?domain={}&repository={}&max-results=1",
            first.domain, first.repository
        ),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        package_listing.status, 200,
        "ListPackages must succeed, got {package_listing:?}"
    );

    let mut cases: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-token".to_string()),
        ("hex of foreign text", hex::encode("ca1:assets:a.bin")),
        ("another version's cursor", other_version_token),
        ("an asset name sent raw", "a.bin".to_string()),
    ];
    if let Some(token) = next_token(&package_listing) {
        cases.push(("a package listing's cursor", token));
    }

    for (label, token) in &cases {
        let answer = list(&router, &List::of(&first).next_token(token)).await;
        assert_eq!(
            (answer.status, answer.code()),
            (400, "ValidationException".to_string()),
            "case {label:?}: a nextToken this listing did not mint must be a 400, got {answer:?}"
        );
        assert_eq!(
            answer.body["reason"].as_str(),
            Some("FIELD_VALIDATION_FAILED"),
            "case {label:?}: the refusal must report the field-validation reason, got {answer:?}"
        );
        assert!(
            entries(&answer).is_empty(),
            "case {label:?}: a refusal lists nothing, got {answer:?}"
        );
    }

    // The listing's OWN cursor still resolves, so the rows above did not pass by refusing
    // every token.
    let own = next_token(&list(&router, &List::of(&first).max_results(1)).await)
        .expect("a two-asset version paged at one must report a nextToken");
    let answer = list(&router, &List::of(&first).next_token(&own)).await;
    assert_eq!(
        (answer.status, answer.code()),
        (200, String::new()),
        "this listing's own cursor must resolve, got {answer:?}"
    );
    assert_eq!(
        names(&answer),
        vec!["b.bin".to_string()],
        "this listing's own cursor must resume after the first asset, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: every unresolvable step names the resource that is missing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unresolvable_coordinate_names_the_missing_resource() {
    let router = emulator_with_repository().await;

    let stored = Publish::new("walked", "1.2.3", "payload.bin", binary_content(64, 2));
    publish_ok(&router, &stored, "walked 1.2.3").await;

    let base = List::of(&stored);
    // Each row changes ONE component of the coordinate to something the store does not
    // hold, and names the `resourceType`/`resourceId` the refusal must report.
    let cases: Vec<(&str, List, &str, String)> = vec![
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
            "team-core/walked/9.9.9".to_string(),
        ),
    ];

    for (label, request, resource_type, resource_id) in &cases {
        let answer = list(&router, request).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected a 404 ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            (
                answer.body["resourceType"].as_str(),
                answer.body["resourceId"].as_str()
            ),
            (Some(*resource_type), Some(resource_id.as_str())),
            "case {label:?}: expected the refusal to name {resource_type} {resource_id}, got \
             {answer:?}"
        );
        assert!(
            entries(&answer).is_empty(),
            "case {label:?}: a refusal lists nothing, got {answer:?}"
        );
    }

    // The coordinate that IS held still lists, so none of the rows above passed by
    // breaking the baseline.
    let answer = list(&router, &base).await;
    assert_eq!(
        (answer.status, answer.code(), names(&answer)),
        (200, String::new(), vec![stored.asset_name.clone()]),
        "the stored coordinate must still list its asset, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: listing is a read.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let router = emulator_with_repository().await;

    let cases: Vec<VersionCase> = vec![
        (
            "one asset",
            Publish::new("ro-list", "1.0.0", "", Vec::new()),
            vec![("a.bin", vec![0x7f])],
        ),
        (
            "three assets",
            Publish::new("ro-list", "1.0.1", "", Vec::new()),
            vec![
                ("a.bin", binary_content(128, 13)),
                ("b.bin", binary_content(256, 17)),
                ("c.bin", binary_content(512, 19)),
            ],
        ),
    ];
    for (label, base, assets) in &cases {
        publish_version(&router, base, assets, label).await;
    }

    let before_domain = describe_domain(&router, DOMAIN).await;

    for (label, base, _) in &cases {
        let described_before = describe(&router, base).await;
        // Read the same listing three times, once paged, so a read that consumed a cursor
        // or moved a status would show up here.
        let first = list(&router, &List::of(base)).await;
        let second = list(&router, &List::of(base)).await;
        let paged = list(&router, &List::of(base).max_results(1)).await;
        let described_after = describe(&router, base).await;

        assert_eq!(
            first.body, second.body,
            "case {label:?}: two listings must answer identically, got {first:?} then {second:?}"
        );
        assert_eq!(
            paged.body["versionRevision"], first.body["versionRevision"],
            "case {label:?}: a paged listing reports the same revision, got {paged:?}"
        );
        assert_eq!(
            described_before.body, described_after.body,
            "case {label:?}: DescribePackageVersion must report the same record after three \
             listings"
        );
    }

    let after_domain = describe_domain(&router, DOMAIN).await;
    assert_eq!(
        before_domain["domain"]["assetSizeBytes"], after_domain["domain"]["assetSizeBytes"],
        "a listing must not move the domain's assetSizeBytes: {before_domain:?} then \
         {after_domain:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: an unfinished version lists what it has accepted so far.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unfinished_version_lists_the_assets_accepted_so_far() {
    let router = emulator_with_repository().await;

    // Rows differ along how many assets keep the version open before the final one closes
    // it, and along the version string.
    let cases: Vec<VersionCase> = vec![
        (
            "one open asset then the closing one",
            Publish::new("staged", "1.0.0", "", Vec::new()),
            vec![
                ("part-1.bin", binary_content(64, 1)),
                ("manifest.txt", b"part-1.bin\n".to_vec()),
            ],
        ),
        (
            "three open assets then the closing one",
            Publish::new("staged", "2.0.0-beta", "", Vec::new()),
            vec![
                ("part-1.bin", binary_content(32, 2)),
                ("part-2.bin", binary_content(32, 3)),
                ("part-3.bin", binary_content(32, 4)),
                ("manifest.txt", b"part-1.bin part-2.bin part-3.bin\n".to_vec()),
            ],
        ),
    ];

    for (label, base, assets) in &cases {
        let mut published_so_far: Vec<String> = Vec::new();
        for (index, (name, content)) in assets.iter().enumerate() {
            let last = index + 1 == assets.len();
            let mut case = base.clone();
            case.asset_name = (*name).to_string();
            case.content = content.clone();
            case.unfinished = if last { None } else { Some(true) };
            let revision = publish_ok(&router, &case, &format!("{label} / {name}")).await;
            published_so_far.push((*name).to_string());
            published_so_far.sort();

            // The listing reports exactly the assets accepted so far — before the version
            // closes as well as after — and the revision the publish just reported.
            let answer = list(&router, &List::of(base)).await;
            assert_eq!(
                (answer.status, answer.code()),
                (200, String::new()),
                "case {label:?} after {name:?}: expected a 200 listing, got {answer:?}"
            );
            assert_eq!(
                names(&answer),
                published_so_far,
                "case {label:?} after {name:?}: expected the assets accepted so far, got \
                 {answer:?}"
            );
            assert_eq!(
                answer.body["versionRevision"].as_str(),
                Some(revision.as_str()),
                "case {label:?} after {name:?}: expected the revision the publish reported, got \
                 {answer:?}"
            );

            // The status the version is in while it is open, and once it closes, is the
            // version's own record — asserted through its reader, so the listing above is
            // known to have been taken on an unfinished version in every row but the last.
            let described = describe(&router, base).await;
            let expected_status = if last { "Published" } else { "Unfinished" };
            assert_eq!(
                described.body["packageVersion"]["status"].as_str(),
                Some(expected_status),
                "case {label:?} after {name:?}: expected status {expected_status}, got \
                 {described:?}"
            );
        }

        // A closed version cannot be reopened, and the refused publish leaves the listing
        // exactly as it was.
        let mut reopen = base.clone();
        reopen.asset_name = "late.bin".to_string();
        reopen.content = binary_content(16, 9);
        reopen.unfinished = Some(true);
        let refused = publish(&router, &reopen).await;
        assert_eq!(
            (refused.status, refused.code()),
            (409, "ConflictException".to_string()),
            "case {label:?}: a publish into a closed version must be a 409, got {refused:?}"
        );
        let answer = list(&router, &List::of(base)).await;
        assert_eq!(
            names(&answer),
            published_so_far,
            "case {label:?}: a refused publish must not add an asset to the listing, got \
             {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 7: a disposed version holds no asset, and lists an empty list.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disposed_version_lists_no_assets() {
    let router = emulator_with_repository().await;

    let cases: Vec<VersionCase> = vec![
        (
            "one asset",
            Publish::new("emptied", "1.0.0", "", Vec::new()),
            vec![("notes.txt", b"bye".to_vec())],
        ),
        (
            "two assets",
            Publish::new("emptied", "2.0.0", "", Vec::new()),
            vec![
                ("blob.bin", binary_content(512, 23)),
                ("notes.txt", b"bye again".to_vec()),
            ],
        ),
    ];

    for (label, base, assets) in &cases {
        publish_version(&router, base, assets, label).await;

        let before = list(&router, &List::of(base)).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: the published assets must list before disposal, got {before:?}"
        );
        assert_eq!(
            names(&before).len(),
            assets.len(),
            "case {label:?}: expected {} assets before disposal, got {before:?}",
            assets.len()
        );

        let mut disposed = base.clone();
        disposed.asset_name = assets[0].0.to_string();
        dispose(&router, &disposed).await;

        let after = list(&router, &List::of(base)).await;
        assert_eq!(
            (after.status, after.code()),
            (200, String::new()),
            "case {label:?}: a disposed version is still listable, got {after:?}"
        );
        assert_eq!(
            after.body["assets"],
            json!([]),
            "case {label:?}: a disposed version's assets are deleted, so the list is present \
             and EMPTY, got {after:?}"
        );
        assert_eq!(
            after.body["version"].as_str(),
            Some(base.version.as_str()),
            "case {label:?}: the answer still reports the version it was asked about, got \
             {after:?}"
        );
    }
}


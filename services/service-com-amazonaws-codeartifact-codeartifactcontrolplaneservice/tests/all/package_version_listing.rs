// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for `ListPackageVersions`.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code and body — not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! example:
//!
//! 1. The listing reports the versions the addressed package HOLDS: one
//!    `PackageVersionSummary` per stored version, each carrying that version's own string,
//!    its status, the revision `DescribePackageVersion` reports for it and its origin,
//!    beside the package's own `(format, namespace, package)` coordinate and the
//!    `defaultDisplayVersion` — the most recently published version of the package.
//! 2. `status` and `originType` each filter EXACTLY, and neither is defaulted: with no
//!    `status` the listing carries versions of every status the package holds. A `status`
//!    naming a state no stored version carries is refused, not answered as an empty page
//!    (`package_version_input_status` owns that rule).
//! 3. The listing has a stable order the cursor names a place in: the stored version-string
//!    order by default, and most recently published FIRST under
//!    `sortBy=PUBLISHED_TIME` — a `defaultDisplayVersion` and a page of the same package
//!    therefore agree on which version is the newest.
//! 4. The listing is paginated over that order: a page carries at most `maxResults`
//!    versions, a `nextToken` is reported exactly while versions are left, and following
//!    the cursor to exhaustion yields every version exactly once, in order.
//! 5. A `nextToken` this service did not mint for THIS package's listing, in THIS order, is
//!    `ValidationException` (400) and lists nothing.
//! 6. Every unresolvable step of the coordinate is `ResourceNotFoundException` (404) naming
//!    the resource that was missing in `resourceType`/`resourceId` — domain, repository or
//!    package.
//! 7. Listing is a READ: repeated listings answer identically, and nothing a later caller
//!    sees (the version's description, the domain's `assetSizeBytes`) moves because a
//!    package's versions were listed.
//! 8. A package that holds no versions — one `PutPackageOriginConfiguration` created, or one
//!    whose last version `DeletePackageVersions` removed — lists an EMPTY `versions` array
//!    rather than being refused, reports no `defaultDisplayVersion`, and the version string
//!    the delete freed can be published again and is then listed once, with the revision of
//!    the NEW content.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The status and origin-type catalogs come from the model's own generated constants, so a
// model change moves the tables below rather than leaving a hand-copied list behind.
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::ops::types::{
    PACKAGE_VERSION_ORIGIN_TYPE_VALUES, PACKAGE_VERSION_STATUS_VALUES,
};

// The HTTP harness the crate's suites share, the publish helpers from the suite that owns
// `PublishPackageVersion`, the disposal helper from the suite that owns the asset download,
// and the status/origin-configuration helpers from the suite that owns the updates.
use crate::harness::{call, Answer};
use crate::package_version_asset_download::dispose;
use crate::package_version_publish::{
    binary_content, describe, emulator_with_repository, publish, Publish, DOMAIN, REPOSITORY,
};
use crate::update_operations_persist::{put_origin_configuration, update_versions_status};

/// The `PackageVersionStatus` a publish stores, and the `PackageVersionOriginType` of a
/// version published straight into the repository. Spelled here so each row names the value
/// it expects rather than reaching for a handler constant.
const PUBLISHED: &str = "Published";
const UNFINISHED: &str = "Unfinished";
const ARCHIVED: &str = "Archived";
const UNLISTED: &str = "Unlisted";
const DISPOSED: &str = "Disposed";
const DELETED: &str = "Deleted";
const INTERNAL: &str = "INTERNAL";

/// The `PackageVersionSortType` value that orders the listing by publish time.
const PUBLISHED_TIME: &str = "PUBLISHED_TIME";

/// One `ListPackageVersions` request, addressed by the package coordinate and carrying the
/// filters, the ordering and the paging the model binds to the query string.
#[derive(Debug, Clone)]
struct List {
    domain: String,
    repository: String,
    format: String,
    namespace: Option<String>,
    package: String,
    status: Option<String>,
    origin_type: Option<String>,
    sort_by: Option<String>,
    max_results: Option<i32>,
    next_token: Option<String>,
}

impl List {
    /// The listing of the package a [`Publish`] stored into.
    fn of(published: &Publish) -> Self {
        Self {
            domain: published.domain.clone(),
            repository: published.repository.clone(),
            format: published.format.clone(),
            namespace: published.namespace.clone(),
            package: published.package.clone(),
            status: None,
            origin_type: None,
            sort_by: None,
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

    fn package(mut self, package: &str) -> Self {
        self.package = package.to_string();
        self
    }

    fn status(mut self, status: &str) -> Self {
        self.status = Some(status.to_string());
        self
    }

    fn origin_type(mut self, origin_type: &str) -> Self {
        self.origin_type = Some(origin_type.to_string());
        self
    }

    fn sorted_by(mut self, sort_by: Option<&str>) -> Self {
        self.sort_by = sort_by.map(str::to_string);
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
            "/v1/package/versions?domain={}&repository={}&format={}&package={}",
            self.domain, self.repository, self.format, self.package
        );
        if let Some(namespace) = &self.namespace {
            uri.push_str(&format!("&namespace={namespace}"));
        }
        if let Some(status) = &self.status {
            uri.push_str(&format!("&status={status}"));
        }
        if let Some(origin_type) = &self.origin_type {
            uri.push_str(&format!("&originType={origin_type}"));
        }
        if let Some(sort_by) = &self.sort_by {
            uri.push_str(&format!("&sortBy={sort_by}"));
        }
        if let Some(max_results) = self.max_results {
            uri.push_str(&format!("&max-results={max_results}"));
        }
        if let Some(next_token) = &self.next_token {
            uri.push_str(&format!("&next-token={next_token}"));
        }
        uri
    }
}

/// `ListPackageVersions`: every member is `httpQuery`-bound, so the body carries nothing.
async fn list(router: &MultiServiceRouter, request: &List) -> Answer {
    call(router, "POST", &request.uri(), Some(json!({}))).await
}

/// The `versions` array of a successful answer, as the wire reports it.
fn summaries(answer: &Answer) -> Vec<Value> {
    answer
        .body
        .get("versions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Just the version strings of a successful answer, in the order they were returned.
fn listed(answer: &Answer) -> Vec<String> {
    summaries(answer)
        .iter()
        .map(|summary| {
            summary["version"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `defaultDisplayVersion` of an answer, or `None` when the answer carries none.
fn display_version(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("defaultDisplayVersion")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `DeletePackageVersions` for one version of the package a [`Publish`] addresses.
async fn delete_version(router: &MultiServiceRouter, published: &Publish) -> Answer {
    let mut uri = format!(
        "/v1/package/versions/delete?domain={}&repository={}&format={}&package={}",
        published.domain, published.repository, published.format, published.package
    );
    if let Some(namespace) = &published.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    call(
        router,
        "POST",
        &uri,
        Some(json!({ "versions": [published.version] })),
    )
    .await
}

/// Publish every version of `package` in the order given, each with its own content, and
/// hand back the [`Publish`] of each so a row can address it afterwards.
async fn publish_versions(
    router: &MultiServiceRouter,
    package: &str,
    namespace: &str,
    versions: &[&str],
) -> Vec<Publish> {
    let mut published = Vec::new();
    for (seed, version) in versions.iter().enumerate() {
        let request = Publish::new(
            package,
            version,
            "payload.bin",
            binary_content(32 + seed, seed as u8),
        )
        .namespace(namespace);
        let answer = publish(router, &request).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "publishing {package} {version} must succeed, got {answer:?}"
        );
        published.push(request);
    }
    published
}

/// Follow `nextToken` to exhaustion, asserting rule 4 on every page: each page is at most
/// `max_results` long, a token is present exactly while versions are left, and the pages
/// concatenate to the whole listing.
async fn walk_pages(
    router: &MultiServiceRouter,
    request: &List,
    max_results: i32,
    case: &str,
) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..32 {
        let mut paged = request.clone().max_results(max_results);
        if let Some(cursor) = &token {
            paged = paged.next_token(cursor);
        }
        let answer = list(router, &paged).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {case:?}: page {page} must answer 200, got {answer:?}"
        );
        let names = listed(&answer);
        let limit = usize::try_from(max_results).expect("a page size is positive");
        assert!(
            names.len() <= limit,
            "case {case:?}: page {page} carries {} versions, more than maxResults {max_results}: {answer:?}",
            names.len()
        );
        collected.extend(names);
        token = answer
            .body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: the listing never ran out of pages, collected {collected:?}");
}

// ---------------------------------------------------------------------------
// Rule 1: the listing projects the stored versions.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_reports_every_stored_version_of_the_package() {
    let router = emulator_with_repository().await;

    // Rows differ along what the rule generalises over: how many versions the package holds
    // (1, 2, 4), the shape of the version strings (semver, a pre-release, a date, a
    // two-digit major whose text order is not its numeric order) and the namespace.
    let cases: Vec<(&str, &str, &str, Vec<&str>)> = vec![
        ("one version", "solo", "team-a", vec!["1.0.0"]),
        (
            "two versions, one a pre-release",
            "duo",
            "team-a",
            vec!["1.0.0", "1.1.0-rc.1"],
        ),
        (
            "date-shaped versions under another namespace",
            "dated",
            "team-b",
            vec!["20240101", "20240202"],
        ),
        (
            "four versions whose text order is not their numeric order",
            "many",
            "team-c",
            vec!["1.0.0", "2.0.0", "9.0.0", "10.0.0"],
        ),
    ];

    for (case, package, namespace, versions) in &cases {
        let published = publish_versions(&router, package, namespace, versions).await;
        let answer = list(&router, &List::of(&published[0])).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {case:?}: ListPackageVersions must answer 200, got {answer:?}"
        );

        // The package's own coordinate, reported once beside the page.
        assert_eq!(
            answer.body["format"].as_str(),
            Some("generic"),
            "case {case:?}: expected the stored format, got {answer:?}"
        );
        assert_eq!(
            answer.body["namespace"].as_str(),
            Some(*namespace),
            "case {case:?}: expected namespace {namespace:?}, got {answer:?}"
        );
        assert_eq!(
            answer.body["package"].as_str(),
            Some(*package),
            "case {case:?}: expected package {package:?}, got {answer:?}"
        );

        // Every stored version, in the stored version-string order.
        let mut expected: Vec<String> = versions.iter().map(|v| (*v).to_string()).collect();
        expected.sort();
        assert_eq!(
            listed(&answer),
            expected,
            "case {case:?}: expected the stored versions in version order, got {answer:?}"
        );

        // The most recently published version is the one to display.
        assert_eq!(
            display_version(&answer).as_deref(),
            versions.last().copied(),
            "case {case:?}: expected the most recently published version as \
             defaultDisplayVersion, got {answer:?}"
        );

        // Each summary carries that version's own status, revision and origin — the same
        // revision `DescribePackageVersion` reports, so a listing cannot disagree with a
        // read of the same version.
        for request in &published {
            let described = describe(&router, request).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: DescribePackageVersion {} must succeed, got {described:?}",
                request.version
            );
            let summary = summaries(&answer)
                .into_iter()
                .find(|summary| summary["version"].as_str() == Some(request.version.as_str()))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: {} must be listed, got {answer:?}",
                        request.version
                    )
                });
            assert_eq!(
                summary["status"].as_str(),
                Some(PUBLISHED),
                "case {case:?}: {} must be listed as {PUBLISHED}, got {summary:?}",
                request.version
            );
            assert_eq!(
                summary["revision"].as_str(),
                described.body["packageVersion"]["revision"].as_str(),
                "case {case:?}: {} must be listed with the revision the description \
                 reports, got {summary:?}",
                request.version
            );
            assert_eq!(
                summary["origin"]["originType"].as_str(),
                Some(INTERNAL),
                "case {case:?}: {} was published into this repository, got {summary:?}",
                request.version
            );
            assert_eq!(
                summary["origin"]["domainEntryPoint"]["repositoryName"].as_str(),
                Some(REPOSITORY),
                "case {case:?}: {} must name the repository it entered through, got \
                 {summary:?}",
                request.version
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the filters.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_and_origin_type_filter_exactly() {
    let router = emulator_with_repository().await;
    let package = "filtered";
    let namespace = "team-filter";

    // One version per status a stored version can carry, each moved there by the operation
    // that owns that status: a publish stores `Published` (and `Unfinished` while it is
    // still accepting assets), `UpdatePackageVersionsStatus` moves one to `Archived` and
    // `Unlisted`, and `DisposePackageVersions` moves one to `Disposed`. No stored version is
    // ever `Deleted` — a deleted version is gone — so that status is not filterable at all.
    let published = publish_versions(
        &router,
        package,
        namespace,
        &["1.0.0", "2.0.0", "3.0.0", "4.0.0"],
    )
    .await;
    let unfinished = Publish::new(package, "5.0.0", "part.bin", binary_content(8, 5))
        .namespace(namespace)
        .unfinished(true);
    let answer = publish(&router, &unfinished).await;
    assert_eq!(answer.status, 200, "the unfinished publish failed: {answer:?}");

    for (version, target) in [("2.0.0", ARCHIVED), ("3.0.0", UNLISTED)] {
        let moved = update_versions_status(&router, &published[0], &[version], target, None, None)
            .await;
        assert_eq!(
            moved.status, 200,
            "moving {version} to {target} failed: {moved:?}"
        );
    }
    dispose(&router, &published[3]).await;

    // The whole modelled status catalog, each value paired with the versions that carry it.
    for status in PACKAGE_VERSION_STATUS_VALUES {
        let answer = list(&router, &List::of(&published[0]).status(status)).await;
        let expected: Vec<&str> = match *status {
            PUBLISHED => vec!["1.0.0"],
            ARCHIVED => vec!["2.0.0"],
            UNLISTED => vec!["3.0.0"],
            DISPOSED => vec!["4.0.0"],
            UNFINISHED => vec!["5.0.0"],
            // `Deleted` is the one status no stored version carries, so it is not a status a
            // request may filter on at all: the listing REFUSES it rather than answering an
            // empty page. `package_version_input_status` owns that rule's own table.
            DELETED => {
                assert_eq!(
                    (answer.status, answer.code()),
                    (400, "ValidationException".to_string()),
                    "case {status:?}: a status no stored version carries must be refused, \
                     got {answer:?}"
                );
                continue;
            }
            other => panic!("the model grew a status this table does not cover: {other}"),
        };
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {status:?}: the filtered listing must answer 200, got {answer:?}"
        );
        assert_eq!(
            listed(&answer),
            expected,
            "case {status:?}: expected {expected:?}, got {answer:?}"
        );
        // A filter narrows the PAGE, not the package: the display version is the package's.
        assert_eq!(
            display_version(&answer).as_deref(),
            Some("1.0.0"),
            "case {status:?}: the display version is the package's, got {answer:?}"
        );
    }

    // No `status` at all returns versions of every status the package holds.
    let answer = list(&router, &List::of(&published[0])).await;
    assert_eq!(
        listed(&answer),
        vec!["1.0.0", "2.0.0", "3.0.0", "4.0.0", "5.0.0"],
        "an unfiltered listing must carry every status, got {answer:?}"
    );

    // The whole modelled origin-type catalog: every version here was published straight
    // into the repository, so only `INTERNAL` matches any of them.
    for origin_type in PACKAGE_VERSION_ORIGIN_TYPE_VALUES {
        let expected: Vec<&str> = if *origin_type == INTERNAL {
            vec!["1.0.0", "2.0.0", "3.0.0", "4.0.0", "5.0.0"]
        } else {
            vec![]
        };
        let answer = list(&router, &List::of(&published[0]).origin_type(origin_type)).await;
        assert_eq!(
            listed(&answer),
            expected,
            "case {origin_type:?}: expected {expected:?}, got {answer:?}"
        );
    }

    // Both filters are ANDed: a status one version carries and an origin type none does.
    let answer = list(
        &router,
        &List::of(&published[0]).status(ARCHIVED).origin_type("EXTERNAL"),
    )
    .await;
    assert!(
        listed(&answer).is_empty(),
        "case \"Archived and EXTERNAL\": expected no version, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// Rules 3 and 4: the order, and the pages cut from it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sort_by_published_time_lists_the_newest_first() {
    // Rows differ along what the rule generalises over: how many versions the package
    // holds, and how far the PUBLISH order is from the version-string order (reversed,
    // interleaved, and a pair whose text order is not its numeric order).
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("publish order reversed", "rev", vec!["3.0.0", "2.0.0", "1.0.0"]),
        ("publish order interleaved", "mix", vec!["2.0.0", "1.0.0", "3.0.0"]),
        (
            "text order is not numeric order",
            "wide",
            vec!["10.0.0", "9.0.0"],
        ),
    ];

    for (case, package, publish_order) in &cases {
        let router = emulator_with_repository().await;
        let published = publish_versions(&router, package, "team-sort", publish_order).await;

        // The default order is the stored version-string order, whatever order the versions
        // were published in.
        let mut version_order: Vec<String> =
            publish_order.iter().map(|v| (*v).to_string()).collect();
        version_order.sort();
        let answer = list(&router, &List::of(&published[0])).await;
        assert_eq!(
            listed(&answer),
            version_order,
            "case {case:?}: with no sortBy the listing is in version order, got {answer:?}"
        );

        // Under `sortBy=PUBLISHED_TIME` the most recently published version comes first, so
        // the order is the publish order reversed.
        let newest_first: Vec<String> = publish_order
            .iter()
            .rev()
            .map(|v| (*v).to_string())
            .collect();
        let answer = list(
            &router,
            &List::of(&published[0]).sorted_by(Some(PUBLISHED_TIME)),
        )
        .await;
        assert_eq!(
            listed(&answer),
            newest_first,
            "case {case:?}: sortBy={PUBLISHED_TIME} lists the newest first, got {answer:?}"
        );
        // The newest version is the one to display, so the first row and the display
        // version agree.
        assert_eq!(
            display_version(&answer).as_deref(),
            publish_order.last().copied(),
            "case {case:?}: the display version must be the newest, got {answer:?}"
        );
    }
}

#[tokio::test]
async fn pagination_walks_the_order_exactly_once() {
    let router = emulator_with_repository().await;
    let publish_order = ["2.0.0", "1.0.0", "4.0.0", "3.0.0", "5.0.0"];
    let published = publish_versions(&router, "paged", "team-page", &publish_order).await;

    let mut version_order: Vec<String> = publish_order.iter().map(|v| (*v).to_string()).collect();
    version_order.sort();
    let newest_first: Vec<String> = publish_order
        .iter()
        .rev()
        .map(|v| (*v).to_string())
        .collect();

    // Rows differ along the page size — one at a time, an exact divisor, a size that
    // leaves a short last page, exactly the whole listing, and more than it — and along the
    // ordering the pages are cut from.
    for (sort_by, expected) in [(None, &version_order), (Some(PUBLISHED_TIME), &newest_first)] {
        let request = List::of(&published[0]).sorted_by(sort_by);
        for max_results in [1, 2, 4, 5, 1000] {
            let case = format!("sortBy={sort_by:?}, maxResults={max_results}");
            let collected = walk_pages(&router, &request, max_results, &case).await;
            assert_eq!(
                &collected, expected,
                "case {case:?}: paging must yield every version once, in order, got \
                 {collected:?}"
            );
        }
    }

    // The last page of a listing carries no `nextToken`: that is what stops a client.
    let answer = list(&router, &List::of(&published[0]).max_results(5)).await;
    assert!(
        answer
            .body
            .get("nextToken")
            .and_then(Value::as_str)
            .is_none(),
        "a page that exhausted the listing must carry no nextToken, got {answer:?}"
    );
    // A page that left something behind carries one.
    let answer = list(&router, &List::of(&published[0]).max_results(4)).await;
    assert!(
        answer
            .body
            .get("nextToken")
            .and_then(Value::as_str)
            .is_some(),
        "a page that left a version behind must carry a nextToken, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: a token this listing did not mint.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_token_this_listing_did_not_mint_is_refused() {
    let router = emulator_with_repository().await;
    let mine = publish_versions(&router, "cursors", "team-cursor", &["1.0.0", "2.0.0"]).await;
    let other = publish_versions(&router, "elsewhere", "team-cursor", &["1.0.0", "2.0.0"]).await;

    // A real token, minted for ANOTHER package's listing.
    let foreign = list(&router, &List::of(&other[0]).max_results(1)).await;
    let foreign_token = foreign.body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the other package's listing must page: {foreign:?}"))
        .to_string();
    // A real token, minted for THIS package's listing but in the OTHER order: a cursor
    // names a place in the order it was cut from.
    let sorted = list(
        &router,
        &List::of(&mine[0])
            .sorted_by(Some(PUBLISHED_TIME))
            .max_results(1),
    )
    .await;
    let sorted_token = sorted.body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the sorted listing must page: {sorted:?}"))
        .to_string();

    // Rows differ along what makes the token unusable: not a token at all, hex that decodes
    // to arbitrary text, a version string handed back raw, another listing's token, and the
    // same listing's token in the other order.
    let cases: Vec<(&str, String)> = vec![
        ("not a token at all", "not-a-token".to_string()),
        ("hex of arbitrary text", hex::encode(b"ca1:packages/x:1")),
        ("a version string handed back raw", "1.0.0".to_string()),
        ("another package's token", foreign_token),
        ("this listing's token in the other order", sorted_token),
    ];

    for (case, token) in &cases {
        let answer = list(&router, &List::of(&mine[0]).next_token(token)).await;
        assert_eq!(
            (answer.status, answer.code()),
            (400, "ValidationException".to_string()),
            "case {case:?}: expected 400 ValidationException, got {answer:?}"
        );
        assert!(
            summaries(&answer).is_empty(),
            "case {case:?}: a refused listing must list nothing, got {answer:?}"
        );
    }

    // The listing itself still works, so no refusal consumed anything.
    let answer = list(&router, &List::of(&mine[0])).await;
    assert_eq!(
        listed(&answer),
        vec!["1.0.0", "2.0.0"],
        "a refused cursor must leave the listing intact, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: a coordinate this emulator does not hold.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_an_unheld_coordinate_is_not_found() {
    let router = emulator_with_repository().await;
    let published = publish_versions(&router, "held", "team-held", &["1.0.0"]).await;
    let held = List::of(&published[0]);

    // (case, the listing, the resource kind the refusal must name, the id it must report)
    let cases: Vec<(&str, List, &str, String)> = vec![
        (
            "a domain that is not held",
            held.clone().domain("absent-domain"),
            "domain",
            "absent-domain".to_string(),
        ),
        (
            "a repository that is not held",
            held.clone().repository("absent-repo"),
            "repository",
            "absent-repo".to_string(),
        ),
        (
            "a package that is not held",
            held.clone().package("absent-pkg"),
            "package",
            "absent-pkg".to_string(),
        ),
        (
            "the right package name under another namespace",
            held.clone().namespace("team-other"),
            "package",
            "held".to_string(),
        ),
        (
            "the right package name under another format",
            held.clone().format("npm"),
            "package",
            "held".to_string(),
        ),
    ];

    for (case, request, resource_type, resource_id) in &cases {
        let answer = list(&router, request).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: expected 404 ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.body["resourceType"].as_str(),
            Some(*resource_type),
            "case {case:?}: the refusal must name the resource kind, got {answer:?}"
        );
        assert_eq!(
            answer.body["resourceId"].as_str(),
            Some(resource_id.as_str()),
            "case {case:?}: the refusal must name the resource, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 7: listing changes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_is_a_read() {
    let router = emulator_with_repository().await;
    let published = publish_versions(&router, "readonly", "team-read", &["1.0.0", "2.0.0"]).await;

    let described_before = describe(&router, &published[0]).await;
    let domain_before = call(
        &router,
        "GET",
        &format!("/v1/domain?domain={DOMAIN}"),
        None,
    )
    .await;

    // Every shape of listing this suite drives, repeated: a read answers the same thing
    // twice whatever it filtered, ordered or paged.
    let shapes: Vec<(&str, List)> = vec![
        ("unfiltered", List::of(&published[0])),
        (
            "filtered by status",
            List::of(&published[0]).status(PUBLISHED),
        ),
        (
            "ordered by publish time",
            List::of(&published[0]).sorted_by(Some(PUBLISHED_TIME)),
        ),
        ("one version per page", List::of(&published[0]).max_results(1)),
    ];
    for (case, request) in &shapes {
        let first = list(&router, request).await;
        let second = list(&router, request).await;
        assert_eq!(
            first.body, second.body,
            "case {case:?}: two listings of the same package must answer identically, got \
             {first:?} then {second:?}"
        );
    }

    let described_after = describe(&router, &published[0]).await;
    assert_eq!(
        described_before.body, described_after.body,
        "a listing must not move the version's description, got {described_before:?} then \
         {described_after:?}"
    );
    let domain_after = call(
        &router,
        "GET",
        &format!("/v1/domain?domain={DOMAIN}"),
        None,
    )
    .await;
    assert_eq!(
        domain_before.body["domain"]["assetSizeBytes"],
        domain_after.body["domain"]["assetSizeBytes"],
        "a listing must not move the domain's assetSizeBytes, got {domain_before:?} then \
         {domain_after:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 8: a package with no versions, and a version string a delete freed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_package_with_no_versions_lists_an_empty_page() {
    let router = emulator_with_repository().await;

    // A package `PutPackageOriginConfiguration` created holds no versions at all ("When
    // called on a package that does not exist, a package is created in the repository with
    // no versions and the requested restrictions are set on the package"). Rows differ
    // along the format, and therefore along whether the package carries a namespace:
    // `cargo` and `pypi` versions have no namespace component.
    let cases: Vec<(&str, Publish)> = vec![
        (
            "a namespaced generic package",
            Publish::new("configured-generic", "0.0.0", "unused.bin", vec![1]),
        ),
        (
            "a cargo package, which has no namespace",
            Publish {
                format: "cargo".to_string(),
                ..Publish::new("configured-cargo", "0.0.0", "unused.bin", vec![1])
            }
            .without_namespace(),
        ),
    ];

    for (case, request) in &cases {
        let configured = put_origin_configuration(&router, request, "BLOCK", "ALLOW").await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: PutPackageOriginConfiguration failed: {configured:?}"
        );

        let answer = list(&router, &List::of(request)).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {case:?}: a package with no versions must still list, got {answer:?}"
        );
        assert_eq!(
            summaries(&answer),
            Vec::<Value>::new(),
            "case {case:?}: expected an empty versions array, got {answer:?}"
        );
        assert!(
            answer.body.get("versions").is_some(),
            "case {case:?}: the empty list must be PRESENT, not absent, got {answer:?}"
        );
        assert_eq!(
            display_version(&answer),
            None,
            "case {case:?}: a package with no version has none to display, got {answer:?}"
        );
        assert_eq!(
            answer.body["package"].as_str(),
            Some(request.package.as_str()),
            "case {case:?}: the coordinate must still be reported, got {answer:?}"
        );
        assert_eq!(
            answer.body["namespace"].as_str(),
            request.namespace.as_deref(),
            "case {case:?}: the stored namespace must be reported, got {answer:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_version_leaves_the_listing_and_frees_its_version_string() {
    let router = emulator_with_repository().await;

    // Rows differ along the version string the delete frees and along whether the package
    // held anything else: the rule is about the version NUMBER being free again, not about
    // one spelling of it.
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("the only version", "freed-solo", vec!["1.0.0"]),
        (
            "one of two, a pre-release",
            "freed-pair",
            vec!["2.0.0-rc.1", "2.0.0"],
        ),
        ("a date-shaped version", "freed-dated", vec!["20240101"]),
    ];

    for (case, package, versions) in &cases {
        let published = publish_versions(&router, package, "team-free", versions).await;
        let target = &published[0];

        let deleted = delete_version(&router, target).await;
        assert_eq!(
            (deleted.status, deleted.code()),
            (200, String::new()),
            "case {case:?}: DeletePackageVersions must answer 200, got {deleted:?}"
        );

        // The package survives its versions: deleting every version of a package is not
        // deleting the package, so the listing answers with what is LEFT.
        let answer = list(&router, &List::of(target)).await;
        let left: Vec<String> = {
            let mut left: Vec<String> = versions
                .iter()
                .filter(|version| **version != target.version)
                .map(|version| (*version).to_string())
                .collect();
            left.sort();
            left
        };
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {case:?}: the listing must answer 200 after the delete, got {answer:?}"
        );
        assert_eq!(
            listed(&answer),
            left,
            "case {case:?}: expected the versions the delete left, got {answer:?}"
        );
        if left.is_empty() {
            assert_eq!(
                display_version(&answer),
                None,
                "case {case:?}: an emptied package has no version to display, got {answer:?}"
            );
        }

        // The version string the delete freed can be published again, with NEW content: the
        // listing then carries it once, with the revision of what was published second.
        let republished = Publish::new(
            package,
            &target.version,
            "payload.bin",
            binary_content(77, 3),
        )
        .namespace("team-free");
        let answer = publish(&router, &republished).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {case:?}: republishing {} must succeed, got {answer:?}",
            target.version
        );

        let answer = list(&router, &List::of(target)).await;
        let mut expected: Vec<String> = versions.iter().map(|v| (*v).to_string()).collect();
        expected.sort();
        assert_eq!(
            listed(&answer),
            expected,
            "case {case:?}: the freed version must be listed once more, got {answer:?}"
        );
        let described = describe(&router, &republished).await;
        let summary = summaries(&answer)
            .into_iter()
            .find(|summary| summary["version"].as_str() == Some(target.version.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the republished version must be listed"));
        assert_eq!(
            summary["revision"].as_str(),
            described.body["packageVersion"]["revision"].as_str(),
            "case {case:?}: the listing must carry the NEW content's revision, got {summary:?}"
        );
    }
}

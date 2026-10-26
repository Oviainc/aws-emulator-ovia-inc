// SPDX-License-Identifier: CC0-1.0

//! The WAFv2 mobile-SDK surface — `ListMobileSdkReleases`,
//! `GetMobileSdkRelease` and `GenerateMobileSdkReleaseUrl` — end to end over the
//! wire.
//!
//! The rule these suites assert is: "the mobile SDK is not generally available.
//! Customers who have access to the mobile SDK can use it …" (every one of the
//! three operations' own doc), so the three operations share ONE gate — an account
//! that was never granted the mobile SDK is refused by ALL of them with 400
//! `WAFInvalidOperationException`, and in particular is handed neither a download
//! URL nor a release record; while for an account that WAS granted it, the
//! catalogue the listing reports, the release a read answers and the URL a generate
//! answers all agree, each release being addressed by `(Platform,
//! ReleaseVersion)` with `LATEST` naming the platform's newest release.
//!
//! Each suite drives that over a TABLE spanning the class rather than one request:
//! the rows cross both device platforms the model declares with each of the three
//! operations, several release versions (one held, one never published, and the
//! `LATEST` alias), and a platform the granted account holds no release for.
//!
//! No operation of this model publishes a release — a catalogue appears for an
//! account because WAF granted that account the mobile SDK, which is not an API
//! call — so these suites provision one through the state API
//! ([`provision_release`], the same seam `crate::managed_rule_sets` uses for a
//! vendor's managed rule set) and then drive every assertion through the router's
//! front door, the same door an SDK uses.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::ops::types::Tag;
use service_com_amazonaws_wafv2_awswaf_20190729::state::ComAmazonawsWafv2Awswaf20190729State;

use crate::permission_policy::error_message;
use crate::web_acl_lifecycle::Waf;

/// The device platforms the model declares
/// (`smithy_query(query="shape",shape="Platform")` → `IOS`, `ANDROID`). Spelled
/// out here independently of the service's own constants, so these suites pin the
/// catalogue of platforms and not merely its reuse.
const PLATFORMS: &[&str] = &["IOS", "ANDROID"];

/// The `ReleaseVersion` that names a platform's newest release rather than one
/// particular version. Source: `GetMobileSdkReleaseRequest$ReleaseVersion` — "The
/// release version. For the latest available version, specify LATEST."
const LATEST: &str = "LATEST";

/// The sentence the mobile-SDK refusal carries, as recorded from WAF.
const NOT_AUTHORIZED: &str = "This account is not authorized to use Mobile-SDK";

/// `WAFNonexistentItemException`'s sentence, as recorded from WAF.
const NONEXISTENT_ITEM: &str =
    "AWS WAF couldn’t perform the operation because your resource doesn’t exist.";

/// A version key no provisioned release carries, inside `VersionKeyString`'s form
/// (`@length(1..64)`, `@pattern(^[\w#:\.\-/]+$)`).
const MISSING_VERSION: &str = "9.9.9";

/// One release to provision: its platform, version, release instant in
/// milliseconds, notes and tags.
struct Release {
    platform: &'static str,
    version: &'static str,
    timestamp_millis: i64,
    notes: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// The catalogue a granted account holds in these suites.
///
/// The newest IOS release by INSTANT (`1.0.0`) is deliberately not the greatest
/// version key, so an assertion about `LATEST` cannot pass by accident on the
/// listing order; and each platform carries its own releases, so one platform's
/// newest is never the other's.
const CATALOGUE: &[Release] = &[
    Release {
        platform: "IOS",
        version: "0.9.0",
        timestamp_millis: 1_700_000_000_000,
        notes: Some("the first iOS drop"),
        tags: &[("Channel", "beta")],
    },
    Release {
        platform: "IOS",
        version: "2.0.0",
        timestamp_millis: 1_710_000_000_000,
        notes: None,
        tags: &[],
    },
    Release {
        platform: "IOS",
        version: "1.0.0",
        timestamp_millis: 1_760_000_000_000,
        notes: Some("the newest iOS drop"),
        tags: &[("Channel", "ga"), ("Team", "sim2real")],
    },
    Release {
        platform: "ANDROID",
        version: "1.0.0",
        timestamp_millis: 1_720_000_000_000,
        notes: Some("the first Android drop"),
        tags: &[],
    },
    Release {
        platform: "ANDROID",
        version: "1.1.0",
        timestamp_millis: 1_730_000_000_000,
        notes: None,
        tags: &[("Channel", "ga")],
    },
];

/// Grant this account the mobile SDK by publishing one release into its catalogue.
///
/// The one thing these suites cannot do over the wire: no operation in this model
/// publishes a mobile SDK release.
fn provision_release(engine: &engine::InMemoryEngine, release: &Release) {
    engine
        .state::<ComAmazonawsWafv2Awswaf20190729State>()
        .lock()
        .insert_mobile_sdk_release(
            release.platform,
            release.version,
            release.timestamp_millis,
            release.notes.map(str::to_string),
            release
                .tags
                .iter()
                .map(|(key, value)| Tag {
                    key: (*key).to_string(),
                    value: (*value).to_string(),
                })
                .collect(),
        );
}

/// An emulator whose account holds `releases`.
fn granted(releases: &[Release]) -> Waf {
    let (waf, engine): (Waf, Arc<engine::InMemoryEngine>) = Waf::sharing_engine();
    for release in releases {
        provision_release(&engine, release);
    }
    waf
}

/// The releases of `platform` in [`CATALOGUE`], in the version order a listing
/// reports them.
fn catalogue_of(platform: &str) -> Vec<&'static Release> {
    let mut releases: Vec<&Release> = CATALOGUE
        .iter()
        .filter(|release| release.platform == platform)
        .collect();
    releases.sort_by_key(|release| release.version);
    releases
}

/// The release of `platform` with the greatest instant: what `LATEST` names.
fn newest_of(platform: &str) -> &'static Release {
    catalogue_of(platform)
        .into_iter()
        .max_by_key(|release| release.timestamp_millis)
        .expect("every platform in the catalogue holds a release")
}

/// A modelled `Timestamp` as the awsJson wire carries it: epoch SECONDS.
fn as_epoch_seconds(millis: i64) -> f64 {
    millis as f64 / 1_000.0
}

/// The summaries one `ListMobileSdkReleases` page reports.
fn summaries(page: &Value) -> Vec<Value> {
    page["ReleaseSummaries"]
        .as_array()
        .expect("ReleaseSummaries")
        .clone()
}

/// On an account that was never granted the mobile SDK, every operation of the
/// family is refused with the SAME 400 `WAFInvalidOperationException` and hands
/// back neither a release record nor a download URL.
///
/// The rows cross both platforms with every version form a caller can ask for: one
/// the granted catalogue would hold, one it would not, and the `LATEST` alias.
#[tokio::test]
async fn an_account_without_the_mobile_sdk_is_refused_by_every_operation() {
    let waf = Waf::new();
    for platform in PLATFORMS {
        let listed = waf
            .refused(
                "ListMobileSdkReleases",
                json!({ "Platform": platform }),
                "WAFInvalidOperationException",
                &format!("{platform}, ListMobileSdkReleases"),
            )
            .await;
        assert_eq!(
            error_message(&listed),
            NOT_AUTHORIZED,
            "case {platform}: the refusal must carry WAF's own sentence, got {listed}"
        );
        assert!(
            listed.get("ReleaseSummaries").is_none(),
            "case {platform}: a refused listing must report no summaries, got {listed}"
        );

        for version in ["1.0.0", MISSING_VERSION, LATEST] {
            for operation in ["GetMobileSdkRelease", "GenerateMobileSdkReleaseUrl"] {
                let case = format!("{platform}, {operation}, version {version:?}");
                let refused = waf
                    .refused(
                        operation,
                        json!({ "Platform": platform, "ReleaseVersion": version }),
                        "WAFInvalidOperationException",
                        &case,
                    )
                    .await;
                assert_eq!(
                    error_message(&refused),
                    NOT_AUTHORIZED,
                    "case {case:?}: the refusal must carry WAF's own sentence, \
                     got {refused}"
                );
                assert!(
                    refused.get("Url").is_none() && refused.get("MobileSdkRelease").is_none(),
                    "case {case:?}: an unentitled account must be handed neither a URL \
                     nor a release, got {refused}"
                );
            }
        }
    }
}

/// On an account that holds the catalogue, the listing, the read and the generate
/// agree: every summary names a release the read answers with the same version and
/// instant, and the generate answers an https URL naming that release.
#[tokio::test]
async fn a_listed_release_reads_back_and_generates_a_url() {
    let waf = granted(CATALOGUE);
    for platform in PLATFORMS {
        let page = waf
            .ok("ListMobileSdkReleases", json!({ "Platform": platform }))
            .await;
        let listed = summaries(&page);
        let expected = catalogue_of(platform);
        assert_eq!(
            listed.len(),
            expected.len(),
            "case {platform}: the listing must report this platform's releases only, \
             got {page}"
        );

        for (summary, release) in listed.iter().zip(&expected) {
            let case = format!("{platform} {}", release.version);
            assert_eq!(
                summary["ReleaseVersion"],
                json!(release.version),
                "case {case:?}: the summaries must be in version order, got {page}"
            );
            assert_eq!(
                summary["Timestamp"].as_f64(),
                Some(as_epoch_seconds(release.timestamp_millis)),
                "case {case:?}: the summary must carry the release instant, got {page}"
            );

            let got = waf
                .ok(
                    "GetMobileSdkRelease",
                    json!({ "Platform": platform, "ReleaseVersion": release.version }),
                )
                .await;
            let read = &got["MobileSdkRelease"];
            assert_eq!(
                read["ReleaseVersion"], summary["ReleaseVersion"],
                "case {case:?}: the read must agree with the summary, got {got}"
            );
            assert_eq!(
                read["Timestamp"], summary["Timestamp"],
                "case {case:?}: the read's instant must agree with the summary, got {got}"
            );
            assert_eq!(
                read["ReleaseNotes"],
                release.notes.map_or(Value::Null, |notes| json!(notes)),
                "case {case:?}: the read must carry the release's notes, got {got}"
            );
            let read_tags: Vec<(String, String)> = read["Tags"]
                .as_array()
                .map(|tags| {
                    tags.iter()
                        .map(|tag| {
                            (
                                tag["Key"].as_str().unwrap_or_default().to_string(),
                                tag["Value"].as_str().unwrap_or_default().to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let expected_tags: Vec<(String, String)> = release
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                read_tags, expected_tags,
                "case {case:?}: the read must carry the release's tags, got {got}"
            );

            let generated = waf
                .ok(
                    "GenerateMobileSdkReleaseUrl",
                    json!({ "Platform": platform, "ReleaseVersion": release.version }),
                )
                .await;
            let url = generated["Url"].as_str().unwrap_or_default().to_string();
            assert!(
                url.starts_with("https://"),
                "case {case:?}: the download URL must be an https URL, got {generated}"
            );
            assert!(
                url.contains(release.version),
                "case {case:?}: the download URL must name the release it was asked \
                 for, got {generated}"
            );
        }
    }
}

/// `LATEST` names the platform's newest release by INSTANT, for both the read and
/// the generate, and each platform resolves it against its own releases.
#[tokio::test]
async fn latest_names_the_newest_release_of_the_platform() {
    let waf = granted(CATALOGUE);
    for platform in PLATFORMS {
        let newest = newest_of(platform);
        let case = format!("{platform} LATEST");

        let got = waf
            .ok(
                "GetMobileSdkRelease",
                json!({ "Platform": platform, "ReleaseVersion": LATEST }),
            )
            .await;
        assert_eq!(
            got["MobileSdkRelease"]["ReleaseVersion"],
            json!(newest.version),
            "case {case:?}: LATEST must resolve to the newest release, got {got}"
        );
        assert_eq!(
            got["MobileSdkRelease"]["Timestamp"].as_f64(),
            Some(as_epoch_seconds(newest.timestamp_millis)),
            "case {case:?}: LATEST must carry the newest release's instant, got {got}"
        );

        let generated = waf
            .ok(
                "GenerateMobileSdkReleaseUrl",
                json!({ "Platform": platform, "ReleaseVersion": LATEST }),
            )
            .await;
        let url = generated["Url"].as_str().unwrap_or_default().to_string();
        assert!(
            url.contains(newest.version) && !url.contains(LATEST),
            "case {case:?}: the URL must name the version LATEST resolved to, \
             got {generated}"
        );
    }
}

/// On a granted account, a version the catalogue does not publish for the platform
/// — including every version of a platform it holds no release for — is the
/// nonexistent-item failure, NOT the entitlement refusal, and the listing of such
/// a platform is an empty 200.
#[tokio::test]
async fn a_version_the_catalogue_does_not_publish_is_nonexistent() {
    // Granted, but holding iOS releases only: the grant is per ACCOUNT, so the
    // other platform is an empty catalogue rather than a refusal.
    let ios_only: Vec<Release> = CATALOGUE
        .iter()
        .filter(|release| release.platform == "IOS")
        .map(|release| Release {
            platform: release.platform,
            version: release.version,
            timestamp_millis: release.timestamp_millis,
            notes: release.notes,
            tags: release.tags,
        })
        .collect();
    let waf = granted(&ios_only);

    let page = waf
        .ok("ListMobileSdkReleases", json!({ "Platform": "ANDROID" }))
        .await;
    assert!(
        summaries(&page).is_empty(),
        "a platform the account holds no release for must list none, got {page}"
    );
    assert!(
        page["NextMarker"].is_null(),
        "an empty page must carry no marker, got {page}"
    );

    for (case, platform, version) in [
        ("a version iOS does not publish", "IOS", MISSING_VERSION),
        ("a version of a platform with no releases", "ANDROID", "1.0.0"),
        ("LATEST of a platform with no releases", "ANDROID", LATEST),
    ] {
        for operation in ["GetMobileSdkRelease", "GenerateMobileSdkReleaseUrl"] {
            let case = format!("{operation}, {case}");
            let refused = waf
                .refused(
                    operation,
                    json!({ "Platform": platform, "ReleaseVersion": version }),
                    "WAFNonexistentItemException",
                    &case,
                )
                .await;
            assert_eq!(
                error_message(&refused),
                NONEXISTENT_ITEM,
                "case {case:?}: the refusal must carry WAF's own sentence, got {refused}"
            );
        }
    }
}

/// `ListMobileSdkReleases` pages the catalogue: a `Limit` hands back that many
/// summaries and a marker, the marker resumes after the last version of the page,
/// and the pages together are the whole catalogue in the listing's own order.
#[tokio::test]
async fn a_release_listing_pages_the_catalogue() {
    let waf = granted(CATALOGUE);
    for platform in PLATFORMS {
        let whole: Vec<&str> = catalogue_of(platform)
            .into_iter()
            .map(|release| release.version)
            .collect();
        for limit in [1usize, 2, whole.len(), whole.len() + 1] {
            let case = format!("{platform}, Limit {limit}");
            let mut walked: Vec<String> = Vec::new();
            let mut marker: Option<String> = None;
            loop {
                let mut request = json!({ "Platform": platform, "Limit": limit });
                if let Some(marker) = &marker {
                    request["NextMarker"] = json!(marker);
                }
                let page = waf.ok("ListMobileSdkReleases", request).await;
                let listed = summaries(&page);
                assert!(
                    listed.len() <= limit,
                    "case {case:?}: a page must hold at most the limit, got {page}"
                );
                if listed.is_empty() {
                    assert!(
                        page["NextMarker"].is_null(),
                        "case {case:?}: the terminal page must carry no marker, got {page}"
                    );
                    break;
                }
                walked.extend(
                    listed
                        .iter()
                        .map(|summary| {
                            summary["ReleaseVersion"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string()
                        }),
                );
                marker = Some(
                    page["NextMarker"]
                        .as_str()
                        .expect("a non-empty page carries a marker")
                        .to_string(),
                );
            }
            assert_eq!(
                walked, whole,
                "case {case:?}: the pages together must be the whole catalogue, \
                 got {walked:?}"
            );
        }
    }
}

/// A `Platform` the model does not declare is refused by the request validator,
/// whichever operation of the family it is sent to, and a `Limit` outside
/// `PaginationLimit`'s range likewise: both are modelled constraints, so neither
/// reaches the entitlement gate.
#[tokio::test]
async fn a_member_outside_its_modelled_constraint_is_refused_by_the_request_validator() {
    let waf = granted(CATALOGUE);

    for operation in [
        "ListMobileSdkReleases",
        "GetMobileSdkRelease",
        "GenerateMobileSdkReleaseUrl",
    ] {
        let mut request = json!({ "Platform": "WINDOWS" });
        if operation != "ListMobileSdkReleases" {
            request["ReleaseVersion"] = json!("1.0.0");
        }
        let (status, body) = waf.call(operation, request).await;
        assert_eq!(
            status, 400,
            "case {operation}: an unmodelled platform must be refused, got status \
             {status} and body {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case {operation}: an enum failure is the request validator's own refusal, \
             got {body}"
        );
    }

    for limit in [0_i64, -1, 101] {
        let (status, body) = waf
            .call(
                "ListMobileSdkReleases",
                json!({ "Platform": "IOS", "Limit": limit }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case Limit={limit}: a limit outside 1..100 must be refused, got status \
             {status} and body {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case Limit={limit}: a limit outside the modelled range must be refused \
             by the request boundary, got {body}"
        );
    }

    for limit in [1_i64, 100] {
        let page = waf
            .ok(
                "ListMobileSdkReleases",
                json!({ "Platform": "IOS", "Limit": limit }),
            )
            .await;
        assert!(
            !summaries(&page).is_empty(),
            "case Limit={limit}: a limit on the bound must be honoured, got {page}"
        );
    }
}

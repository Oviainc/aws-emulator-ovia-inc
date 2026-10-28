// SPDX-License-Identifier: CC0-1.0

//! `ListMobileSdkReleases` as a served, state-derived listing.
//!
//! The rule: the operation "retrieves a list of the available releases for the
//! mobile SDK and the specified device platform" (its own doc), so every call is
//! answered from the releases the account holds for THAT platform at the instant of
//! the call — never from a fixed answer and never from the other platform's
//! catalogue — and `NextMarker` names the position a later call resumes after.
//!
//! The suite in `crate::mobile_sdk_releases` asserts the family's entitlement gate
//! and the agreement between the listing, the read and the generate on ONE
//! catalogue. This suite pins the listing itself over a TABLE of catalogue shapes —
//! rows differing in how many releases a platform holds and in the form of their
//! version keys — plus the two properties a listing computed from state has and a
//! canned answer does not: a release provisioned after a call shows up in the next
//! one, and listing twice reports the same catalogue twice.
//!
//! No operation of this model publishes a release, so the catalogue is provisioned
//! through the state API (the seam `crate::mobile_sdk_releases` uses), while every
//! assertion is driven through the router's front door.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::ComAmazonawsWafv2Awswaf20190729State;

use crate::web_acl_lifecycle::Waf;

/// The device platforms the model declares
/// (`smithy_query(query="shape",shape="Platform")` → `IOS`, `ANDROID`).
const PLATFORMS: &[&str] = &["IOS", "ANDROID"];

/// The release instant the first provisioned release carries; each later one is a
/// second after its predecessor.
const FIRST_INSTANT_MILLIS: i64 = 1_700_000_000_000;

/// A version key no case provisions, greater than every key below, inside
/// `NextMarker`'s form (`@length(1..256)`, `@pattern(\S)`).
const MARKER_PAST_THE_END: &str = "zzz-no-such-release";

/// One catalogue shape to list: the versions one platform holds, in the order they
/// were provisioned (deliberately not the order a listing reports them).
struct Case {
    /// What this row varies, for the assertion messages.
    name: &'static str,
    /// The `ReleaseVersion` keys to provision, each inside `VersionKeyString`'s
    /// form (`@length(1..64)`, `@pattern(^[\w#:\.\-/]+$)`).
    versions: &'static [&'static str],
}

/// The rows: a platform holding one release, two provisioned backwards, keys whose
/// ordering is not their numeric ordering, and a catalogue long enough to need
/// several pages at every limit this suite walks.
const CASES: &[Case] = &[
    Case {
        name: "one release",
        versions: &["1.0.0"],
    },
    Case {
        name: "two releases provisioned newest first",
        versions: &["2.0.0", "1.0.0"],
    },
    Case {
        name: "version keys whose order is not their numeric order",
        versions: &["1.9.0", "1.10.0", "1.9.0-beta"],
    },
    Case {
        name: "five releases",
        versions: &["0.10.0", "0.1.0", "1.0.0", "0.2.0", "0.3.0"],
    },
];

/// The version key one platform holds in every case, so a listing that answered
/// from the whole account rather than from the platform it was asked for would
/// report it too.
const OTHER_PLATFORM_VERSION: &str = "7.7.7";

/// Provision `versions` for `platform`, each release a second newer than the one
/// before it.
fn provision(engine: &engine::InMemoryEngine, platform: &str, versions: &[&str]) {
    let catalogue = engine.state::<ComAmazonawsWafv2Awswaf20190729State>();
    let mut state = catalogue.lock();
    for (index, version) in versions.iter().enumerate() {
        state.insert_mobile_sdk_release(
            platform,
            version,
            FIRST_INSTANT_MILLIS + index as i64 * 1_000,
            None,
            Vec::new(),
        );
    }
}

/// The platform that is not `platform`.
fn other_platform(platform: &str) -> &'static str {
    PLATFORMS
        .iter()
        .copied()
        .find(|candidate| *candidate != platform)
        .expect("the model declares two platforms")
}

/// An emulator holding `versions` for `platform` and one release for the other
/// platform, plus the shared engine so a case can provision more later.
fn holding(platform: &str, versions: &[&str]) -> (Waf, Arc<engine::InMemoryEngine>) {
    let (waf, engine): (Waf, Arc<engine::InMemoryEngine>) = Waf::sharing_engine();
    provision(&engine, platform, versions);
    provision(&engine, other_platform(platform), &[OTHER_PLATFORM_VERSION]);
    (waf, engine)
}

/// One `ListMobileSdkReleases` page: the call must be SERVED — a 200 carrying no
/// error code, so a scaffold's 501 `NotImplemented` fails here — and its body is
/// answered.
async fn page(waf: &Waf, request: Value, case: &str) -> Value {
    let (status, body) = waf.call("ListMobileSdkReleases", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the listing must be served, got status {status} and body {body}"
    );
    assert!(
        body.get("__type").is_none(),
        "case {case:?}: a served listing carries no error code, got {body}"
    );
    body
}

/// The `(ReleaseVersion, Timestamp)` pairs one page reports, in the order it
/// reports them.
fn summaries(page: &Value, case: &str) -> Vec<(String, Option<f64>)> {
    page["ReleaseSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a served listing reports summaries, got {page}"))
        .iter()
        .map(|summary| {
            (
                summary["ReleaseVersion"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                summary["Timestamp"].as_f64(),
            )
        })
        .collect()
}

/// Just the versions one page reports.
fn versions_of(page: &Value, case: &str) -> Vec<String> {
    summaries(page, case)
        .into_iter()
        .map(|(version, _)| version)
        .collect()
}

/// Every version of `platform` the listing reports, walked page by page with
/// `limit` per page.
async fn walk(waf: &Waf, platform: &str, limit: usize, case: &str) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut request = json!({ "Platform": platform, "Limit": limit });
        if let Some(marker) = &marker {
            request["NextMarker"] = json!(marker);
        }
        let body = page(waf, request, case).await;
        let reported = versions_of(&body, case);
        assert!(
            reported.len() <= limit,
            "case {case:?}: a page must hold at most {limit}, got {body}"
        );
        if reported.is_empty() {
            break;
        }
        walked.extend(reported);
        marker = Some(
            body["NextMarker"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a non-empty page carries a marker, got {body}")
                })
                .to_string(),
        );
    }
    walked
}

/// The instant the release at `index` of a provisioning order carries, as the
/// awsJson wire reports a modelled `Timestamp`: epoch SECONDS.
fn instant_of(index: usize) -> f64 {
    (FIRST_INSTANT_MILLIS + index as i64 * 1_000) as f64 / 1_000.0
}

/// A listing reports exactly the releases the account holds for the platform it
/// names, each with its own release instant, and reports them in one stable order
/// that is independent of the order they were provisioned in.
#[tokio::test]
async fn a_listing_reports_the_platform_s_persisted_releases() {
    for platform in PLATFORMS {
        for Case { name, versions } in CASES {
            let case = format!("{platform}, {name}");
            let (waf, _engine) = holding(platform, versions);

            let body = page(&waf, json!({ "Platform": platform }), &case).await;
            let reported = summaries(&body, &case);

            let mut expected: Vec<(String, Option<f64>)> = versions
                .iter()
                .enumerate()
                .map(|(index, version)| ((*version).to_string(), Some(instant_of(index))))
                .collect();
            expected.sort_by(|left, right| left.0.cmp(&right.0));
            let mut got = reported.clone();
            got.sort_by(|left, right| left.0.cmp(&right.0));
            assert_eq!(
                got, expected,
                "case {case:?}: the listing must report this platform's releases with \
                 their own instants, got {body}"
            );

            let reported_versions: Vec<String> =
                reported.iter().map(|(version, _)| version.clone()).collect();
            assert!(
                !reported_versions.contains(&OTHER_PLATFORM_VERSION.to_string()),
                "case {case:?}: the other platform's release must not be reported, \
                 got {body}"
            );

            // One stable order, whatever the provisioning order was: the same
            // sequence on a second call, and the sequence every paged walk of the
            // same catalogue reproduces.
            let again = page(&waf, json!({ "Platform": platform }), &case).await;
            assert_eq!(
                versions_of(&again, &case),
                reported_versions,
                "case {case:?}: two listings of one catalogue must agree, got {again}"
            );
            for limit in [1usize, 2, versions.len(), versions.len() + 1] {
                let paged_case = format!("{case}, Limit {limit}");
                assert_eq!(
                    walk(&waf, platform, limit, &paged_case).await,
                    reported_versions,
                    "case {paged_case:?}: the pages together must be the whole \
                     catalogue in the listing's order"
                );
            }
        }
    }
}

/// `NextMarker` names a position in the catalogue: a call carrying it reports the
/// releases that follow it, and a marker past the last release reports an empty
/// page with no marker of its own.
#[tokio::test]
async fn a_marker_resumes_after_the_release_it_names() {
    for platform in PLATFORMS {
        for Case { name, versions } in CASES {
            let case = format!("{platform}, {name}");
            let (waf, _engine) = holding(platform, versions);
            let whole = versions_of(&page(&waf, json!({ "Platform": platform }), &case).await, &case);

            for (index, marker) in whole.iter().enumerate() {
                let case = format!("{case}, marker {marker:?}");
                let body = page(
                    &waf,
                    json!({ "Platform": platform, "NextMarker": marker }),
                    &case,
                )
                .await;
                assert_eq!(
                    versions_of(&body, &case),
                    whole[index + 1..],
                    "case {case:?}: the page must hold the releases after the marker, \
                     got {body}"
                );
            }

            let case = format!("{case}, marker past the end");
            let body = page(
                &waf,
                json!({ "Platform": platform, "NextMarker": MARKER_PAST_THE_END }),
                &case,
            )
            .await;
            assert!(
                versions_of(&body, &case).is_empty(),
                "case {case:?}: a marker past the last release must report no \
                 summaries, got {body}"
            );
            assert!(
                body["NextMarker"].is_null(),
                "case {case:?}: an empty page must carry no marker, got {body}"
            );
        }
    }
}

/// The listing is computed when it is called: a release that appears in the
/// catalogue after one call is reported by the next, and listing never changes what
/// a later caller sees.
#[tokio::test]
async fn a_listing_follows_the_catalogue_it_is_asked_about() {
    for platform in PLATFORMS {
        let case = format!("{platform}, a catalogue that grows");
        let (waf, engine) = holding(platform, &["1.0.0"]);

        let before = versions_of(&page(&waf, json!({ "Platform": platform }), &case).await, &case);
        assert_eq!(
            before,
            vec!["1.0.0".to_string()],
            "case {case:?}: the first listing must report the one release held"
        );

        provision(&engine, platform, &["1.1.0"]);
        let after = versions_of(&page(&waf, json!({ "Platform": platform }), &case).await, &case);
        assert_eq!(
            after,
            vec!["1.0.0".to_string(), "1.1.0".to_string()],
            "case {case:?}: a release added after the first call must be reported by \
             the next one, got {after:?}"
        );

        let repeated = versions_of(&page(&waf, json!({ "Platform": platform }), &case).await, &case);
        assert_eq!(
            repeated, after,
            "case {case:?}: listing must not change the catalogue a later caller \
             sees, got {repeated:?}"
        );
    }
}

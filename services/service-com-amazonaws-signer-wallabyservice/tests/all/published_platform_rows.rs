// SPDX-License-Identifier: CC0-1.0

//! Every member of a signing-platform row is PUBLISHED DATA, served verbatim, end
//! to end over HTTP.
//!
//! A signing platform is published by AWS Signer, so each of its members is a fact
//! the catalog states about that platform — not a value computed from another one.
//! The rule under test, over the whole published catalog rather than over one row:
//!
//! * both readers (`GetSigningPlatform` and `ListSigningPlatforms`) report, for
//!   every row, exactly the members the catalog publishes for it — the id, the
//!   display name character for character (surrounding whitespace included), the
//!   partner, the target, the category, the algorithm options, the image formats,
//!   the size bound and the revocation capability;
//! * no member is derived from another: a row's `partner` need not be a prefix of
//!   its `platformId` and its `target` need not be the remainder of it, and the
//!   catalog publishes at least one row where neither is — the row still reports
//!   its published pair, and a listing filtered on that pair still selects it;
//! * a `category` the catalog publishes is served as published even when this
//!   model's `Category` enum does not name that value, and the catalog publishes
//!   at least one such value;
//! * a member the catalog publishes NO value for is absent from both readers'
//!   answers rather than filled in with a plausible one, and the catalog publishes
//!   at least one row with no `signingImageFormat`;
//! * the display name a platform is published with is the `platformDisplayName` a
//!   profile created on it reports, verbatim.
//!
//! The rows are the whole catalog, so they differ along everything the rule
//! generalises over: the platform, whether its partner/target decompose its id,
//! which category it is published in, whether it publishes an image format, its
//! algorithms, its size bound and its revocation capability.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::ops::types::CATEGORY_VALUES;
use service_com_amazonaws_signer_wallabyservice::state::{
    SigningPlatformEntry, SIGNING_PLATFORM_CATALOG,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// An ACM certificate ARN for `signingMaterial`, so a profile can be created on
/// every published platform, including the ones that require it.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status and JSON body.
struct Answer {
    status: u16,
    body: Value,
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "signer.us-west-2.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_WEST_2);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// The whole published catalog, as one page of `ListSigningPlatforms`.
async fn listed(router: &MultiServiceRouter, query: &str) -> Vec<Value> {
    let answer = call(router, "GET", &format!("/signing-platforms{query}"), None).await;
    assert_eq!(
        answer.status, 200,
        "query {query:?}: expected ListSigningPlatforms 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["platforms"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "query {query:?}: expected a platforms list, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `SigningPlatform` members the catalog publishes for `entry`, as the wire
/// spells them — the expectation BOTH readers are held to.
///
/// Built from the stored row, member by member, so a reader that computes a member
/// from the platform id instead of reading it off the row fails here.
fn published(entry: &SigningPlatformEntry) -> Value {
    let mut expected = json!({
        "platformId": entry.platform_id,
        "partner": entry.partner,
        "target": entry.target,
        "category": entry.category,
        "signingConfiguration": {
            "encryptionAlgorithmOptions": {
                "allowedValues": entry.encryption_algorithms,
                "defaultValue": entry.encryption_algorithms[0],
            },
            "hashAlgorithmOptions": {
                "allowedValues": entry.hash_algorithms,
                "defaultValue": entry.hash_algorithms[0],
            },
        },
        "maxSizeInMB": entry.max_size_in_mb,
        "revocationSupported": entry.revocation_supported,
    });
    if let Some(display) = entry.display_name {
        expected["displayName"] = json!(display);
    }
    if let Some(default_format) = entry.image_formats.first() {
        expected["signingImageFormat"] = json!({
            "supportedFormats": entry.image_formats,
            "defaultFormat": default_format,
        });
    }
    expected
}

/// Whether `entry`'s partner and target are the two halves of its `platformId`,
/// split at the first `-` after the partner — the decomposition a reader must NOT
/// rely on, since the catalog publishes rows it does not hold for.
fn decomposes_its_id(entry: &SigningPlatformEntry) -> bool {
    entry.platform_id == format!("{}-{}", entry.partner, entry.target)
}

/// Both readers report every published row exactly as the catalog publishes it.
#[tokio::test]
async fn both_readers_report_every_published_member_verbatim() {
    let router = emulator();
    let page = listed(&router, "?maxResults=25").await;
    assert_eq!(
        page.len(),
        SIGNING_PLATFORM_CATALOG.len(),
        "expected the listing to report the whole published catalog of {} rows, got {page:?}",
        SIGNING_PLATFORM_CATALOG.len()
    );

    for (ordinal, entry) in SIGNING_PLATFORM_CATALOG.iter().enumerate() {
        let case = entry.platform_id;
        let expected = published(entry);

        // The listing reports the row at the position the catalog publishes it in,
        // with every published member and no other.
        assert_eq!(
            page[ordinal], expected,
            "case {case:?}: expected the listed row to be the published row {expected:?}, got {:?}",
            page[ordinal]
        );

        // The single-platform reader answers the SAME row.
        let got = call(&router, "GET", &format!("/signing-platforms/{case}"), None).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected GetSigningPlatform 200, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.body, expected,
            "case {case:?}: expected the get to answer the published row {expected:?}, got {:?}",
            got.body
        );
    }
}

/// A display name is served character for character, including surrounding
/// whitespace, and a row published with none reports none.
#[tokio::test]
async fn a_display_name_is_served_character_for_character() {
    let router = emulator();

    for entry in SIGNING_PLATFORM_CATALOG {
        let case = entry.platform_id;
        let got = call(&router, "GET", &format!("/signing-platforms/{case}"), None).await;
        match entry.display_name {
            Some(display) => {
                let reported = got.body["displayName"].as_str();
                assert_eq!(
                    reported,
                    Some(display),
                    "case {case:?}: expected displayName {display:?} verbatim, got {reported:?}"
                );
                // Trimming is what a reader that "tidies up" published data would
                // do, so the length is asserted too: it is the only thing that
                // separates a published trailing space from a dropped one.
                assert_eq!(
                    reported.unwrap_or_default().len(),
                    display.len(),
                    "case {case:?}: expected displayName to keep all {} characters it is \
                     published with, got {reported:?}",
                    display.len()
                );
            }
            None => assert!(
                got.body["displayName"].is_null(),
                "case {case:?}: expected no displayName for a platform published without one, \
                 got {:?}",
                got.body["displayName"]
            ),
        }
    }
}

/// The partner and the target are published facts, not segments of the platform
/// id: the catalog holds a row whose id does not decompose into them, and that row
/// still reports its published pair — and a listing filtered on either half still
/// selects it.
#[tokio::test]
async fn a_partner_and_target_are_published_rather_than_split_off_the_id() {
    let router = emulator();

    let undecomposed: Vec<&SigningPlatformEntry> = SIGNING_PLATFORM_CATALOG
        .iter()
        .filter(|entry| !decomposes_its_id(entry))
        .collect();
    assert!(
        !undecomposed.is_empty(),
        "expected the catalog to publish at least one row whose platformId is not \
         <partner>-<target>, so that a reader cannot pass by splitting the id; got {:?}",
        SIGNING_PLATFORM_CATALOG
            .iter()
            .map(|entry| (entry.platform_id, entry.partner, entry.target))
            .collect::<Vec<_>>()
    );

    for entry in undecomposed {
        let case = entry.platform_id;
        let got = call(&router, "GET", &format!("/signing-platforms/{case}"), None).await;
        assert_eq!(
            got.body["partner"].as_str(),
            Some(entry.partner),
            "case {case:?}: expected the published partner {:?}, got {:?}",
            entry.partner,
            got.body["partner"]
        );
        assert_eq!(
            got.body["target"].as_str(),
            Some(entry.target),
            "case {case:?}: expected the published target {:?}, got {:?}",
            entry.target,
            got.body["target"]
        );
    }

    // Each half is an `@httpQuery` filter of the listing, so a client that read a
    // published pair can narrow the catalog by it.
    for entry in SIGNING_PLATFORM_CATALOG {
        let case = entry.platform_id;
        for (member, value) in [("partner", entry.partner), ("target", entry.target)] {
            let page = listed(&router, &format!("?maxResults=25&{member}={value}")).await;
            let ids: Vec<&str> = page
                .iter()
                .map(|row| row["platformId"].as_str().unwrap_or_default())
                .collect();
            assert!(
                ids.contains(&case),
                "case {case:?}: expected a listing filtered on {member}={value:?} to select it, \
                 got {ids:?}"
            );
        }
    }
}

/// A published `category` is served as published, even when this model's enum does
/// not name that value.
#[tokio::test]
async fn a_category_outside_the_modelled_enum_is_served_as_published() {
    let router = emulator();

    let beyond_the_enum: Vec<&str> = SIGNING_PLATFORM_CATALOG
        .iter()
        .map(|entry| entry.category)
        .filter(|category| !CATEGORY_VALUES.contains(category))
        .collect();
    assert!(
        !beyond_the_enum.is_empty(),
        "expected the catalog to publish a category outside the modelled {CATEGORY_VALUES:?}, so \
         that clamping to the enum cannot pass; got {:?}",
        SIGNING_PLATFORM_CATALOG
            .iter()
            .map(|entry| entry.category)
            .collect::<Vec<_>>()
    );

    for entry in SIGNING_PLATFORM_CATALOG {
        let case = entry.platform_id;
        let got = call(&router, "GET", &format!("/signing-platforms/{case}"), None).await;
        assert_eq!(
            got.body["category"].as_str(),
            Some(entry.category),
            "case {case:?}: expected the published category {:?}, got {:?}",
            entry.category,
            got.body["category"]
        );
        // The category is also a filter, so a client can narrow the catalog by a
        // value the enum does not name.
        let page = listed(
            &router,
            &format!("?maxResults=25&category={}", entry.category),
        )
        .await;
        let ids: Vec<&str> = page
            .iter()
            .map(|row| row["platformId"].as_str().unwrap_or_default())
            .collect();
        assert!(
            ids.contains(&case),
            "case {case:?}: expected a listing filtered on category={:?} to select it, got {ids:?}",
            entry.category
        );
    }
}

/// A member the catalog publishes no value for is absent from both readers rather
/// than filled in — and the catalog holds at least one such row.
#[tokio::test]
async fn a_member_the_catalog_publishes_nothing_for_is_absent() {
    let router = emulator();
    let page = listed(&router, "?maxResults=25").await;

    assert!(
        SIGNING_PLATFORM_CATALOG
            .iter()
            .any(|entry| entry.image_formats.is_empty()),
        "expected the catalog to publish a row with no signingImageFormat, so that inventing one \
         cannot pass; got {:?}",
        SIGNING_PLATFORM_CATALOG
            .iter()
            .map(|entry| (entry.platform_id, entry.image_formats))
            .collect::<Vec<_>>()
    );

    for (ordinal, entry) in SIGNING_PLATFORM_CATALOG.iter().enumerate() {
        let case = entry.platform_id;
        let got = call(&router, "GET", &format!("/signing-platforms/{case}"), None).await;
        for (reader, body) in [("get", &got.body), ("list", &page[ordinal])] {
            if entry.image_formats.is_empty() {
                assert!(
                    body["signingImageFormat"].is_null(),
                    "case {case:?} ({reader}): expected no signingImageFormat for a platform \
                     published without one, got {:?}",
                    body["signingImageFormat"]
                );
            } else {
                assert_eq!(
                    body["signingImageFormat"]["defaultFormat"].as_str(),
                    entry.image_formats.first().copied(),
                    "case {case:?} ({reader}): expected the published default format, got {:?}",
                    body["signingImageFormat"]
                );
            }
        }
    }
}

/// One catalog: the display name a platform is published with is the
/// `platformDisplayName` a profile created on it reports, verbatim.
#[tokio::test]
async fn a_profile_reports_the_display_name_its_platform_publishes() {
    let router = emulator();

    for (ordinal, entry) in SIGNING_PLATFORM_CATALOG.iter().enumerate() {
        let case = entry.platform_id;
        let name = format!("Published_Row_Profile_{ordinal}");
        let put = call(
            &router,
            "PUT",
            &format!("/signing-profiles/{name}"),
            Some(json!({
                "platformId": case,
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
            })),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected PutSigningProfile 200, got {} {:?}",
            put.status, put.body
        );

        let profile = call(&router, "GET", &format!("/signing-profiles/{name}"), None).await;
        assert_eq!(
            profile.status, 200,
            "case {case:?}: expected GetSigningProfile 200, got {} {:?}",
            profile.status, profile.body
        );
        assert_eq!(
            profile.body["platformDisplayName"].as_str(),
            entry.display_name,
            "case {case:?}: expected the profile to report the published displayName {:?}, got {:?}",
            entry.display_name,
            profile.body["platformDisplayName"]
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The refusal every paginated listing of this service answers an unusable
//! `nextToken` with.
//!
//! Every test drives real HTTP through the router the production binary builds, so what
//! is asserted is the wire behaviour a `mediapackage-vod` client sees, and each test
//! owns its own engine. The request harness is the shared one
//! ([`super::packaging_lifecycle::call`] and friends).
//!
//! The RULE under test, over a TABLE of inputs rather than one recorded call: a
//! `nextToken` this service did not issue for the listing it arrived at is refused with
//! the modelled `UnprocessableEntityException` (HTTP 422, `errorKind client`) carrying
//! the FIXED message [`vod::INVALID_NEXT_TOKEN_MESSAGE`] — a sentence that interpolates
//! nothing from the request: it names no input member and echoes no offending token,
//! however the token is malformed and whichever listing it was sent to. A token the
//! listing DID issue still resumes it, and a listing asked for no token reports its
//! records.
//!
//! The per-attribute `errors` envelope these refusals travel in is
//! `tests/all/error_responses.rs`; the paging behaviour itself is
//! `tests/all/asset_listing.rs` and `tests/all/packaging_configuration_listing.rs`.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

use super::packaging_lifecycle::{call, create_asset, create_config, create_group, router, REGION};
// The status + `x-amzn-errortype` + body harness this crate's suites share, rather than
// another copy of it.
use super::resource_tag_listing::call as answer;

/// The wire error code of the refusal and its modelled `httpError`
/// (`smithy_query(query="operation",operation="ListAssets")`:
/// `UnprocessableEntityException`, `errorKind client`, `httpError 422`).
const UNPROCESSABLE: (&str, u16) = ("UnprocessableEntityException", 422);

/// One paginated listing of this service: what to call it, its path, and the output
/// member its page of records arrives in.
///
/// The three listings the model marks `@paginated` with `inputToken NextToken`
/// (`smithy_query(query="digest")`: `ListAssets` `GET /assets`,
/// `ListPackagingConfigurations` `GET /packaging_configurations`,
/// `ListPackagingGroups` `GET /packaging_groups`).
const LISTINGS: [(&str, &str, &str); 3] = [
    ("ListAssets", "/assets", "assets"),
    (
        "ListPackagingConfigurations",
        "/packaging_configurations",
        "packagingConfigurations",
    ),
    ("ListPackagingGroups", "/packaging_groups", "packagingGroups"),
];

/// Two groups, two configurations and two assets, so every listing has more records
/// than a one-record page and therefore issues a cursor of its own.
async fn populated() -> MultiServiceRouter {
    let router = router();
    for group in ["group-a", "group-b"] {
        create_group(&router, group, None).await;
    }
    for (index, group) in ["group-a", "group-b"].into_iter().enumerate() {
        create_config(
            &router,
            &format!("cfg-{index}"),
            group,
            json!({ "hlsPackage": { "hlsManifests": [{}] } }),
        )
        .await;
        create_asset(&router, &format!("asset-{index}"), group).await;
    }
    router
}

/// The cursor `path` hands out for a page of one record, which must be non-empty for
/// the store [`populated`] builds.
async fn issued_cursor(router: &MultiServiceRouter, operation: &str, path: &str) -> String {
    let (status, page) = call(router, "GET", &format!("{path}?maxResults=1"), None).await;
    assert_eq!(
        status, 200,
        "case {operation:?}: a one-record page is a 200, got {status} body {page}"
    );
    let token = page["nextToken"].as_str().unwrap_or_default().to_string();
    assert!(
        !token.is_empty(),
        "case {operation:?}: with more records than one page the listing must issue a \
         cursor, got {page}"
    );
    token
}

#[tokio::test]
async fn every_listing_refuses_an_unissued_cursor_with_the_fixed_message() {
    for (operation, path, member) in LISTINGS {
        let router = populated().await;
        let own = issued_cursor(&router, operation, path).await;
        // A cursor ANOTHER listing of this service issued is not one THIS listing
        // issued, so it is refused exactly like a malformed one.
        let foreign_path = LISTINGS
            .iter()
            .map(|(_, other, _)| *other)
            .find(|other| *other != path)
            .expect("the model declares more than one paginated listing");
        let foreign = issued_cursor(&router, operation, foreign_path).await;

        // (case, the token as it appears in the query string) — the forms span an
        // absent value, non-hex text, a token too short to carry a checksum, a
        // checksum that does not match the id it carries, each half of a real cursor
        // on its own, non-ASCII text, a very long token, and a cursor another listing
        // issued.
        let forged: Vec<(&str, String)> = vec![
            ("empty", String::new()),
            ("not hex", "this-is-not-a-valid-pagination-token".to_string()),
            ("too short", "abcd".to_string()),
            ("checksum does not match the id", "00000000616263".to_string()),
            ("checksum only", own[..8].to_string()),
            ("payload only", own[8..].to_string()),
            ("flipped checksum", format!("ffffffff{}", &own[8..])),
            ("non-ascii", "%F0%9F%99%88%F0%9F%99%88".to_string()),
            ("very long", "ab".repeat(600)),
            ("a cursor another listing issued", foreign),
        ];

        for (form, token) in forged {
            let case = format!("{operation} / {form}");
            let refused =
                answer(&router, REGION, "GET", &format!("{path}?nextToken={token}"), None).await;
            assert_eq!(
                (refused.status, refused.errortype.as_str()),
                (UNPROCESSABLE.1, UNPROCESSABLE.0),
                "case {case:?}: expected HTTP {} / {}, got HTTP {} / {:?} body {}",
                UNPROCESSABLE.1,
                UNPROCESSABLE.0,
                refused.status,
                refused.errortype,
                refused.body
            );
            assert_eq!(
                refused.body["message"].as_str(),
                Some(vod::INVALID_NEXT_TOKEN_MESSAGE),
                "case {case:?}: expected the fixed refusal message, got {}",
                refused.body
            );
            // Fixed means fixed: nothing of the request is interpolated into it.
            if !token.is_empty() {
                assert!(
                    !vod::INVALID_NEXT_TOKEN_MESSAGE.contains(token.as_str()),
                    "case {case:?}: the message must not echo the offending token, got {}",
                    refused.body
                );
            }
            assert_eq!(
                refused.body["errors"],
                json!({ "nextToken": ["is not a cursor this service issued"] }),
                "case {case:?}: expected the violation attributed to `nextToken`, got {}",
                refused.body
            );
        }

        // The positive controls: the listing with no token reports its records, and
        // the cursor it DID issue is still accepted.
        let (status, unpaged) = call(&router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: an untokened listing is a 200, got {status} body {unpaged}"
        );
        assert_eq!(
            unpaged[member].as_array().map(Vec::len),
            Some(2),
            "case {operation:?}: the untokened listing must report both stored records, \
             got {unpaged}"
        );
        let (status, resumed) =
            call(&router, "GET", &format!("{path}?nextToken={own}"), None).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: the cursor this listing issued must be accepted, got \
             {status} body {resumed}"
        );
        assert_eq!(
            resumed[member].as_array().map(Vec::len),
            Some(1),
            "case {operation:?}: resuming after the first record must report the second, \
             got {resumed}"
        );
    }
}

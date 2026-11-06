// SPDX-License-Identifier: CC0-1.0

//! What a `nextToken` IS, on every paginated listing of this service.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — so what is asserted is the wire behaviour, not a handler call.
//!
//! The rule under test: a page cursor is a position in ONE enumeration — one collection,
//! of one Region, selected by one set of filters, in key order — that the listing itself
//! handed out, so a listing resumes only at a cursor it issued for that same enumeration
//! and refuses any other `nextToken` with `UnprocessableEntityException` (422) instead of
//! using it as a lower bound, which would serve an arbitrary slice of the collection as
//! if it were a page.
//!
//! The rows below span the class the rule generalises over: WHICH listing (all three),
//! and HOW the token fails to be one that enumeration issued — unreadable, well-formed
//! but never minted, a minted one altered, and a minted one of another listing, another
//! Region or another filter.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{
    call_in, emulator, error_code, CROSS_ACCOUNT_HARVEST_ROLE, REGION,
};

/// One paginated listing of this service: the collection it enumerates and the output
/// member its items arrive in.
///
/// Source for the three and their bindings:
/// `smithy_query(query="operation",operation="ListChannels,ListOriginEndpoints,ListHarvestJobs")`
/// — `GET /channels` (`Channels`), `GET /origin_endpoints` (`OriginEndpoints`) and
/// `GET /harvest_jobs` (`HarvestJobs`), each taking `NextToken` in and answering one out.
struct Listing {
    path: &'static str,
    items: &'static str,
}

const LISTINGS: &[Listing] = &[
    Listing {
        path: "/channels",
        items: "channels",
    },
    Listing {
        path: "/origin_endpoints",
        items: "originEndpoints",
    },
    Listing {
        path: "/harvest_jobs",
        items: "harvestJobs",
    },
];

/// The whole sentence a refused `nextToken` is answered with
/// (`crate::state::INVALID_PAGE_TOKEN_MESSAGE`): the service's own wording for the class,
/// stating no member and no part of the request.
const TOKEN_MESSAGE: &str = "Invalid NextToken";

/// The ids this suite seeds each collection with: four per collection, so every listing
/// has several pages at the small page sizes below, and the ids differ in their first
/// character so a cursor that skipped ahead loses a known prefix of the enumeration.
const SEED_IDS: &[&str] = &["alpha", "bravo", "charlie", "zulu"];

/// The channel every seeded endpoint — and so every seeded harvest job — belongs to,
/// which is [`SEED_IDS`]'s first id. One channel holds them all, so the `channelId` and
/// `includeChannelId` filters have an enumeration of several pages of their own, and
/// every other seeded channel filters down to none.
const FILTER_CHANNEL: &str = "alpha";

/// Seed four channels, four origin endpoints and four harvest jobs in `region`, each
/// collection keyed by [`SEED_IDS`], asserting every create was accepted.
async fn seed(router: &MultiServiceRouter, region: &str) {
    for id in SEED_IDS {
        let (status, body) = call_in(
            router,
            region,
            "POST",
            "/channels",
            Some(&json!({ "id": id })),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding channel {id:?} in {region}: expected 200, got {status} {body:?}"
        );
        // Every endpoint packages the one filtered channel, and a harvest needs content
        // to export, so each retains a startover window.
        let (status, body) = call_in(
            router,
            region,
            "POST",
            "/origin_endpoints",
            Some(&json!({
                "channelId": FILTER_CHANNEL,
                "id": id,
                "hlsPackage": {},
                "startoverWindowSeconds": 3_600
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding endpoint {id:?} in {region}: expected 200, got {status} {body:?}"
        );
        let (status, body) = call_in(
            router,
            region,
            "POST",
            "/harvest_jobs",
            Some(&json!({
                "id": id,
                "originEndpointId": id,
                "startTime": "2026-01-01T00:00:00Z",
                "endTime": "2026-01-01T01:00:00Z",
                "s3Destination": {
                    "bucketName": "harvest-bucket",
                    "manifestKey": "index.m3u8",
                    "roleArn": CROSS_ACCOUNT_HARVEST_ROLE
                }
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding harvest job {id:?} in {region}: expected 200, got {status} {body:?}"
        );
    }
}

/// `path` with `query` appended, whichever of the two carries a query string already.
fn with_query(path: &str, query: &str) -> String {
    let separator = if path.contains('?') { '&' } else { '?' };
    format!("{path}{separator}{query}")
}

/// The ids one page of `path` serves at `page_size`, resumed at `token` when there is
/// one, together with the `nextToken` that page carried. `path` may already carry a
/// filter, which is part of the enumeration being paged.
async fn page_in(
    router: &MultiServiceRouter,
    region: &str,
    path: &str,
    items: &str,
    page_size: usize,
    token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut uri = with_query(path, &format!("maxResults={page_size}"));
    if let Some(token) = token {
        uri = with_query(&uri, &format!("nextToken={token}"));
    }
    let (status, body) = call_in(router, region, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {uri:?}: an issued cursor must be served, got {status} {body:?}"
    );
    let ids = body
        .get(items)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect::<Vec<String>>()
        })
        .unwrap_or_else(|| panic!("case {uri:?}: a page carries {items}, got {body:?}"));
    let next = body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (ids, next)
}

/// [`page_in`] for the suite's default Region.
async fn page(
    router: &MultiServiceRouter,
    path: &str,
    items: &str,
    page_size: usize,
    token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    page_in(router, REGION, path, items, page_size, token).await
}

/// A token the enumeration `path` names really issued: the one its first page of
/// `page_size` carried.
async fn issued_token(
    router: &MultiServiceRouter,
    region: &str,
    path: &str,
    items: &str,
    page_size: usize,
) -> String {
    let (_, next) = page_in(router, region, path, items, page_size, None).await;
    next.unwrap_or_else(|| {
        panic!("{path} must carry a nextToken while more than {page_size} records remain")
    })
}

/// `token` with its last character changed to another hex digit: a string of the right
/// shape and length that this service never handed out.
fn with_last_character_changed(token: &str) -> String {
    let mut altered: Vec<char> = token.chars().collect();
    if let Some(last) = altered.last_mut() {
        *last = if *last == '0' { '1' } else { '0' };
    }
    altered.into_iter().collect()
}

/// `token` with its first character changed to another hex digit: the key it names is no
/// longer the one the rest of it was issued for.
fn with_first_character_changed(token: &str) -> String {
    let mut altered: Vec<char> = token.chars().collect();
    if let Some(first) = altered.first_mut() {
        *first = if *first == '6' { '7' } else { '6' };
    }
    altered.into_iter().collect()
}

/// Assert that `path` with `token` is refused as a `nextToken` this service did not
/// issue: the modelled 422, stating the service's sentence, with no items served.
async fn assert_refused(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    path: &str,
    items: &str,
    token: &str,
) {
    let uri = with_query(path, &format!("nextToken={token}"));
    let (status, body) = call_in(router, region, "GET", &uri, None).await;
    assert_eq!(
        status, 422,
        "case {case:?} ({uri:?}): expected 422, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "UnprocessableEntityException",
        "case {case:?} ({uri:?}): expected UnprocessableEntityException, got {body:?}"
    );
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(TOKEN_MESSAGE),
        "case {case:?} ({uri:?}): expected the service's sentence, got {body:?}"
    );
    assert_eq!(
        body.get("errors"),
        None,
        "case {case:?} ({uri:?}): the sentence is the whole refusal, so no errors entry, \
         got {body:?}"
    );
    assert_eq!(
        body.get(items),
        None,
        "case {case:?} ({uri:?}): a refused listing must serve no items, got {body:?}"
    );
}

#[tokio::test]
async fn a_next_token_the_listing_did_not_issue_is_refused_on_every_paginated_listing() {
    let router = emulator();
    seed(&router, REGION).await;

    for listing in LISTINGS {
        // The cursor this listing really did hand out, which the altered rows below are
        // built from so each one differs from an accepted token in exactly one way.
        let genuine = issued_token(&router, REGION, listing.path, listing.items, 1).await;
        let rows: Vec<(String, String)> = vec![
            ("not hex at all".to_string(), "not-a-token".to_string()),
            ("odd-length hex".to_string(), "abc".to_string()),
            (
                "letters outside the hex alphabet".to_string(),
                "zz".to_string(),
            ),
            ("percent noise".to_string(), "%%%".to_string()),
            // Well-formed, even-length hex of a key that sorts inside the collection
            // ("7a" is "z"), which no call of this service ever handed out: honouring it
            // would silently skip every record sorting below it.
            (
                "hex of a key that sorts mid-collection".to_string(),
                "7a".to_string(),
            ),
            (
                "hex of the first stored key".to_string(),
                hex::encode(SEED_IDS[0]),
            ),
            (
                "hex of the key the next page starts at".to_string(),
                hex::encode(SEED_IDS[1]),
            ),
            (
                "hex of a key no record uses".to_string(),
                hex::encode("mike"),
            ),
            // A token of the right shape whose tail does not belong to its key, and one
            // whose key does not belong to its tail.
            (
                "an issued token with its last character changed".to_string(),
                with_last_character_changed(&genuine),
            ),
            (
                "an issued token with its first character changed".to_string(),
                with_first_character_changed(&genuine),
            ),
            (
                "an issued token truncated".to_string(),
                genuine[..genuine.len() - 2].to_string(),
            ),
            (
                "an issued token with characters appended".to_string(),
                format!("{genuine}00"),
            ),
            // Far longer than any key this service stores a record under.
            ("hex far longer than any key".to_string(), "ab".repeat(4_096)),
        ];
        for (case, token) in rows {
            let case = format!("{}: {case}", listing.path);
            assert_refused(&router, REGION, &case, listing.path, listing.items, &token).await;
        }
    }
}

#[tokio::test]
async fn a_token_issued_by_another_listing_or_another_region_is_refused() {
    let router = emulator();
    seed(&router, REGION).await;
    // A second Region, so a cursor of one Region's enumeration can be offered to another.
    let other_region = "us-west-2";
    seed(&router, other_region).await;

    for listing in LISTINGS {
        let genuine = issued_token(&router, REGION, listing.path, listing.items, 1).await;
        // Every OTHER listing is offered this listing's cursor: the collections are
        // seeded with the same ids, so a listing that honoured it would answer 200 with a
        // short page rather than refuse.
        for other in LISTINGS {
            if other.path == listing.path {
                continue;
            }
            let case = format!("{}'s cursor offered to {}", listing.path, other.path);
            assert_refused(&router, REGION, &case, other.path, other.items, &genuine).await;
        }
        // And the same cursor offered to the same listing in another Region, whose
        // collection holds the same ids.
        let case = format!(
            "{} cursor of {REGION} offered in {other_region}",
            listing.path
        );
        assert_refused(
            &router,
            other_region,
            &case,
            listing.path,
            listing.items,
            &genuine,
        )
        .await;
    }
}

/// The filters that narrow WHICH records a listing walks, and so which enumeration a
/// cursor belongs to: `ChannelId` on `ListOriginEndpoints` and `IncludeChannelId` on
/// `ListHarvestJobs` (`smithy_query(query="operation",operation="ListOriginEndpoints,ListHarvestJobs")`
/// binds them as `@httpQuery("channelId")` and `@httpQuery("includeChannelId")`).
const FILTERED: &[(&str, &str, &str)] = &[
    ("/origin_endpoints", "originEndpoints", "channelId"),
    ("/harvest_jobs", "harvestJobs", "includeChannelId"),
];

#[tokio::test]
async fn a_cursor_of_one_filtered_walk_is_not_honoured_for_another() {
    let router = emulator();
    seed(&router, REGION).await;

    for (path, items, filter) in FILTERED {
        let matching = with_query(path, &format!("{filter}={FILTER_CHANNEL}"));
        // The filter narrows which records are walked, so the filtered walk is its own
        // enumeration: it serves every matching record in key order from its own cursors.
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=SEED_IDS.len() {
            let (ids, next) = page(&router, &matching, items, 1, token.as_deref()).await;
            seen.extend(ids);
            match next {
                Some(next) => token = Some(next),
                None => {
                    token = None;
                    break;
                }
            }
        }
        let expected: Vec<String> = SEED_IDS.iter().map(|id| id.to_string()).collect();
        assert_eq!(
            (seen, token),
            (expected, None),
            "case {matching:?}: a filtered walk must serve every matching record exactly \
             once in key order and terminate"
        );

        // A cursor of the filtered walk is not a position in the unfiltered one, nor in
        // the walk of another filter value — and the unfiltered cursor is not a position
        // in the filtered walk either. Each row would otherwise be served as a page,
        // because the same keys exist in both walks.
        let filtered_cursor = issued_token(&router, REGION, &matching, items, 1).await;
        let unfiltered_cursor = issued_token(&router, REGION, path, items, 1).await;
        let other_value = with_query(path, &format!("{filter}={}", SEED_IDS[1]));
        let rows: Vec<(String, &str, &str)> = vec![
            (
                format!("{matching}'s cursor offered unfiltered"),
                path,
                filtered_cursor.as_str(),
            ),
            (
                format!("{matching}'s cursor offered with {filter}={}", SEED_IDS[1]),
                other_value.as_str(),
                filtered_cursor.as_str(),
            ),
            (
                format!("{path}'s unfiltered cursor offered with {filter}={FILTER_CHANNEL}"),
                matching.as_str(),
                unfiltered_cursor.as_str(),
            ),
        ];
        for (case, target, token) in rows {
            assert_refused(&router, REGION, &case, target, items, token).await;
        }
    }
}

#[tokio::test]
async fn an_issued_token_resumes_the_enumeration_that_minted_it() {
    let router = emulator();
    seed(&router, REGION).await;
    let expected: Vec<String> = SEED_IDS.iter().map(|id| id.to_string()).collect();

    for listing in LISTINGS {
        // Page sizes differing in how they split the four seeded records: one at a time,
        // an uneven split, the exact size of the collection, and more than it holds.
        for page_size in [1, 3, SEED_IDS.len(), SEED_IDS.len() + 1] {
            let case = format!("{} at maxResults={page_size}", listing.path);
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            // Bounded: one page per record plus one, so a cursor that failed to advance
            // cannot spin.
            for _ in 0..=SEED_IDS.len() {
                let (ids, next) =
                    page(&router, listing.path, listing.items, page_size, token.as_deref()).await;
                assert!(
                    ids.len() <= page_size,
                    "case {case:?}: a page holds at most {page_size} ids, got {ids:?}"
                );
                seen.extend(ids.iter().cloned());
                match next {
                    Some(next) => {
                        assert_eq!(
                            ids.len(),
                            page_size,
                            "case {case:?}: a short page must carry no nextToken, got {ids:?}"
                        );
                        token = Some(next);
                    }
                    None => {
                        token = None;
                        break;
                    }
                }
            }
            assert_eq!(
                token, None,
                "case {case:?}: paging must terminate, still resuming at {token:?}"
            );
            assert_eq!(
                seen, expected,
                "case {case:?}: paging must serve every record exactly once in key order, \
                 got {seen:?}"
            );
        }

        // A cursor is a position, not a one-shot handle: resuming at the same issued token
        // twice serves the same page both times, and reading it changes nothing.
        let genuine = issued_token(&router, REGION, listing.path, listing.items, 2).await;
        let (first, first_next) =
            page(&router, listing.path, listing.items, 2, Some(&genuine)).await;
        let (again, again_next) =
            page(&router, listing.path, listing.items, 2, Some(&genuine)).await;
        assert_eq!(
            (first, first_next),
            (again, again_next),
            "case {}: resuming at the same cursor must serve the same page twice",
            listing.path
        );
        // And the token a listing issues for one position is the same on every call, so a
        // client that stored one can still use it.
        let reissued = issued_token(&router, REGION, listing.path, listing.items, 2).await;
        assert_eq!(
            reissued, genuine,
            "case {}: the cursor for one position must be the same token on every call, \
             got {reissued:?} then {genuine:?}",
            listing.path
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The two pagination rules EVERY paginated operation of this service shares, asserted
//! through each operation that follows them.
//!
//! Every case here drives the real request path through the shared harness
//! (`tests/all/harness.rs`) — the router picks the service from the SigV4 credential
//! scope, the framework deserializes and validates the request, the handler runs, and the
//! response is serialized as `restJson1` — so what these tests assert is what a client
//! sees.
//!
//! The rules under test:
//!
//! * PAGE SIZE: one page carries `maxResults` records when the request names one, the
//!   operation's own default when it names none, and never more than the ceiling the
//!   model gives that member — asserted for `ListFeeds` (whose member documents a default
//!   of 10 results per page), `ListDictionaries` and `SearchFixtures` (whose members
//!   document none, so the modelled maximum stands in), over a table of requested sizes
//!   that spans one record per page, a size that does not divide the count, an omitted
//!   size and a page larger than the whole selection;
//! * REGION-SCOPED TOKEN CRITERIA: a listing that enumerates one Region's records binds
//!   its `nextToken` to its operation AND to that Region, so each of the four
//!   (operation, Region) pairs below is served only by its own token.

use crate::harness::{
    call_in_region, message_of, minimal_feed, router_with_engine, REGION,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::ops::types::Competitor;
use service_com_amazonaws_elementalinference_elementalinference::state::{
    fixture_start_secs, parse_fixture_date, ComAmazonawsElementalinferenceElementalinferenceState,
    DataSourceSport, Fixture, FIXTURE_STATUS_SCHEDULED, PAGE_TOKEN_OTHER_REQUEST_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// How many records each page-size row is asserted over: more than the smallest default
/// (10) and not a multiple of every requested size, so the last page is a partial one.
const RECORD_COUNT: usize = 12;

/// The largest page any of these operations admits: the `@range` maximum the model gives
/// `maxResults` on all three (`smithy_query(query="operation",operation="ListFeeds")`,
/// `…="ListDictionaries"`, `…="SearchFixtures"` each give it `{min: 1, max: 100}`).
const CEILING: i32 = 100;

/// The UTC day the fixtures this suite seeds are scheduled on, and the day a search
/// selects them with.
const FIXTURE_DAY: &str = "2026-05-04";

/// Another Region's endpoint, for the rows about which endpoint issued a token.
const OTHER_REGION: &str = "eu-west-1";

/// One paginated operation, and the page size it answers with when a request omits
/// `maxResults`.
struct Listing {
    /// The operation's name, which also selects its wire path below.
    label: &'static str,
    /// The page size a request that names no `maxResults` is answered with.
    default_page_size: usize,
}

/// The three paginated operations of this service, each with the default its own member
/// documents: "If MaxResults is not included in the request, the service defaults to
/// pagination with a maximum of 10 results per page" (`ListFeedsRequest$maxResults`),
/// while `ListDictionariesRequest$maxResults` and `SearchFixturesRequest$maxResults`
/// document no default, so the modelled maximum stands in.
const LISTINGS: [Listing; 3] = [
    Listing {
        label: "ListFeeds",
        default_page_size: 10,
    },
    Listing {
        label: "ListDictionaries",
        default_page_size: 100,
    },
    Listing {
        label: "SearchFixtures",
        default_page_size: 100,
    },
];

/// One seeded fixture of the day this suite searches: the id and the slot are all a
/// page-size row selects on.
fn seeded_fixture(index: usize) -> Fixture {
    let epoch_day = parse_fixture_date(FIXTURE_DAY)
        .unwrap_or_else(|| panic!("{FIXTURE_DAY:?} is a real UTC day"));
    Fixture {
        id: format!("pgfx-{index:02}"),
        sport: DataSourceSport::Basketball,
        name: format!("Page Panthers at Row Ravens {index}"),
        group: None,
        scheduled_start_secs: fixture_start_secs(epoch_day, (index % 24) as i64, 0)
            .unwrap_or_else(|| panic!("slot {index} of {FIXTURE_DAY:?} is a representable instant")),
        status: FIXTURE_STATUS_SCHEDULED.to_string(),
        competitors: vec![
            Competitor {
                name: Some("Page Panthers".to_string()),
                is_home: Some(true),
            },
            Competitor {
                name: Some("Row Ravens".to_string()),
                is_home: Some(false),
            },
        ],
    }
}

/// Create one feed through `region`'s endpoint.
async fn create_feed_in(router: &MultiServiceRouter, region: &str, name: &str) {
    let (status, created) =
        call_in_region(router, region, "POST", "/v1/feed", Some(minimal_feed(name))).await;
    assert_eq!(
        status, 202,
        "CreateFeed({name}) in {region} answers 202, got {status} with {created}"
    );
}

/// Create one dictionary through `region`'s endpoint.
async fn create_dictionary_in(router: &MultiServiceRouter, region: &str, name: &str) {
    let (status, created) = call_in_region(
        router,
        region,
        "POST",
        "/v1/dictionary",
        Some(json!({"name": name, "language": "eng"})),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateDictionary({name}) in {region} answers 202, got {status} with {created}"
    );
}

/// A router whose store holds exactly `count` records of the kind `listing` enumerates,
/// in [`REGION`] where the listing is region-scoped.
async fn seeded_router(listing: &Listing, count: usize) -> MultiServiceRouter {
    let (router, engine) = router_with_engine();
    match listing.label {
        "ListFeeds" => {
            for index in 0..count {
                create_feed_in(&router, REGION, &format!("page-feed-{index:02}")).await;
            }
        }
        "ListDictionaries" => {
            for index in 0..count {
                create_dictionary_in(&router, REGION, &format!("pagedict{index:02}")).await;
            }
        }
        "SearchFixtures" => {
            let state = engine.state::<ComAmazonawsElementalinferenceElementalinferenceState>();
            let mut state = state.lock();
            state.clear_fixtures();
            state.seed_fixtures((0..count).map(seeded_fixture));
        }
        other => panic!("no such paginated operation: {other}"),
    }
    router
}

/// Ask `listing` for one page, with the `maxResults` and `nextToken` the row carries.
async fn one_page(
    router: &MultiServiceRouter,
    listing: &Listing,
    region: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (u16, Value) {
    if listing.label == "SearchFixtures" {
        let mut body = json!({"sport": "basketball", "startDate": FIXTURE_DAY});
        let object = body.as_object_mut().expect("the body is an object");
        if let Some(max) = max_results {
            object.insert("maxResults".to_string(), json!(max));
        }
        if let Some(token) = next_token {
            object.insert("nextToken".to_string(), json!(token));
        }
        return call_in_region(router, region, "POST", "/v1/fixtures", Some(body)).await;
    }
    let mut query: Vec<String> = Vec::new();
    if let Some(max) = max_results {
        query.push(format!("maxResults={max}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let path = list_path(listing.label);
    let uri = if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", query.join("&"))
    };
    call_in_region(router, region, "GET", &uri, None).await
}

/// The collection path a region-scoped listing is served under (`OPERATIONS.md`).
fn list_path(label: &str) -> &'static str {
    match label {
        "ListFeeds" => "/v1/feeds",
        "ListDictionaries" => "/v1/dictionaries",
        other => panic!("{other} is not a region-scoped listing"),
    }
}

/// The ids one page reports, in the order it reports them.
fn ids_of(listing: &Listing, page: &Value) -> Vec<String> {
    let (member, id_member) = match listing.label {
        "ListFeeds" => ("feeds", "id"),
        "ListDictionaries" => ("dictionaries", "id"),
        "SearchFixtures" => ("fixtures", "fixtureId"),
        other => panic!("no such paginated operation: {other}"),
    };
    page[member]
        .as_array()
        .unwrap_or_else(|| panic!("the response carries a {member} list, got {page}"))
        .iter()
        .map(|summary| {
            summary[id_member]
                .as_str()
                .unwrap_or_else(|| panic!("a summary carries a {id_member}, got {summary}"))
                .to_string()
        })
        .collect()
}

/// The `nextToken` a page carries, or `None` when it carries none.
fn token_of(page: &Value) -> Option<String> {
    page["nextToken"].as_str().map(str::to_string)
}

/// Page through `listing` with `max_results` per page, returning the ids in the order the
/// pages reported them and how many pages it took.
async fn page_through(
    router: &MultiServiceRouter,
    listing: &Listing,
    label: &str,
    max_results: Option<i32>,
    page_size: usize,
) -> (Vec<String>, usize) {
    // An empty selection still answers one page, and the last page may be a partial one.
    let max_pages = RECORD_COUNT.div_ceil(page_size).max(1);
    let mut seen: Vec<String> = Vec::new();
    let mut pages = 0usize;
    let mut next_token: Option<String> = None;
    loop {
        let (status, page) = one_page(
            router,
            listing,
            REGION,
            max_results,
            next_token.as_deref(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: page {} answers 200, got {status} with {page}",
            pages + 1
        );
        pages += 1;
        assert!(
            pages <= max_pages,
            "case {label:?}: paging terminates within {max_pages} pages, got page {pages}: {page}"
        );
        let ids = ids_of(listing, &page);
        next_token = token_of(&page);
        if next_token.is_some() {
            assert_eq!(
                ids.len(),
                page_size,
                "case {label:?}: a page that carries a nextToken is a FULL page, got {ids:?}"
            );
        }
        seen.extend(ids);
        if next_token.is_none() {
            return (seen, pages);
        }
    }
}

/// Every paginated operation resolves one page's size the same way: `maxResults` when the
/// request names one, the operation's own default when it names none, bounded by the
/// ceiling the model gives that member — and the pages it issues together report every
/// selected record exactly once, in one stable order.
#[tokio::test]
async fn every_paginated_operation_resolves_its_page_size_by_the_shared_rule() {
    // The requested page sizes: one record a page, a size that does not divide the
    // record count, a size the records nearly fill, no size at all, and the ceiling.
    let requested: [Option<i32>; 5] = [Some(1), Some(5), Some(11), None, Some(CEILING)];

    for listing in &LISTINGS {
        let router = seeded_router(listing, RECORD_COUNT).await;

        // The one order every page of this listing is positioned in, read in a single
        // page of the ceiling.
        let (status, whole) = one_page(&router, listing, REGION, Some(CEILING), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: a page of the ceiling answers 200, got {status} with {whole}",
            listing.label
        );
        let order = ids_of(listing, &whole);
        assert_eq!(
            order.len(),
            RECORD_COUNT,
            "case {:?}: the ceiling reports every seeded record, got {order:?}",
            listing.label
        );
        assert!(
            token_of(&whole).is_none(),
            "case {:?}: one page holds the whole selection, so no nextToken, got {whole}",
            listing.label
        );

        for max_results in requested {
            let label = format!("{} with maxResults={max_results:?}", listing.label);
            // THE RULE: the requested size, else this operation's default, never more
            // than the selection holds.
            let page_size = max_results.map_or(listing.default_page_size, |max| max as usize);
            let expected_first = page_size.min(RECORD_COUNT);

            let (status, first) = one_page(&router, listing, REGION, max_results, None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the listing answers 200, got {status} with {first}"
            );
            let first_ids = ids_of(listing, &first);
            assert_eq!(
                first_ids.len(),
                expected_first,
                "case {label:?}: page one carries {expected_first} records, got {first_ids:?}"
            );
            assert_eq!(
                token_of(&first).is_some(),
                expected_first < RECORD_COUNT,
                "case {label:?}: a nextToken comes back exactly while a record remains, got \
                 {first}"
            );

            let (seen, pages) =
                page_through(&router, listing, &label, max_results, page_size).await;
            assert_eq!(
                seen, order,
                "case {label:?}: the pages report every record exactly once, in the one order, \
                 got {seen:?}"
            );
            assert_eq!(
                pages,
                RECORD_COUNT.div_ceil(page_size),
                "case {label:?}: {RECORD_COUNT} records in pages of {page_size}, got {pages} \
                 pages"
            );
        }
    }
}

/// Assert `answered` is the modelled refusal for a token that belongs to another request.
fn assert_refused(label: &str, answered: (u16, Value)) {
    let (status, error) = answered;
    assert_eq!(
        status, 400,
        "case {label:?}: a token of another request is refused with a 400, got {status} with \
         {error}"
    );
    assert_eq!(
        error["__type"],
        json!("ValidationException"),
        "case {label:?}: that 400 is a ValidationException, got {error}"
    );
    assert_eq!(
        message_of(&error),
        PAGE_TOKEN_OTHER_REQUEST_MESSAGE,
        "case {label:?}: expected the sentence for a token of another request, got {error}"
    );
}

/// A region-scoped listing binds its `nextToken` to BOTH the operation that issued it and
/// the Region whose records it enumerates: each (operation, Region) pair serves only its
/// own token and refuses every other pair's.
#[tokio::test]
async fn a_region_scoped_listing_token_binds_its_operation_and_region() {
    let (router, _engine) = router_with_engine();
    for region in [REGION, OTHER_REGION] {
        for index in 0..2 {
            create_feed_in(&router, region, &format!("bind-feed-{index}")).await;
            create_dictionary_in(&router, region, &format!("binddict{index}")).await;
        }
    }

    // The four (operation, Region) pairs, each with the token its own page one issued.
    let mut pairs: Vec<(&Listing, &str, String)> = Vec::new();
    for listing in LISTINGS.iter().filter(|l| l.label != "SearchFixtures") {
        for region in [REGION, OTHER_REGION] {
            let (status, first) = one_page(&router, listing, region, Some(1), None).await;
            assert_eq!(
                status, 200,
                "page one of {} in {region} answers 200, got {status} with {first}",
                listing.label
            );
            let token = token_of(&first).unwrap_or_else(|| {
                panic!(
                    "a record remains after page one of {} in {region}, so a nextToken comes \
                     back, got {first}",
                    listing.label
                )
            });
            pairs.push((listing, region, token));
        }
    }

    for (owner, owner_region, token) in &pairs {
        for (listing, region, _) in &pairs {
            let label = format!(
                "a {} token of {owner_region} submitted to {} in {region}",
                owner.label, listing.label
            );
            let answered = one_page(&router, listing, region, Some(1), Some(token)).await;
            if owner.label == listing.label && owner_region == region {
                let (status, page) = answered;
                assert_eq!(
                    status, 200,
                    "case {label:?}: its own token is served, got {status} with {page}"
                );
                assert_eq!(
                    ids_of(listing, &page).len(),
                    1,
                    "case {label:?}: page two reports the one remaining record, got {page}"
                );
            } else {
                assert_refused(&label, answered);
            }
        }
    }
}

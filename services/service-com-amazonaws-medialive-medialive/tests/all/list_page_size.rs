// SPDX-License-Identifier: CC0-1.0

//! The page size this service's list operations answer with, end to end over the
//! restJson1 wire.
//!
//! The rule both suites below assert, over a TABLE spanning the class rather
//! than one recorded request: a page carries as many items as the request's
//! `MaxResults` asks for while items are left to report — up to the upper bound
//! of the `MaxResults` range the service publishes, which is also the size a
//! request naming no `MaxResults` gets — and `NextToken` is handed out only when
//! an item is left over after the page.
//!
//! Suite 1 varies the asked-for size against one collection, spanning sizes
//! below, at and past both the stored count and the published bound. Suite 2
//! varies the OPERATION, because the rule is the service's and not one
//! listing's.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour rather than an internal function's return value.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_LIST_PAGE_SIZE;
use smithy_server_runtime::MultiServiceRouter;

use crate::multiplex_lifecycle::{
    create_body, settings, TRANSPORT_STREAM_BITRATE, TRANSPORT_STREAM_ID,
};
use crate::wire::{call, medialive};

/// The upper bound of the `MaxResults` range AWS publishes for this service's
/// list operations ("Valid Range: Minimum value of 1. Maximum value of 1000").
/// The model declares no range on the member itself
/// (`smithy_query(query="shape", shape="MaxResults")`), so the published range is
/// where the bound comes from, and the emulator's own ceiling is that bound.
const PUBLISHED_MAX_RESULTS: usize = 1000;

/// The channel collection `CreateChannel` and `ListChannels` are bound to
/// (`smithy_query(query="operation", operation="CreateChannel,ListChannels")`:
/// POST / GET /prod/channels).
const CHANNELS: &str = "/prod/channels";

/// How many channels suite 1 stores: more than a client would hand-write as a
/// page size, and within the channel store's own bound (`MAX_CHANNELS`), so the
/// whole collection fits in one page of the published size.
const STORED_CHANNELS: usize = 60;

/// How many resources suite 2 stores per operation: the same reasoning, within
/// every one of those stores' bounds.
const STORED_PER_OPERATION: usize = 55;

/// A list member of a response as a list, whatever its length. An empty list and
/// an absent member both mean "no entries" to a client, and which of the two the
/// restJson1 codec writes is the serializer's choice, so both read here as the
/// empty list.
fn items(case: &str, body: &Value, member: &str) -> Vec<Value> {
    match body.get(member) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(other) => panic!("case {case:?}: expected a {member} list, got {other}"),
    }
}

/// The `MaxResults` / `NextToken` query string a client sends (both members are
/// `@httpQuery`).
fn query(max_results: Option<usize>, next_token: Option<&str>) -> String {
    let mut query = String::new();
    for parameter in [
        max_results.map(|size| format!("maxResults={size}")),
        next_token.map(|token| format!("nextToken={token}")),
    ]
    .into_iter()
    .flatten()
    {
        query.push(if query.is_empty() { '?' } else { '&' });
        query.push_str(&parameter);
    }
    query
}

/// One page of the listing at `uri`: the items it reports in `member` and the
/// `NextToken` it hands out, with the call itself asserted to have answered 200.
async fn page(
    router: &MultiServiceRouter,
    case: &str,
    uri: &str,
    member: &str,
    query: &str,
) -> (Vec<Value>, Option<String>) {
    let (status, body) = call(router, "GET", &format!("{uri}{query}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GET {uri}{query} to answer 200, got {status}: {body}"
    );
    let token = body["nextToken"].as_str().map(ToString::to_string);
    let page = items(case, &body, member);
    if token.is_some() {
        assert!(
            !page.is_empty(),
            "case {case:?}: expected no nextToken beside an empty page, got {body}"
        );
    }
    (page, token)
}

/// The `id` member of a reported summary, as a client reads it.
fn id_of(case: &str, summary: &Value) -> String {
    summary["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {summary}"))
        .to_string()
}

/// A created resource, with the create asserted to have succeeded (the creates
/// below answer the 200 or 201 their own model binding declares).
async fn created(router: &MultiServiceRouter, case: &str, uri: &str, body: Value) -> Value {
    let (status, response) = call(router, "POST", uri, Some(body)).await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: expected POST {uri} to succeed, got {status}: {response}"
    );
    response
}

/// Rule: a `ListChannels` page carries as many channels as `MaxResults` asks for
/// while channels are left to report, bounded by the published `MaxResults`
/// maximum, which is also the size a request naming no `MaxResults` gets; a
/// `NextToken` comes back only when a channel is left over, and however the pages
/// were cut the walk reports every stored channel exactly once in mint order.
///
/// The rows differ along the asked-for page size relative to both what the store
/// holds and the published bound — none at all, the smallest, several sizes below
/// the stored count, the stored count exactly, one past it, the published bound
/// and a size past that bound — because that is what the rule generalises over.
#[tokio::test]
async fn a_page_carries_as_many_channels_as_the_request_asks_for() {
    assert_eq!(
        MAX_LIST_PAGE_SIZE, PUBLISHED_MAX_RESULTS,
        "expected the service's page ceiling to be the published MaxResults maximum \
         {PUBLISHED_MAX_RESULTS}, got {MAX_LIST_PAGE_SIZE}"
    );

    let table = [
        None,
        Some(1),
        Some(2),
        Some(7),
        Some(49),
        Some(50),
        Some(51),
        Some(STORED_CHANNELS - 1),
        Some(STORED_CHANNELS),
        Some(STORED_CHANNELS + 1),
        Some(PUBLISHED_MAX_RESULTS),
        Some(PUBLISHED_MAX_RESULTS + 500),
    ];

    let router = medialive();
    let mut minted: Vec<String> = Vec::new();
    for index in 0..STORED_CHANNELS {
        let response = created(
            &router,
            "the stored channels",
            CHANNELS,
            json!({
                "name": format!("channel-{index}"),
                "encoderSettings": crate::support::channel_encoder_settings(),
            }),
        )
        .await;
        minted.push(id_of("the stored channels", &response["channel"]));
    }

    for max_results in table {
        let case = format!("maxResults={max_results:?}");
        // The size the service may cut a page at: what the request asked for,
        // bounded by the published maximum.
        let cut_at = max_results
            .unwrap_or(PUBLISHED_MAX_RESULTS)
            .min(PUBLISHED_MAX_RESULTS);
        let expected_first = cut_at.min(STORED_CHANNELS);

        let (first, mut token) =
            page(&router, &case, CHANNELS, "channels", &query(max_results, None)).await;
        assert_eq!(
            first.len(),
            expected_first,
            "case {case:?}: expected a page of {expected_first} channels out of \
             {STORED_CHANNELS} stored, got {}",
            first.len()
        );
        assert_eq!(
            token.is_some(),
            expected_first < STORED_CHANNELS,
            "case {case:?}: expected a nextToken only with channels left over, got {token:?}"
        );

        let mut reported: Vec<String> = first.iter().map(|one| id_of(&case, one)).collect();
        while let Some(resume) = token.clone() {
            let (more, next) = page(
                &router,
                &case,
                CHANNELS,
                "channels",
                &query(max_results, Some(&resume)),
            )
            .await;
            assert!(
                more.len() <= cut_at,
                "case {case:?}: expected at most {cut_at} channels in a page, got {}",
                more.len()
            );
            reported.extend(more.iter().map(|one| id_of(&case, one)));
            token = next;
            assert!(
                reported.len() <= STORED_CHANNELS,
                "case {case:?}: expected the walk to terminate, got {} reported",
                reported.len()
            );
        }
        assert_eq!(
            reported, minted,
            "case {case:?}: expected the walk to report every stored channel once, in mint order"
        );
    }
}

/// One row of the cross-operation table: the collection a list operation reports,
/// the member it reports it in, and the create that fills it.
struct Listing {
    case: &'static str,
    uri: &'static str,
    member: &'static str,
    body: fn(usize) -> Value,
}

/// Rule: the page size is the SERVICE's, so every list operation of this service
/// answers a page as large as `MaxResults` asks for, up to the published maximum
/// a request naming no `MaxResults` also gets, and hands out a `NextToken` only
/// when an item is left over.
///
/// The rows differ along the OPERATION — three collections whose stores,
/// identifiers and summaries are unrelated — because the rule is the service's
/// and not one listing's, and each row asks for its collection's size exactly, a
/// size past the published bound, no size at all and one item short of the
/// collection, which is where the cut and the token are observable.
#[tokio::test]
async fn every_listing_of_the_service_honours_the_asked_for_page_size() {
    let table = [
        Listing {
            case: "ListChannels",
            uri: CHANNELS,
            member: "channels",
            body: |index| json!({
                "name": format!("paged-{index}"),
                "encoderSettings": crate::support::channel_encoder_settings(),
            }),
        },
        Listing {
            // `smithy_query(query="operation", operation="ListInputSecurityGroups")`:
            // GET /prod/inputSecurityGroups, reported in `inputSecurityGroups`.
            case: "ListInputSecurityGroups",
            uri: "/prod/inputSecurityGroups",
            member: "inputSecurityGroups",
            body: |index| json!({ "whitelistRules": [{ "cidr": format!("10.0.{index}.0/24") }] }),
        },
        Listing {
            // `smithy_query(query="operation", operation="ListMultiplexes")`:
            // GET /prod/multiplexes, reported in `multiplexes`.
            case: "ListMultiplexes",
            uri: "/prod/multiplexes",
            member: "multiplexes",
            body: |index| {
                create_body(
                    &format!("paged-{index}"),
                    &format!("token-{index}"),
                    settings(
                        TRANSPORT_STREAM_BITRATE.0,
                        TRANSPORT_STREAM_ID.0 + index as i64,
                    ),
                )
            },
        },
    ];

    for Listing {
        case,
        uri,
        member,
        body,
    } in table
    {
        let router = medialive();
        for index in 0..STORED_PER_OPERATION {
            created(&router, case, uri, body(index)).await;
        }

        // A size that admits the whole collection — asked for exactly, past the
        // published bound, or not asked for at all — reports it in ONE page.
        for asked_for in [
            Some(STORED_PER_OPERATION),
            Some(PUBLISHED_MAX_RESULTS),
            Some(PUBLISHED_MAX_RESULTS + 500),
            None,
        ] {
            let row = format!("{case} maxResults={asked_for:?}");
            let (whole, token) = page(&router, &row, uri, member, &query(asked_for, None)).await;
            assert_eq!(
                whole.len(),
                STORED_PER_OPERATION,
                "case {row:?}: expected one page to hold all {STORED_PER_OPERATION} items, got {}",
                whole.len()
            );
            assert!(
                token.is_none(),
                "case {row:?}: expected no nextToken beside a page holding every item, got \
                 {token:?}"
            );
        }

        // One item short of the collection: the page is cut exactly there and the
        // token resumes at the item left over.
        let short = STORED_PER_OPERATION - 1;
        let row = format!("{case} maxResults={short}");
        let (cut, token) = page(&router, &row, uri, member, &query(Some(short), None)).await;
        assert_eq!(
            cut.len(),
            short,
            "case {row:?}: expected a page of {short}, got {}",
            cut.len()
        );
        let resume = token.unwrap_or_else(|| {
            panic!("case {row:?}: expected a nextToken with one item left over")
        });
        let (rest, token) = page(
            &router,
            &row,
            uri,
            member,
            &query(Some(short), Some(&resume)),
        )
        .await;
        assert_eq!(
            rest.len(),
            1,
            "case {row:?}: expected the last page to carry the one item left over, got {}",
            rest.len()
        );
        assert!(
            token.is_none(),
            "case {row:?}: expected no nextToken on the last page, got {token:?}"
        );
    }
}

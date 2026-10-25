// SPDX-License-Identifier: CC0-1.0

//! The ONE page-size rule every paged list of this service resolves through, driven over
//! HTTP through the same router the emulator boots.
//!
//! The rule, asserted over a SET of requested sizes rather than one recorded request: a
//! paged list serves pages of exactly the `maxResults` the request asks for while items
//! remain, falls back to the page size the model declares as `MaxResults`'s default when
//! the member is absent, refuses a value outside the member's published range, and pages
//! the whole collection with no item repeated or skipped — identically for every list,
//! whichever collection it walks.
//!
//! The suite drives the rule through `ListChannelGroups` and through `ListHarvestJobs`:
//! the harvest-job list resolved its page size from its own copy of the bounds and the
//! default, so a row here holds both lists to the same resolved sizes.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, Emulator};
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};

/// The smallest `maxResults` the member's range admits
/// (`smithy_query(query="shape",shape="ListResourceMaxResults")` — `@range` min 1, max
/// 100), the largest it admits, and the page size the model declares as the member's
/// default.
const MAX_RESULTS_MIN: usize = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: usize = 100;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_DEFAULT: usize = 10;

/// How many items each collection under test holds: more than [`MAX_RESULTS_DEFAULT`], so
/// a request that omits `maxResults` is pinned to the modelled default rather than being
/// satisfied by "everything fits in one page".
const COLLECTION_SIZE: usize = 12;

/// The channel group the harvest jobs hang under. It is also one of the groups
/// `ListChannelGroups` reports, so the group table below counts it.
const HARVEST_GROUP: &str = "sps-harvest";

/// One paged list, as a request path plus the way that list spells one item's identity.
struct Listing {
    operation: &'static str,
    path: String,
    identity: fn(&Value) -> String,
}

/// `ListChannelGroups` reports a group by name.
fn group_identity(item: &Value) -> String {
    item["ChannelGroupName"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `ListHarvestJobs` reports a job by the triple it is ordered within its group by:
/// channel, then origin endpoint, then job name.
fn job_identity(item: &Value) -> String {
    format!(
        "{}/{}/{}",
        item["ChannelName"].as_str().unwrap_or_default(),
        item["OriginEndpointName"].as_str().unwrap_or_default(),
        item["HarvestJobName"].as_str().unwrap_or_default(),
    )
}

/// Build both collections: [`COLLECTION_SIZE`] channel groups (one of them
/// [`HARVEST_GROUP`]) and [`COLLECTION_SIZE`] harvest jobs spread over two channels and
/// two origin endpoints of that group. Returns the two listings and, for each, the item
/// identities in the order a list must report them.
async fn populate(emulator: &Emulator) -> Vec<(Listing, Vec<String>)> {
    let mut groups: Vec<String> = vec![HARVEST_GROUP.to_string()];
    for index in 0..COLLECTION_SIZE - 1 {
        groups.push(format!("sps-group-{index:02}"));
    }
    for name in &groups {
        let (status, body) = emulator.create_group(name, json!({})).await;
        assert_eq!(
            status, 200,
            "case \"setup\": CreateChannelGroup {name} expected 200, got {status} {body}"
        );
    }
    groups.sort();

    // Two channels, two endpoints each, three jobs each: twelve jobs whose order is the
    // composite one the list pages in, so a page boundary that fell inside a channel or an
    // endpoint would show up in the walk below.
    let channels = ["chan-1", "chan-2"];
    let endpoints = ["end-1", "end-2"];
    let mut jobs: Vec<String> = Vec::new();
    let start = epoch_seconds_from_now(60.0);
    for channel in channels {
        let (status, body) = emulator
            .create_channel(HARVEST_GROUP, channel, json!({}))
            .await;
        assert_eq!(
            status, 200,
            "case \"setup\": CreateChannel {channel} expected 200, got {status} {body}"
        );
        for endpoint in endpoints {
            let (status, body) = emulator
                .create_endpoint(
                    HARVEST_GROUP,
                    channel,
                    endpoint,
                    json!({"ContainerType": "TS", "HlsManifests": [{"ManifestName": "index"}]}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case \"setup\": CreateOriginEndpoint {endpoint} expected 200, got {status} {body}"
            );
            for index in 0..COLLECTION_SIZE / (channels.len() * endpoints.len()) {
                let name = format!("job-{index}");
                let mut request = harvest_body(start, start + 3600.0, &["index"]);
                request["HarvestJobName"] = json!(name);
                let (status, body) = emulator
                    .call(
                        "POST",
                        &format!(
                            "/channelGroup/{HARVEST_GROUP}/channel/{channel}\
                             /originEndpoint/{endpoint}/harvestJob"
                        ),
                        Some(request),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case \"setup\": CreateHarvestJob {name} expected 200, got {status} {body}"
                );
                jobs.push(format!("{channel}/{endpoint}/{name}"));
            }
        }
    }
    jobs.sort();

    assert_eq!(
        (groups.len(), jobs.len()),
        (COLLECTION_SIZE, COLLECTION_SIZE),
        "case \"setup\": both collections must hold {COLLECTION_SIZE} items"
    );
    vec![
        (
            Listing {
                operation: "ListChannelGroups",
                path: "/channelGroup".to_string(),
                identity: group_identity,
            },
            groups,
        ),
        (
            Listing {
                operation: "ListHarvestJobs",
                path: format!("/channelGroup/{HARVEST_GROUP}/harvestJob"),
                identity: job_identity,
            },
            jobs,
        ),
    ]
}

/// Walk `listing` to its end, asking for `requested` items a page, and return the item
/// identities page by page.
async fn pages(emulator: &Emulator, listing: &Listing, requested: Option<usize>) -> Vec<Vec<String>> {
    let operation = listing.operation;
    let mut collected: Vec<Vec<String>> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = requested {
            query.push(format!("maxResults={size}"));
        }
        if let Some(resume) = &token {
            query.push(format!("nextToken={resume}"));
        }
        let path = if query.is_empty() {
            listing.path.clone()
        } else {
            format!("{}?{}", listing.path, query.join("&"))
        };
        let (status, body) = emulator.call("GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {operation:?} maxResults={requested:?}: expected 200, got {status} {body}"
        );
        let items = body["Items"].as_array().cloned().unwrap_or_default();
        collected.push(items.iter().map(listing.identity).collect());
        match body["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            collected.len() <= COLLECTION_SIZE,
            "case {operation:?} maxResults={requested:?}: a token after every page of a \
             {COLLECTION_SIZE}-item collection never ends; pages so far: {collected:?}"
        );
    }
    collected
}

#[tokio::test]
async fn every_paged_list_resolves_one_page_size_rule() {
    let emulator = Emulator::new();
    let listings = populate(&emulator).await;

    // The requested sizes, differing in value and in how they divide the collection: each
    // published bound, a size that divides it exactly, one that leaves a short last page,
    // one equal to the collection, one past it, and the member left absent — which is the
    // row that pins the modelled default.
    let requested: &[Option<usize>] = &[
        None,
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(5),
        Some(MAX_RESULTS_DEFAULT),
        Some(COLLECTION_SIZE),
        Some(COLLECTION_SIZE + 1),
        Some(MAX_RESULTS_MAX),
    ];
    for (listing, expected) in &listings {
        let operation = listing.operation;
        for requested in requested {
            let size = requested.unwrap_or(MAX_RESULTS_DEFAULT);
            let served = pages(&emulator, listing, *requested).await;

            let lengths: Vec<usize> = served.iter().map(Vec::len).collect();
            let full = expected.len() / size;
            let mut wanted: Vec<usize> = vec![size; full];
            if expected.len() % size != 0 {
                wanted.push(expected.len() % size);
            }
            assert_eq!(
                lengths, wanted,
                "case {operation:?} maxResults={requested:?}: pages of {lengths:?} where \
                 {wanted:?} is the resolved page size {size} applied to {} items",
                expected.len()
            );

            let seen: Vec<String> = served.concat();
            assert_eq!(
                &seen, expected,
                "case {operation:?} maxResults={requested:?}: paging must report every item \
                 once, in order; got {seen:?}"
            );
        }
    }

    // A size outside the member's published range is refused, so no list resolves a page
    // size from one: each bound is accepted above, and one past each bound is a 400 here.
    for value in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        for (listing, _) in &listings {
            let operation = listing.operation;
            let (status, body) = emulator
                .call("GET", &format!("{}?maxResults={value}", listing.path), None)
                .await;
            assert_eq!(
                status, 400,
                "case {operation:?} maxResults={value}: a value outside the published range \
                 expected 400, got {status} {body}"
            );
            assert_eq!(
                error_type(&body),
                "ValidationException",
                "case {operation:?} maxResults={value}: expected ValidationException, got {body}"
            );
        }
    }
}

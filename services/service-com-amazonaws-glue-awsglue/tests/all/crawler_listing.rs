// SPDX-License-Identifier: CC0-1.0

//! `ListCrawlers`: the names of the crawlers this account holds.
//!
//! RULE: `ListCrawlers` "retrieves the names of all crawler resources in this Amazon Web
//! Services account, or the resources with the specified tag" — so it answers 200 and
//! reports the name of every crawler the store holds, from the same records
//! `CreateCrawler` wrote and `GetCrawler` reads back, in name order, one page at a time
//! with a `NextToken` that resumes the walk, narrowed to the crawlers carrying every pair
//! a `Tags` filter names, and it changes nothing a later caller sees.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The tables below differ along what the rule generalises over: how many crawlers are
//! stored (including none at all, which is the recorded case), the page size asked for
//! (each published bound of `MaxResults` and one past it), and which tags the filter names
//! (none, an empty map, one pair, a pair that must both match, a key or a value nothing
//! carries).

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateCrawlerInput;
use service_com_amazonaws_glue_awsglue::state::{ComAmazonawsGlueAwsglueState, CrawlerEntry};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, ok, ACCOUNT_ID, REGION};

/// The role every crawler here is created with.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-listing";

/// The largest page `ListCrawlers` can serve, from the response member's own bound:
/// `ListCrawlersResponse$CrawlerNames` targets `CrawlerNameList`, whose `length` is 0..100
/// (`smithy_query(query="shape",shape="CrawlerNameList")`).
const MAX_CRAWLER_NAMES: usize = 100;

/// Create one crawler, with the tags the row asks for.
async fn given_crawler(router: &MultiServiceRouter, name: &str, tags: Value) {
    let mut body = json!({
        "Name": name,
        "Role": ROLE,
        "Targets": { "S3Targets": [{ "Path": format!("s3://crawler-bucket/{name}/") }] },
    });
    if !tags.is_null() {
        body["Tags"] = tags;
    }
    ok(router, "CreateCrawler", &body).await;
}

/// The `CrawlerNames` of one answer, in the order the service returned them.
fn crawler_names(page: &Value) -> Vec<String> {
    page["CrawlerNames"]
        .as_array()
        .unwrap_or_else(|| panic!("ListCrawlers must report a CrawlerNames list, got {page}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("a crawler name is a string, got {name}"))
                .to_string()
        })
        .collect()
}

/// Walk every page `ListCrawlers` hands out, following its own token, and return the names
/// in the order they were served.
async fn list_all(
    router: &MultiServiceRouter,
    case: &str,
    page_size: Option<i32>,
    filter: &Value,
    ceiling: usize,
) -> Vec<String> {
    let mut listed: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if !filter.is_null() {
            request["Tags"] = filter.clone();
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "ListCrawlers", &request).await;
        let names = crawler_names(&page);
        let bound = page_size
            .map(|size| usize::try_from(size).unwrap_or(MAX_CRAWLER_NAMES))
            .unwrap_or(MAX_CRAWLER_NAMES)
            .min(MAX_CRAWLER_NAMES);
        assert!(
            names.len() <= bound,
            "case {case:?}: a page must not exceed {bound} names, got {names:?}"
        );
        listed.extend(names);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            listed.len() <= ceiling,
            "case {case:?}: the walk must terminate, got {listed:?} for {ceiling} crawlers"
        );
    }
    listed
}

// ---------------------------------------------------------------------------
// Every stored crawler is listed, in name order, one page at a time
// ---------------------------------------------------------------------------

/// The listing reports every stored crawler's name, in name order, over as many pages as
/// the page size needs — and each name it reports is one `GetCrawler` resolves.
///
/// The rows differ along how many crawlers are stored and the page size asked for. The
/// recorded reference case — an account that holds no crawler at all — is the first row:
/// an empty listing, not an error.
#[tokio::test]
async fn every_stored_crawler_is_listed_and_paged() {
    // (case, how many crawlers are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no crawlers at all", 0, None),
        ("one crawler, no page size", 1, None),
        // The published lower bound of `MaxResults` (`PageSize` range 1..1000).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page larger than the stored count", 3, Some(50)),
        // The published upper bound.
        ("the largest page the model accepts", 4, Some(1_000)),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        // Created out of name order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("crawler-{index:02}"))
            .collect();
        for name in expected.iter().rev() {
            given_crawler(&router, name, Value::Null).await;
        }

        let listed = list_all(&router, case, *page_size, &Value::Null, *stored).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected every stored crawler name in name order, got {listed:?}"
        );

        // A name the listing reports is a crawler `GetCrawler` resolves: one record, two
        // reads.
        for name in &listed {
            let read = ok(&router, "GetCrawler", &json!({ "Name": name })).await;
            assert_eq!(
                read["Crawler"]["Name"].as_str(),
                Some(name.as_str()),
                "case {case:?}: GetCrawler({name}) must report the crawler the listing named, \
                 got {read}"
            );
        }

        // A read does not change what a later caller sees: the same call answers the same
        // thing.
        let again = list_all(&router, case, *page_size, &Value::Null, *stored).await;
        assert_eq!(
            again, listed,
            "case {case:?}: the listing must be repeatable, got {again:?}"
        );
    }
}

/// A store holding more crawlers than one page can carry is walked in full, and no page
/// exceeds the modelled ceiling of `CrawlerNames` even when `MaxResults` asks for more.
///
/// Seeded directly rather than over the wire: filling the store would exercise the create
/// path, which has its own suite.
#[tokio::test]
async fn a_store_past_the_page_ceiling_is_walked_in_full() {
    // (case, how many crawlers are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("exactly one full page", MAX_CRAWLER_NAMES, None),
        ("one crawler past a full page", MAX_CRAWLER_NAMES + 1, None),
        (
            "a page size past what the response can carry",
            MAX_CRAWLER_NAMES * 2 + 3,
            Some(1_000),
        ),
    ];

    for (case, stored, page_size) in rows {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(engine.clone());
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("seeded-{index:04}"))
            .collect();
        {
            let state = engine.state::<ComAmazonawsGlueAwsglueState>();
            let mut store = state.lock();
            for name in &expected {
                let input = CreateCrawlerInput {
                    name: name.clone(),
                    role: ROLE.to_string(),
                    ..CreateCrawlerInput::default()
                };
                let entry =
                    CrawlerEntry::from_create_input(&input, ACCOUNT_ID, SystemTime::UNIX_EPOCH);
                store.crawlers.insert(name.clone(), entry);
            }
        }

        let listed = list_all(&router, case, *page_size, &Value::Null, *stored).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected all {stored} names in name order, got {} names",
            listed.len()
        );
    }
}

// ---------------------------------------------------------------------------
// A tag filter narrows the listing
// ---------------------------------------------------------------------------

/// "If you choose to use tags filtering, only resources with the tag are retrieved": a
/// crawler is reported only when it carries every pair the filter names.
///
/// The rows differ along the filter's size and along whether its keys and values are ones
/// the store holds.
#[tokio::test]
async fn a_tag_filter_narrows_the_listing_to_the_crawlers_carrying_every_pair() {
    let router = emulator();
    given_crawler(&router, "tagged-both", json!({"team": "data", "env": "prod"})).await;
    given_crawler(&router, "tagged-team", json!({"team": "data"})).await;
    given_crawler(&router, "tagged-other", json!({"team": "ops"})).await;
    given_crawler(&router, "untagged", Value::Null).await;

    let everything = &[
        "tagged-both",
        "tagged-other",
        "tagged-team",
        "untagged",
    ];
    // (case, the Tags filter, the names it must report)
    let rows: &[(&str, Value, &[&str])] = &[
        ("no filter: every crawler", Value::Null, everything),
        ("an empty filter narrows nothing", json!({}), everything),
        (
            "one pair",
            json!({"team": "data"}),
            &["tagged-both", "tagged-team"],
        ),
        (
            "a pair that must BOTH match",
            json!({"team": "data", "env": "prod"}),
            &["tagged-both"],
        ),
        ("a value nothing carries", json!({"team": "finance"}), &[]),
        ("a key nothing carries", json!({"owner": "data"}), &[]),
    ];

    for (case, filter, expected) in rows {
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        // Walked page by page at the smallest page the model accepts as well as in one
        // call, so the filter and the paging agree about which set is being walked.
        for page_size in [None, Some(1)] {
            let listed = list_all(&router, case, page_size, filter, 4).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: expected the crawlers carrying every pair of {filter}, got \
                 {listed:?}"
            );
        }
    }
}

/// The filter reads the tags the store holds AT THE TIME OF THE CALL: a tag written after
/// the create brings its crawler into the filtered listing, and removing it takes it out
/// again.
#[tokio::test]
async fn the_filter_follows_later_tag_writes() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    given_crawler(&router, "late-tagged", Value::Null).await;
    let filter = json!({"stage": "beta"});

    let before = list_all(&router, "before the tag write", None, &filter, 1).await;
    assert!(
        before.is_empty(),
        "case \"before the tag write\": an untagged crawler is not in the filtered listing, got \
         {before:?}"
    );

    let arn = format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:crawler/late-tagged");
    ok(
        &router,
        "TagResource",
        &json!({ "ResourceArn": arn, "TagsToAdd": { "stage": "beta" } }),
    )
    .await;
    let after = list_all(&router, "after the tag write", None, &filter, 1).await;
    assert_eq!(
        after,
        vec!["late-tagged".to_string()],
        "case \"after the tag write\": the tagged crawler is in the filtered listing, got \
         {after:?}"
    );

    // The tag leaves the store directly rather than through `UntagResource`, which is
    // still a scaffold in this crate; what is under test is that the listing reads the
    // tags on every call rather than caching its first answer.
    engine
        .state::<ComAmazonawsGlueAwsglueState>()
        .lock()
        .crawlers
        .get_mut("late-tagged")
        .expect("the crawler the create stored")
        .tags
        .remove("stage");
    let removed = list_all(&router, "after the tag is removed", None, &filter, 1).await;
    assert!(
        removed.is_empty(),
        "case \"after the tag is removed\": the crawler leaves the filtered listing, got \
         {removed:?}"
    );
}

// ---------------------------------------------------------------------------
// The modelled bounds
// ---------------------------------------------------------------------------

/// `MaxResults` is `PageSize`, range 1..1000: one past each published bound is refused by
/// the modelled constraint rather than served as some other page size.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    given_crawler(&router, "bounded-crawler", Value::Null).await;

    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 1_001)] {
        let (status, body) =
            call(&router, "ListCrawlers", &json!({"MaxResults": max_results})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
    }
}

/// A continuation token no name matches is not an error: the walk simply resumes past
/// every stored name, so the page is empty and final.
#[tokio::test]
async fn a_token_past_the_last_name_serves_an_empty_final_page() {
    let router = emulator();
    for name in ["alpha", "beta"] {
        given_crawler(&router, name, Value::Null).await;
    }

    // (case, the token sent)
    for (case, token) in [
        ("a token after every stored name", "zzzz"),
        ("a token naming the last stored name", "beta"),
    ] {
        let page = ok(&router, "ListCrawlers", &json!({ "NextToken": token })).await;
        assert!(
            crawler_names(&page).is_empty(),
            "case {case:?}: expected an empty page, got {page}"
        );
        assert!(
            page.get("NextToken").is_none(),
            "case {case:?}: an exhausted walk carries no token, got {page}"
        );
    }

    // A token BEFORE every stored name resumes at the first of them.
    let page = ok(&router, "ListCrawlers", &json!({ "NextToken": "aaaa" })).await;
    assert_eq!(
        crawler_names(&page),
        vec!["alpha".to_string(), "beta".to_string()],
        "case \"a token before every stored name\": expected the whole set, got {page}"
    );
}

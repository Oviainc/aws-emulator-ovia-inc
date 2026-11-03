// SPDX-License-Identifier: CC0-1.0

//! `GetCrawlerMetrics`: the metrics of the crawlers a request asks about.
//!
//! RULE: `GetCrawlerMetrics` "retrieves metrics about specified crawlers", so it answers 200
//! and reports one `CrawlerMetrics` per crawler the store holds that the request's
//! `CrawlerNameList` selects — every crawler in the account when the request selects by no
//! name — in name order, one page bounded by `MaxResults` at a time with a `NextToken` that
//! resumes the walk; each element is computed from the stored crawler the create wrote and
//! `GetCrawler` reads back, so a crawler with a crawl IN PROGRESS is the one whose run is
//! still being estimated, a crawler that has finished no crawl has no duration and no table
//! count to report, and a name no crawler is stored under selects nothing. The call changes
//! nothing a later caller sees.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The tables below differ along what the rule generalises over: which names the request
//! selects by (none at all, an empty list, one, several out of order, repeated, unknown,
//! mixed, another casing, a list at the modelled ceiling), how many crawlers are stored, the
//! page size asked for (each published bound of `MaxResults` and one past it), and the run
//! state the stored crawler is in.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateCrawlerInput;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, CrawlerEntry, CRAWLER_STATE_READY, CRAWLER_STATE_RUNNING,
    CRAWLER_STATE_STOPPING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, ok, ACCOUNT_ID};

/// The role every crawler here is created with.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-metrics";

/// The largest `CrawlerNameList` the model accepts, and the longest name one of its
/// elements may carry.
///
/// Source: `smithy_query(query="shape",shape="CrawlerMetrics,CrawlerMetricsList")` and
/// `GetCrawlerMetrics`'s own input — `CrawlerNameList` has `length 0..100` and its member
/// targets `NameString`, `length 1..255`.
const MAX_CRAWLER_NAMES: usize = 100;
const MAX_NAME_LENGTH: usize = 255;

/// The largest page `GetCrawlerMetrics` serves, from `MaxResults`' own target: `PageSize`,
/// range 1..1000 (`smithy_query(query="operation",operation="GetCrawlerMetrics")`). The
/// response's `CrawlerMetricsList` carries no length of its own.
const MAX_PAGE_SIZE: i32 = 1_000;

/// Create one crawler over the wire.
async fn given_crawler(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateCrawler",
        &json!({
            "Name": name,
            "Role": ROLE,
            "Targets": { "S3Targets": [{ "Path": format!("s3://crawler-bucket/{name}/") }] },
        }),
    )
    .await;
}

/// One `GetCrawlerMetrics` request body: the selection the row asks about, plus a page size
/// and a cursor when the row names them.
fn request(selection: &Value, page_size: Option<i32>, token: Option<&str>) -> Value {
    let mut body = json!({});
    if !selection.is_null() {
        body["CrawlerNameList"] = selection.clone();
    }
    if let Some(size) = page_size {
        body["MaxResults"] = json!(size);
    }
    if let Some(cursor) = token {
        body["NextToken"] = json!(cursor);
    }
    body
}

/// The `CrawlerMetricsList` of one answer, which must be present even when empty.
fn reported(body: &Value) -> Vec<Value> {
    body.get("CrawlerMetricsList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("GetCrawlerMetrics must report a CrawlerMetricsList, got {body}")
        })
        .clone()
}

/// The `CrawlerName` of every reported metric, in the order the service returned them.
fn metric_names(body: &Value) -> Vec<String> {
    reported(body)
        .iter()
        .map(|metric| {
            metric
                .get("CrawlerName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every metric names its crawler, got {metric}"))
                .to_string()
        })
        .collect()
}

/// A numeric member of one metric. A member AT its modelled `@default` of 0 is the same
/// value to a client whether the protocol puts it on the wire or leaves it out, so an
/// absent member reads as that default.
fn number(metric: &Value, member: &str) -> f64 {
    match metric.get(member) {
        None => 0.0,
        Some(value) => value
            .as_f64()
            .unwrap_or_else(|| panic!("{member} is a number, got {value}")),
    }
}

/// `StillEstimating`, whose modelled `@default` is `false`.
fn still_estimating(metric: &Value) -> bool {
    match metric.get("StillEstimating") {
        None => false,
        Some(value) => value
            .as_bool()
            .unwrap_or_else(|| panic!("StillEstimating is a boolean, got {value}")),
    }
}

/// The durations and the table counts of a crawler that has FINISHED no crawl: it has no
/// run to have taken any time, and its crawls have written no table into the catalog, so
/// each of these is zero. `TimeLeftSeconds` is among them because it is "the estimated time
/// left to complete a running crawl" and no estimate has been made.
fn assert_no_finished_crawl(case: &str, metric: &Value) {
    for member in [
        "TimeLeftSeconds",
        "LastRuntimeSeconds",
        "MedianRuntimeSeconds",
        "TablesCreated",
        "TablesUpdated",
        "TablesDeleted",
    ] {
        let actual = number(metric, member);
        assert_eq!(
            actual, 0.0,
            "case {case:?}: no crawl has run, so expected {member} 0, got {actual} in {metric}"
        );
    }
}

/// Walk every page the operation hands out for one selection, following its own token, and
/// return the crawler names in the order they were served.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    selection: &Value,
    page_size: Option<i32>,
    ceiling: usize,
) -> Vec<String> {
    let mut served: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = ok(
            router,
            "GetCrawlerMetrics",
            &request(selection, page_size, token.as_deref()),
        )
        .await;
        let names = metric_names(&page);
        let bound = page_size
            .map(|size| usize::try_from(size).unwrap_or(MAX_CRAWLER_NAMES))
            .unwrap_or(MAX_PAGE_SIZE as usize);
        assert!(
            names.len() <= bound,
            "case {case:?}: a page must not exceed {bound} metrics, got {names:?}"
        );
        served.extend(names);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            served.len() <= ceiling,
            "case {case:?}: the walk must terminate, got {served:?} for {ceiling} crawlers"
        );
    }
    served
}

// ---------------------------------------------------------------------------
// The name list SELECTS which crawlers are reported on
// ---------------------------------------------------------------------------

/// Every crawler the store holds that the request's names select is reported, in name order,
/// and nothing else is: a name no crawler is stored under selects nothing, and a request
/// that selects by no name asks about every crawler in the account.
///
/// The rows differ along the selection: absent, empty, one name, several out of order,
/// repeated, unknown, unknown mixed with stored, another casing, a legal name at the
/// modelled length ceiling, and a list at the modelled count ceiling. The recorded reference
/// case — one call naming the single crawler under test — is the "one stored name" row.
#[tokio::test]
async fn the_selected_crawlers_are_the_ones_reported() {
    let router = emulator();
    for stored in ["alpha", "beta", "gamma"] {
        given_crawler(&router, stored).await;
    }
    let everything = vec![
        "alpha".to_string(),
        "beta".to_string(),
        "gamma".to_string(),
    ];

    // A list at the modelled count ceiling, holding two stored names among unknown ones.
    let at_the_ceiling: Vec<String> = (0..MAX_CRAWLER_NAMES - 2)
        .map(|index| format!("absent-{index:03}"))
        .chain(["gamma".to_string(), "alpha".to_string()])
        .collect();

    // (case, the CrawlerNameList sent, the names that must be reported)
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("no CrawlerNameList member", Value::Null, everything.clone()),
        (
            "an empty name list selects by nothing",
            json!([]),
            everything.clone(),
        ),
        ("one stored name", json!(["beta"]), vec!["beta".to_string()]),
        (
            "several stored names, asked out of order",
            json!(["gamma", "alpha"]),
            vec!["alpha".to_string(), "gamma".to_string()],
        ),
        (
            "every stored name",
            json!(["alpha", "beta", "gamma"]),
            everything.clone(),
        ),
        ("only unknown names", json!(["absent", "missing"]), vec![]),
        (
            "unknown names beside a stored one",
            json!(["absent", "beta", "missing"]),
            vec!["beta".to_string()],
        ),
        (
            // A set of names: the same crawler is reported once however often it is named.
            "a repeated stored name",
            json!(["beta", "beta"]),
            vec!["beta".to_string()],
        ),
        (
            // A crawler name carries no documented case fold, so another casing names
            // another crawler — and the store holds none under it.
            "a stored name in another casing",
            json!(["ALPHA"]),
            vec![],
        ),
        (
            "a legal name at the modelled length ceiling",
            json!(["n".repeat(MAX_NAME_LENGTH)]),
            vec![],
        ),
        (
            "a name list at the modelled count ceiling",
            json!(at_the_ceiling),
            vec!["alpha".to_string(), "gamma".to_string()],
        ),
    ];

    for (case, selection, expected) in rows {
        let body = ok(&router, "GetCrawlerMetrics", &request(&selection, None, None)).await;
        let names = metric_names(&body);
        assert_eq!(
            names, expected,
            "case {case:?}: expected the metrics of {expected:?} in name order, got {names:?} in \
             {body}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {case:?}: one page holds the whole selection, so it carries no token, got {body}"
        );
        for metric in reported(&body) {
            assert!(
                !still_estimating(&metric),
                "case {case:?}: no crawl is in progress, so nothing is being estimated, got \
                 {metric}"
            );
            assert_no_finished_crawl(case, &metric);
        }

        // A read does not change what a later caller sees: the same call answers the same
        // thing, and every stored crawler is still the crawler it was.
        let again = ok(&router, "GetCrawlerMetrics", &request(&selection, None, None)).await;
        assert_eq!(
            again, body,
            "case {case:?}: GetCrawlerMetrics must be repeatable, got {again}"
        );
        for stored in &everything {
            let read = ok(&router, "GetCrawler", &json!({ "Name": stored })).await;
            assert_eq!(
                read["Crawler"]["State"].as_str(),
                Some(CRAWLER_STATE_READY),
                "case {case:?}: reading metrics must not move {stored}'s run state, got {read}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A crawl in progress is the run still being estimated
// ---------------------------------------------------------------------------

/// "True if the crawler is still estimating how long it will take to complete this run":
/// the crawlers with a crawl IN PROGRESS are exactly the ones reported as still being
/// estimated, and a crawler with no run in progress is not.
///
/// The rows are the modelled `CrawlerState` values (`READY | RUNNING | STOPPING`, from
/// `smithy_query(query="shape",shape="Crawler")`): `READY` is the one state with no run in
/// it, and the other two each have a crawl the service is working on. The crawler is put
/// into each state directly, because the operations that move it there are not implemented
/// yet; the rule under test is what the metrics report of a run in progress, not how the
/// run got there.
#[tokio::test]
async fn a_crawl_in_progress_is_the_run_still_being_estimated() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    given_crawler(&router, "crawling").await;
    given_crawler(&router, "idle").await;

    /// Put the stored crawler into `state`.
    fn set_state(engine: &Arc<engine::InMemoryEngine>, name: &str, state: &str) {
        let store = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = store.lock();
        let entry = store
            .crawlers
            .get_mut(name)
            .unwrap_or_else(|| panic!("the suite created {name}"));
        entry.state = state.to_string();
    }

    // (the run state the crawler is in, whether its run is still being estimated)
    for (state, expected) in [
        (CRAWLER_STATE_READY, false),
        (CRAWLER_STATE_RUNNING, true),
        (CRAWLER_STATE_STOPPING, true),
        // Back to the state with no run in it: the report follows the store on every call.
        (CRAWLER_STATE_READY, false),
    ] {
        set_state(&engine, "crawling", state);
        let before = ok(&router, "GetCrawler", &json!({ "Name": "crawling" })).await;

        let body = ok(
            &router,
            "GetCrawlerMetrics",
            &request(&json!(["crawling", "idle"]), None, None),
        )
        .await;
        let metrics = reported(&body);
        assert_eq!(
            metric_names(&body),
            vec!["crawling".to_string(), "idle".to_string()],
            "case state {state}: both named crawlers are reported, got {body}"
        );
        let crawling = &metrics[0];
        assert_eq!(
            still_estimating(crawling),
            expected,
            "case state {state}: expected StillEstimating {expected}, got {crawling}"
        );
        // The crawler sharing the call has no run in progress whatever state its sibling
        // is in: the report is per crawler, not per call.
        assert!(
            !still_estimating(&metrics[1]),
            "case state {state}: the idle crawler has no run to estimate, got {}",
            metrics[1]
        );
        for metric in &metrics {
            assert_no_finished_crawl(state, metric);
        }

        // Reading the metrics of a crawl in progress does not advance it: the stored
        // crawler — its run state, its definition `Version` and its `LastUpdated` — is
        // exactly what it was.
        let after = ok(&router, "GetCrawler", &json!({ "Name": "crawling" })).await;
        assert_eq!(
            after, before,
            "case state {state}: the read must leave the crawler untouched, expected {before}, \
             got {after}"
        );
    }
}

// ---------------------------------------------------------------------------
// The selection is served one bounded page at a time
// ---------------------------------------------------------------------------

/// The whole selection is served over as many pages as the page size needs, in name order,
/// with no name served twice and none left out.
///
/// The rows differ along how many crawlers are stored, how the request selects them (every
/// crawler, or a named subset) and the page size asked for, including both published bounds
/// of `MaxResults`.
#[tokio::test]
async fn the_whole_selection_is_walked_in_name_order() {
    // (case, how many crawlers are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no crawlers at all", 0, None),
        ("one crawler, no page size", 1, None),
        // The published lower bound of `MaxResults` (`PageSize`, range 1..1000).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the selection", 7, Some(3)),
        ("a page larger than the selection", 3, Some(50)),
        // The published upper bound.
        ("the largest page the model accepts", 4, Some(MAX_PAGE_SIZE)),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        // Created out of name order, so the service's own order is what is asserted.
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("crawler-{index:02}"))
            .collect();
        for name in expected.iter().rev() {
            given_crawler(&router, name).await;
        }

        let served = walk(&router, case, &Value::Null, *page_size, *stored).await;
        assert_eq!(
            served, expected,
            "case {case:?}: expected every stored crawler's metrics in name order, got {served:?}"
        );

        // The same walk over a NAMED subset covers exactly that subset: here every second
        // crawler, named in reverse so the order served is the service's and not the
        // request's.
        let subset: Vec<String> = expected.iter().step_by(2).cloned().collect();
        let mut asked: Vec<String> = subset.clone();
        asked.reverse();
        let served = walk(&router, case, &json!(asked), *page_size, *stored).await;
        assert_eq!(
            served, subset,
            "case {case:?}: expected the metrics of {subset:?} in name order, got {served:?}"
        );
    }
}

/// A store holding more crawlers than one page can carry is walked in full.
///
/// Seeded directly rather than over the wire: filling the store would exercise the create
/// path, which has its own suite.
#[tokio::test]
async fn a_store_past_the_page_size_is_walked_in_full() {
    // (case, how many crawlers are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("exactly one full page", 10, Some(10)),
        ("one crawler past a full page", 11, Some(10)),
        ("many pages", 25, Some(4)),
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

        let served = walk(&router, case, &Value::Null, *page_size, *stored).await;
        assert_eq!(
            served, expected,
            "case {case:?}: expected all {stored} crawlers' metrics in name order, got {} of them",
            served.len()
        );
    }
}

/// A continuation token no name matches is not an error: the walk simply resumes past every
/// selected name, so the page is empty and final.
#[tokio::test]
async fn a_token_past_the_last_name_serves_an_empty_final_page() {
    let router = emulator();
    for name in ["alpha", "beta"] {
        given_crawler(&router, name).await;
    }

    for (case, token) in [
        ("a token after every stored name", "zzzz"),
        ("a token naming the last stored name", "beta"),
    ] {
        let page = ok(
            &router,
            "GetCrawlerMetrics",
            &request(&Value::Null, None, Some(token)),
        )
        .await;
        assert!(
            metric_names(&page).is_empty(),
            "case {case:?}: expected an empty page, got {page}"
        );
        assert!(
            page.get("NextToken").is_none(),
            "case {case:?}: an exhausted walk carries no token, got {page}"
        );
    }

    // A token BEFORE every selected name resumes at the first of them.
    let page = ok(
        &router,
        "GetCrawlerMetrics",
        &request(&Value::Null, None, Some("aaaa")),
    )
    .await;
    assert_eq!(
        metric_names(&page),
        vec!["alpha".to_string(), "beta".to_string()],
        "case \"a token before every stored name\": expected the whole selection, got {page}"
    );
}

// ---------------------------------------------------------------------------
// The modelled bounds
// ---------------------------------------------------------------------------

/// A request outside the modelled bounds is refused at the request boundary rather than
/// served as some other page or selection: `MaxResults` is `PageSize`, range 1..1000, and
/// `CrawlerNameList` is `length 0..100` over names of `length 1..255`. Each published bound
/// is exercised one past it, and the bounds themselves are served above.
#[tokio::test]
async fn a_request_outside_the_modeled_bounds_is_refused() {
    let router = emulator();
    given_crawler(&router, "bounded-crawler").await;

    let too_many: Vec<String> = (0..=MAX_CRAWLER_NAMES)
        .map(|index| format!("absent-{index:03}"))
        .collect();
    // (case, the request body)
    let rows: Vec<(&str, Value)> = vec![
        ("a page size below the minimum", json!({ "MaxResults": 0 })),
        (
            "a page size above the maximum",
            json!({ "MaxResults": MAX_PAGE_SIZE + 1 }),
        ),
        (
            "one name too many",
            json!({ "CrawlerNameList": too_many.clone() }),
        ),
        ("an empty name", json!({ "CrawlerNameList": [""] })),
        (
            "a name one character too long",
            json!({ "CrawlerNameList": ["n".repeat(MAX_NAME_LENGTH + 1)] }),
        ),
        (
            "a stored name beside an over-long one",
            json!({
                "CrawlerNameList": ["bounded-crawler", "n".repeat(MAX_NAME_LENGTH + 1)],
            }),
        ),
    ];

    for (case, body) in rows {
        let (status, answer) = call(&router, "GetCrawlerMetrics", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            !crate::wire::error_code(&answer).is_empty(),
            "case {case:?}: the refusal must name an error, got {answer}"
        );
    }

    // The refusals changed nothing: the stored crawler is still readable, and its metrics
    // are still served.
    let body = ok(
        &router,
        "GetCrawlerMetrics",
        &request(&json!(["bounded-crawler"]), None, None),
    )
    .await;
    assert_eq!(
        metric_names(&body),
        vec!["bounded-crawler".to_string()],
        "case \"after the refusals\": expected the stored crawler's metrics, got {body}"
    );
}

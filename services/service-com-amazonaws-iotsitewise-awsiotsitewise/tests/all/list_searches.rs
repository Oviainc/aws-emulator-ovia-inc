// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListSearches`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a listing enumerates exactly the searches `StartSearch` recorded for the
//!   workspace the request names, and no search of another workspace; a workspace
//!   this service does not hold is the modelled not-found refusal rather than an
//!   empty page, while a held workspace with no search is an empty page;
//! * every summary reports the stored search — its statement, strategy, group and
//!   start instant — and the status the CLOCK has made due, the same values a read
//!   of that one search answers with, and a listing changes neither;
//! * the enumeration is most recently started first, so a listing is ordered by
//!   start instant descending whatever order the searches were recorded in;
//! * `maxResults`/`nextToken` page that one enumeration: the pages joined are the
//!   whole listing once, in one order, whatever page size the caller asks for;
//! * every `listSearchesFilters` member narrows the listing to the searches that
//!   match it, several of them to the searches that match ALL of them, and a value
//!   outside a modelled catalogue — or a time window whose end precedes its start —
//!   is refused rather than answered with an empty page.
//!
//! The HTTP harness (`call`, `error_code`, `router`, `router_with_engine`) is the
//! asset suite's and the `StartSearch` / `DescribeSearch` / `CreateWorkspace`
//! wrappers are the search suite's; this suite reuses both rather than growing a
//! third copy.
//!
//! Rows whose expectation depends on an EXACT start instant or an exact status are
//! planted on this service's state rather than started over HTTP: `StartSearch`
//! mints `startedAt` from the wall clock and owes its transitions a span of it, so a
//! started search carries no instant a filter bound can be written against.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    search_key, service_state, SearchRecord, SEARCH_QUEUED, SEARCH_RUNNING, SEARCH_SUCCEEDED,
};

use crate::asset_lifecycle::{call, error_code, router, router_with_engine};
use crate::search_lifecycle::{create_workspace, describe, start};

/// The workspace most rows below list the searches of.
const WORKSPACE: &str = "plant-one";

/// A second workspace holding searches of its own, so a listing can be shown to
/// report only the searches of the workspace it names.
const OTHER_WORKSPACE: &str = "plant_two";

/// A third workspace nothing starts a search in, so the empty page can be told
/// apart from the absent workspace.
const EMPTY_WORKSPACE: &str = "plant-three";

/// Every `SearchStatus` the model names. Source: `smithy_query(query="shape",
/// shape="SearchStatus")` — the enum's own value list.
const SEARCH_STATUSES: [&str; 4] = ["QUEUED", "RUNNING", "SUCCEEDED", "FAILED"];

/// Every `SearchType` the model names. Source: `smithy_query(query="shape",
/// shape="SearchType")`.
const SEARCH_TYPES: [&str; 2] = ["DEEP", "QUICK"];

/// `POST /workspaces/{workspaceName}/searches/list`, as `OPERATIONS.md` reports the
/// route of `ListSearches`.
fn list_uri(workspace: &str) -> String {
    format!("/workspaces/{workspace}/searches/list")
}

/// `ListSearches` on `workspace` with `body`.
async fn list_raw(router: &MultiServiceRouter, workspace: &str, body: Value) -> (u16, Value) {
    call(router, "POST", &list_uri(workspace), Some(body)).await
}

/// `ListSearches`, asserting it answered and that the required `searchSummaries`
/// member really is a list, and the body it answered with.
async fn listed(router: &MultiServiceRouter, workspace: &str, body: Value) -> Value {
    let case = body.to_string();
    let (status, answer) = list_raw(router, workspace, body).await;
    assert_eq!(
        status, 200,
        "case {case}: listing {workspace} must be served, got {status} {answer}"
    );
    assert!(
        answer["searchSummaries"].is_array(),
        "case {case}: searchSummaries is a required list, got {answer}"
    );
    answer
}

/// The `searchId`s a listing body reports, in the order it reported them.
fn ids(body: &Value) -> Vec<String> {
    body["searchSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a searchSummaries list, got {body}"))
        .iter()
        .map(|summary| {
            summary["searchId"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary carries a searchId, got {summary}"))
                .to_string()
        })
        .collect()
}

/// `StartSearch` with `body`, asserted accepted, and the `searchId` it minted.
async fn start_one(router: &MultiServiceRouter, workspace: &str, body: Value) -> String {
    let case = body.to_string();
    let (status, answer) = start(router, workspace, body).await;
    assert_eq!(
        status, 202,
        "case {case}: StartSearch in {workspace} must be accepted, got {status} {answer}"
    );
    answer["searchId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: StartSearch answers a searchId, got {answer}"))
        .to_string()
}

/// The starts the first suite records: rows differing along what a summary has to
/// carry — the strategy named or defaulted, a group or none, filters or none, and a
/// statement from one word to the long end of the modelled `SearchQueryStatement`.
fn start_bodies() -> Vec<Value> {
    vec![
        json!({ "queryStatement": "compressor" }),
        json!({ "queryStatement": "pump pressure anomalies", "searchType": "DEEP" }),
        json!({ "queryStatement": "boiler trend", "searchType": "QUICK", "groupId": "boilers" }),
        json!({
            "queryStatement": "turbine vibration",
            "groupId": "turbines-2",
            "searchFilters": { "timeIntervals": [{
                "startTime": { "timeInSeconds": 1, "offsetInNanos": 0 },
                "endTime": { "timeInSeconds": 2, "offsetInNanos": 0 },
            }] },
        }),
        json!({ "queryStatement": "x".repeat(5000) }),
    ]
}

/// One planted search: the instant it started, the status the clock is to have made
/// due for it, and the members the filters select on.
#[derive(Debug, Clone)]
struct Planted {
    label: &'static str,
    workspace: &'static str,
    /// Whole seconds since the Unix epoch, so `startedAt` is an exact value a
    /// `startedAfter` / `startedBefore` bound can be written against.
    started_seconds: u64,
    status: &'static str,
    search_type: &'static str,
    group_id: Option<&'static str>,
}

/// The instant whole second `seconds` of the Unix epoch names.
fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// A `searchId` of the planted search `index`, built to satisfy the modelled
/// `SearchId` constraints (len 23..36, pattern ^[a-zA-Z0-9]+(-[a-zA-Z0-9]+)*$ —
/// `smithy_query(query="shape", shape="SearchId")`).
fn planted_id(index: usize) -> String {
    format!("aaaaaaaa-0000-4000-8000-{index:012}")
}

/// Plant `case` on this service's state as the search `index`, and the `searchId` it
/// is stored under.
///
/// The record is the one `StartSearch` writes; only the instants are chosen, which
/// is what makes a planted row's `startedAt` and status exact. A status is planted
/// by the instants its transitions fall due at — a span still ahead for `QUEUED`,
/// the run still ahead for `RUNNING`, both behind for `SUCCEEDED` — never by storing
/// a status, since the handler derives it.
fn plant(engine: &Arc<engine::InMemoryEngine>, index: usize, case: &Planted) -> String {
    let search_id = planted_id(index);
    let started_at = at(case.started_seconds);
    let ahead = SystemTime::now() + Duration::from_secs(3600);
    let (running_at, succeeded_at) = match case.status {
        SEARCH_QUEUED => (ahead, ahead + Duration::from_secs(1)),
        SEARCH_RUNNING => (started_at, ahead),
        SEARCH_SUCCEEDED => (started_at, started_at + Duration::from_secs(1)),
        other => panic!("case {}: {other} is no status a search reaches", case.label),
    };
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    store.searches.insert(
        search_key(case.workspace, &search_id),
        SearchRecord {
            workspace_name: case.workspace.to_string(),
            search_id: search_id.clone(),
            query_statement: format!("statement of {}", case.label),
            search_type: case.search_type.to_string(),
            search_filters: None,
            group_id: case.group_id.map(str::to_string),
            started_at,
            running_at,
            succeeded_at,
        },
    );
    search_id
}

/// The planted table every filter and paging suite below reads.
///
/// The rows span each status a search reports and each strategy it runs under, a
/// group shared by two searches, a group held by one, a search in no group at all, a
/// search of ANOTHER workspace, two searches that started in the same instant and
/// start instants spread over a known range — so no filter can be satisfied by the
/// shape of one row.
fn planted_cases() -> Vec<Planted> {
    vec![
        Planted {
            label: "queued-quick-grouped",
            workspace: WORKSPACE,
            started_seconds: 1_700_000_100,
            status: SEARCH_QUEUED,
            search_type: "QUICK",
            group_id: Some("boilers"),
        },
        Planted {
            label: "running-deep-grouped",
            workspace: WORKSPACE,
            started_seconds: 1_700_000_200,
            status: SEARCH_RUNNING,
            search_type: "DEEP",
            group_id: Some("boilers"),
        },
        Planted {
            label: "succeeded-deep-other-group",
            workspace: WORKSPACE,
            started_seconds: 1_700_000_300,
            status: SEARCH_SUCCEEDED,
            search_type: "DEEP",
            group_id: Some("turbines-2"),
        },
        Planted {
            label: "succeeded-quick-ungrouped",
            workspace: WORKSPACE,
            started_seconds: 1_700_000_400,
            status: SEARCH_SUCCEEDED,
            search_type: "QUICK",
            group_id: None,
        },
        Planted {
            label: "queued-deep-ungrouped",
            workspace: WORKSPACE,
            started_seconds: 1_700_000_500,
            status: SEARCH_QUEUED,
            search_type: "DEEP",
            group_id: None,
        },
        Planted {
            label: "running-quick-same-instant",
            workspace: WORKSPACE,
            started_seconds: 1_700_000_500,
            status: SEARCH_RUNNING,
            search_type: "QUICK",
            group_id: Some("pumps"),
        },
        Planted {
            label: "succeeded-quick-elsewhere",
            workspace: OTHER_WORKSPACE,
            started_seconds: 1_700_000_600,
            status: SEARCH_SUCCEEDED,
            search_type: "QUICK",
            group_id: Some("boilers"),
        },
    ]
}

/// A router over a fresh engine carrying the three workspaces and the planted table,
/// with the id each row was planted under.
async fn planted_fixture() -> (MultiServiceRouter, Vec<(Planted, String)>) {
    let (router, engine) = router_with_engine();
    for workspace in [WORKSPACE, OTHER_WORKSPACE, EMPTY_WORKSPACE] {
        create_workspace(&router, workspace).await;
    }
    let planted = planted_cases()
        .into_iter()
        .enumerate()
        .map(|(index, case)| {
            let id = plant(&engine, index, &case);
            (case, id)
        })
        .collect();
    (router, planted)
}

/// The ids of the planted rows `keep` selects, as a set.
fn expected(planted: &[(Planted, String)], keep: impl Fn(&Planted) -> bool) -> BTreeSet<String> {
    planted
        .iter()
        .filter(|(case, _)| keep(case))
        .map(|(_, id)| id.clone())
        .collect()
}

/// A `nextToken` in the form this listing issues: the hex of the `searchId` the next
/// page resumes from.
///
/// Hand-rolled as an INDEPENDENT oracle: the handler encodes with the `hex` crate,
/// so re-using that crate here would let one encoding bug agree with itself.
fn token_for(search_id: &str) -> String {
    search_id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Every page of a listing of `workspace`, walked with `page_size` when given and
/// with no `maxResults` otherwise: the ids in order, and how many pages it took.
async fn walk(
    router: &MultiServiceRouter,
    workspace: &str,
    page_size: Option<i64>,
    expected_total: usize,
) -> (Vec<String>, usize) {
    let case = match page_size {
        Some(size) => format!("maxResults={size}"),
        None => "the default page size".to_string(),
    };
    let mut collected: Vec<String> = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    loop {
        let mut body = serde_json::Map::new();
        if let Some(size) = page_size {
            body.insert("maxResults".to_string(), json!(size));
        }
        if let Some(token) = &token {
            body.insert("nextToken".to_string(), json!(token));
        }
        let answer = listed(router, workspace, Value::Object(body)).await;
        let page = ids(&answer);
        pages += 1;
        if let Some(size) = page_size {
            assert!(
                page.len() as i64 <= size,
                "case {case:?}: page {pages} holds {} summaries, more than asked: {answer}",
                page.len()
            );
        }
        collected.extend(page);
        match answer["nextToken"].as_str() {
            Some(next) => {
                // A page followed by a token has to leave something to resume from.
                assert!(
                    collected.len() < expected_total,
                    "case {case:?}: page {pages} ends the listing yet carries {next}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
        assert!(
            pages <= expected_total + 2,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
    }
    (collected, pages)
}

#[tokio::test]
async fn a_listing_reports_the_searches_of_the_workspace_it_names() {
    let router = router();
    for workspace in [WORKSPACE, OTHER_WORKSPACE, EMPTY_WORKSPACE] {
        create_workspace(&router, workspace).await;
    }

    let mut here: Vec<String> = Vec::new();
    for body in start_bodies() {
        here.push(start_one(&router, WORKSPACE, body).await);
    }
    let mut there: Vec<String> = Vec::new();
    for body in start_bodies().into_iter().take(2) {
        there.push(start_one(&router, OTHER_WORKSPACE, body).await);
    }

    for (case, workspace, started) in [
        ("the workspace the searches were started in", WORKSPACE, &here),
        ("another workspace with searches of its own", OTHER_WORKSPACE, &there),
    ] {
        let body = listed(&router, workspace, json!({ "maxResults": 1000 })).await;
        let reported: BTreeSet<String> = ids(&body).into_iter().collect();
        let wanted: BTreeSet<String> = started.iter().cloned().collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: a listing reports exactly that workspace's searches, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: a listing that reported everything carries no nextToken, got {body}"
        );
    }

    // A held workspace nobody started a search in is an EMPTY page: absence of
    // searches is not absence of the workspace.
    let empty = listed(&router, EMPTY_WORKSPACE, json!({})).await;
    assert!(
        ids(&empty).is_empty() && empty["nextToken"].is_null(),
        "a workspace with no search answers an empty page, got {empty}"
    );

    // A workspace this service does not hold, in every form of a name no
    // `CreateWorkspace` minted — including ones differing from a held name only in
    // case, since a workspace is named exactly, and each published `WorkspaceName`
    // length bound. The workspace the PATH names is resolved before the filters are
    // read, so a request that is wrong in both ways still reports the absent
    // resource.
    for name in ["plant-nine", "PLANT-ONE", "plant-one-copy", "p", &"w".repeat(64)] {
        for body in [
            json!({}),
            json!({ "listSearchesFilters": { "statusFilter": ["QUEUED"] } }),
            json!({ "listSearchesFilters": { "startedAfter": 1_700_000_000_i64 } }),
            json!({ "maxResults": 1 }),
            json!({ "nextToken": "AAAA" }),
        ] {
            let case = body.to_string();
            let (status, answer) = list_raw(&router, name, body).await;
            assert_eq!(
                status, 404,
                "workspace {name:?} with {case}: expected 404, got {status} {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "workspace {name:?} with {case}: unexpected error code in {answer}"
            );
        }
    }
}

#[tokio::test]
async fn a_summary_reports_the_stored_search_and_the_status_the_clock_has_made_due() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;

    let mut started: Vec<String> = Vec::new();
    for body in start_bodies() {
        started.push(start_one(&router, WORKSPACE, body).await);
    }

    let body = listed(&router, WORKSPACE, json!({ "maxResults": 1000 })).await;
    for summary in body["searchSummaries"].as_array().expect("a summaries list") {
        let search_id = summary["searchId"].as_str().unwrap_or_default().to_string();
        assert!(
            SEARCH_STATUSES.contains(&summary["status"].as_str().unwrap_or_default()),
            "case {search_id}: a summary reports a modelled SearchStatus, got {summary}"
        );
        assert!(
            SEARCH_TYPES.contains(&summary["searchType"].as_str().unwrap_or_default()),
            "case {search_id}: a summary reports a modelled SearchType, got {summary}"
        );
        assert!(
            summary["startedAt"].as_f64().unwrap_or(-1.0) > 0.0,
            "case {search_id}: startedAt is the instant the search was started, got {summary}"
        );
        // No run of this emulator fails, so no search carries a status reason.
        assert!(
            summary["statusReason"].is_null(),
            "case {search_id}: a search that has not FAILED reports no statusReason, got {summary}"
        );

        // The same search, read on its own: a listing and a read of one stored
        // search cannot disagree about it. `status` is left out of the comparison
        // for the reason the sibling read suite gives — it is the one member the
        // clock may have moved between the two calls — and is checked above against
        // the modelled catalogue instead.
        let (status, read) = describe(&router, WORKSPACE, &search_id).await;
        assert_eq!(status, 200, "case {search_id}: describe got {status} {read}");
        for member in [
            "searchId",
            "workspaceName",
            "queryStatement",
            "searchType",
            "groupId",
            "startedAt",
            "statusReason",
        ] {
            assert_eq!(
                summary[member], read[member],
                "case {search_id}: the summary's {member} disagrees with the read: \
                 {summary} vs {read}"
            );
        }
    }

    // A listing REPORTS: two listings of one unchanged store agree, and neither
    // moves a search on.
    let again = listed(&router, WORKSPACE, json!({ "maxResults": 1000 })).await;
    assert_eq!(
        ids(&again),
        ids(&body),
        "two listings of one store must report the same searches in the same order"
    );
    assert_eq!(
        ids(&again).len(),
        started.len(),
        "every started search is listed exactly once, got {again}"
    );
}

#[tokio::test]
async fn a_listing_is_ordered_most_recently_started_first() {
    let (router, planted) = planted_fixture().await;

    let body = listed(&router, WORKSPACE, json!({ "maxResults": 1000 })).await;
    let summaries = body["searchSummaries"]
        .as_array()
        .expect("a summaries list")
        .clone();

    // Every planted search of this workspace, and nothing of another's.
    let reported: BTreeSet<String> = ids(&body).into_iter().collect();
    assert_eq!(
        reported,
        expected(&planted, |case| case.workspace == WORKSPACE),
        "a listing reports exactly the planted searches of {WORKSPACE}, got {body}"
    );

    // "Most recently started first": the start instants a listing reports never
    // increase from one summary to the next, whatever order the searches were
    // recorded in.
    let instants: Vec<f64> = summaries
        .iter()
        .map(|summary| summary["startedAt"].as_f64().unwrap_or(-1.0))
        .collect();
    for pair in instants.windows(2) {
        assert!(
            pair[0] >= pair[1],
            "a listing is ordered most recently started first, got {instants:?}"
        );
    }
    let mut descending = instants.clone();
    descending.sort_by(|left, right| right.partial_cmp(left).expect("a comparable instant"));
    assert_eq!(
        instants, descending,
        "a listing reports the start instants in descending order, got {body}"
    );

    // Two searches started in the SAME instant still order deterministically, so
    // two listings of one store report one order.
    let again = listed(&router, WORKSPACE, json!({ "maxResults": 1000 })).await;
    assert_eq!(
        ids(&again),
        ids(&body),
        "two listings of one store report one order, got {again} and {body}"
    );
}

#[tokio::test]
async fn pagination_walks_one_stable_enumeration_whatever_the_page_size() {
    let (router, planted) = planted_fixture().await;
    let total = expected(&planted, |case| case.workspace == WORKSPACE).len();

    // The whole listing in one page is the order every paged walk has to reproduce.
    let baseline = ids(&listed(&router, WORKSPACE, json!({ "maxResults": 1000 })).await);
    assert_eq!(
        baseline.len(),
        total,
        "the unpaged listing holds every planted search of {WORKSPACE}"
    );

    // Rows: a page smaller than the listing, sizes that divide it and sizes that do
    // not, exactly the listing, more than it, the modelled maximum, and no
    // `maxResults` at all.
    let cases: [Option<i64>; 7] = [
        Some(1),
        Some(2),
        Some(3),
        Some(total as i64),
        Some(total as i64 + 1),
        Some(1000),
        None,
    ];
    for page_size in cases {
        let case = format!("maxResults={page_size:?}");
        let (walked, pages) = walk(&router, WORKSPACE, page_size, total).await;
        assert_eq!(
            walked, baseline,
            "case {case:?}: the pages joined must be the whole listing once, in one order, \
             got {walked:?}"
        );
        let expected_pages = match page_size {
            Some(size) => total.div_ceil(size as usize),
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, walked {pages}"
        );
    }

    // The token a page carries names the search the NEXT page starts at, in the form
    // this listing issues.
    let first = listed(&router, WORKSPACE, json!({ "maxResults": 1 })).await;
    assert_eq!(
        first["nextToken"].as_str(),
        Some(token_for(&baseline[1]).as_str()),
        "the first page's token resumes at the second search, got {first}"
    );

    // A token this listing never issued is refused rather than answered with an
    // empty page: unhexable text, hex of an id no search carries, and hex of the id
    // of a search of ANOTHER workspace.
    let elsewhere = expected(&planted, |case| case.workspace == OTHER_WORKSPACE);
    let elsewhere = elsewhere.iter().next().expect("a search elsewhere").clone();
    for (case, token) in [
        // Inside the `^[A-Za-z0-9+/=]+$` form the member models, so the refusal is
        // the handler's rather than the modelled pattern's.
        ("not a token this service issues", "ZZZZ".to_string()),
        ("hex this service never encoded a token from", "AAAA".to_string()),
        ("hex of an unknown id", token_for(&planted_id(99))),
        ("hex of another workspace's search", token_for(&elsewhere)),
    ] {
        let (status, body) = list_raw(&router, WORKSPACE, json!({ "nextToken": token })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {token}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // `maxResults` carries @range 1..1000 (`smithy_query(query="operation",
    // operation="ListSearches")`): each published bound is served and one past it on
    // either side is refused.
    for (case, max_results, served) in [
        ("the range minimum", 1_i64, true),
        ("the range maximum", 1000, true),
        ("one under the minimum", 0, false),
        ("one past the maximum", 1001, false),
    ] {
        let (status, body) =
            list_raw(&router, WORKSPACE, json!({ "maxResults": max_results })).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?} (maxResults={max_results}): an in-range page size must be served, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} (maxResults={max_results}): an out-of-range page size must be \
                 refused, got {status} {body}"
            );
        }
    }
}

#[tokio::test]
async fn filters_narrow_the_listing_to_the_searches_that_match_all_of_them() {
    let (router, planted) = planted_fixture().await;

    // Rows: each status on its own and two together; each strategy and both; a group
    // two searches share, a group one holds, and two groups at once; each end of the
    // start window, inclusive, and a window that holds a middle slice; and filters
    // combined, which a search has to match ALL of.
    let cases: Vec<(&str, Value, BTreeSet<String>)> = vec![
        (
            "no filters at all",
            json!({}),
            expected(&planted, |case| case.workspace == WORKSPACE),
        ),
        (
            "statusFilter QUEUED",
            json!({ "listSearchesFilters": { "statusFilter": ["QUEUED"] } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE && case.status == SEARCH_QUEUED
            }),
        ),
        (
            "statusFilter RUNNING",
            json!({ "listSearchesFilters": { "statusFilter": ["RUNNING"] } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE && case.status == SEARCH_RUNNING
            }),
        ),
        (
            "statusFilter SUCCEEDED",
            json!({ "listSearchesFilters": { "statusFilter": ["SUCCEEDED"] } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE && case.status == SEARCH_SUCCEEDED
            }),
        ),
        (
            "statusFilter FAILED, a status nothing here reaches",
            json!({ "listSearchesFilters": { "statusFilter": ["FAILED"] } }),
            BTreeSet::new(),
        ),
        (
            "statusFilter QUEUED and SUCCEEDED",
            json!({ "listSearchesFilters": { "statusFilter": ["QUEUED", "SUCCEEDED"] } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE
                    && (case.status == SEARCH_QUEUED || case.status == SEARCH_SUCCEEDED)
            }),
        ),
        (
            "searchTypeFilter DEEP",
            json!({ "listSearchesFilters": { "searchTypeFilter": ["DEEP"] } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE && case.search_type == "DEEP"
            }),
        ),
        (
            "searchTypeFilter QUICK",
            json!({ "listSearchesFilters": { "searchTypeFilter": ["QUICK"] } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE && case.search_type == "QUICK"
            }),
        ),
        (
            "searchTypeFilter both strategies",
            json!({ "listSearchesFilters": { "searchTypeFilter": ["DEEP", "QUICK"] } }),
            expected(&planted, |case| case.workspace == WORKSPACE),
        ),
        (
            "groupIdFilter a group two searches share",
            json!({ "listSearchesFilters": { "groupIdFilter": ["boilers"] } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE && case.group_id == Some("boilers")
            }),
        ),
        (
            "groupIdFilter a group one search holds",
            json!({ "listSearchesFilters": { "groupIdFilter": ["pumps"] } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE && case.group_id == Some("pumps")
            }),
        ),
        (
            "groupIdFilter two groups at once",
            json!({ "listSearchesFilters": { "groupIdFilter": ["boilers", "turbines-2"] } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE
                    && matches!(case.group_id, Some("boilers") | Some("turbines-2"))
            }),
        ),
        (
            "groupIdFilter a group no search was filed under",
            json!({ "listSearchesFilters": { "groupIdFilter": ["chillers"] } }),
            BTreeSet::new(),
        ),
        (
            "startedAfter the earliest start, inclusive",
            json!({ "listSearchesFilters": { "startedAfter": 1_700_000_100_i64 } }),
            expected(&planted, |case| case.workspace == WORKSPACE),
        ),
        (
            "startedAfter one second past the earliest start",
            json!({ "listSearchesFilters": { "startedAfter": 1_700_000_101_i64 } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE && case.started_seconds > 1_700_000_100
            }),
        ),
        (
            "startedBefore the latest start, inclusive",
            json!({ "listSearchesFilters": { "startedBefore": 1_700_000_500_i64 } }),
            expected(&planted, |case| case.workspace == WORKSPACE),
        ),
        (
            "startedBefore one second under the latest start",
            json!({ "listSearchesFilters": { "startedBefore": 1_700_000_499_i64 } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE && case.started_seconds < 1_700_000_500
            }),
        ),
        (
            "a window of one instant, both bounds on it",
            json!({ "listSearchesFilters": {
                "startedAfter": 1_700_000_300_i64,
                "startedBefore": 1_700_000_300_i64,
            } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE && case.started_seconds == 1_700_000_300
            }),
        ),
        (
            "a window holding a middle slice",
            json!({ "listSearchesFilters": {
                "startedAfter": 1_700_000_200_i64,
                "startedBefore": 1_700_000_400_i64,
            } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE
                    && (1_700_000_200..=1_700_000_400).contains(&case.started_seconds)
            }),
        ),
        (
            "a window before every start",
            json!({ "listSearchesFilters": { "startedBefore": 1_600_000_000_i64 } }),
            BTreeSet::new(),
        ),
        (
            "every filter at once, which a search must match all of",
            json!({ "listSearchesFilters": {
                "statusFilter": ["QUEUED", "RUNNING", "SUCCEEDED"],
                "searchTypeFilter": ["DEEP"],
                "groupIdFilter": ["boilers", "turbines-2"],
                "startedAfter": 1_700_000_200_i64,
                "startedBefore": 1_700_000_300_i64,
            } }),
            expected(&planted, |case| {
                case.workspace == WORKSPACE
                    && case.search_type == "DEEP"
                    && matches!(case.group_id, Some("boilers") | Some("turbines-2"))
                    && (1_700_000_200..=1_700_000_300).contains(&case.started_seconds)
            }),
        ),
        (
            "filters that no one search satisfies together",
            json!({ "listSearchesFilters": {
                "statusFilter": ["QUEUED"],
                "groupIdFilter": ["pumps"],
            } }),
            BTreeSet::new(),
        ),
    ];

    for (case, body, wanted) in cases {
        let answer = listed(&router, WORKSPACE, body.clone()).await;
        let reported: BTreeSet<String> = ids(&answer).into_iter().collect();
        assert_eq!(
            reported, wanted,
            "case {case:?} ({body}): expected {} searches, got {answer}",
            wanted.len()
        );
        assert!(
            answer["nextToken"].is_null(),
            "case {case:?}: a filtered listing that reported everything carries no nextToken, \
             got {answer}"
        );
        // A filtered page never disagrees with the summaries in it.
        for summary in answer["searchSummaries"].as_array().expect("a list") {
            assert_eq!(
                summary["workspaceName"].as_str(),
                Some(WORKSPACE),
                "case {case:?}: a page reports only this workspace's searches, got {summary}"
            );
        }
    }

    // A value outside a modelled catalogue, and a window whose end precedes its
    // start, name no search the way a filter can be satisfied: each is the bad
    // request it is rather than an empty page. The catalogue rows are refused by the
    // modelled enum constraint before the handler runs, the window row by the
    // handler, so the rows assert the status and not which layer answered.
    for (case, body) in [
        (
            "a status in the wrong case",
            json!({ "listSearchesFilters": { "statusFilter": ["queued"] } }),
        ),
        (
            "a near-miss status",
            json!({ "listSearchesFilters": { "statusFilter": ["RUNNIN"] } }),
        ),
        (
            "a status of another API",
            json!({ "listSearchesFilters": { "statusFilter": ["COMPLETED"] } }),
        ),
        (
            "one bad status beside a good one",
            json!({ "listSearchesFilters": { "statusFilter": ["QUEUED", "ALL"] } }),
        ),
        (
            "a strategy in the wrong case",
            json!({ "listSearchesFilters": { "searchTypeFilter": ["deep"] } }),
        ),
        (
            "a strategy that is no strategy",
            json!({ "listSearchesFilters": { "searchTypeFilter": ["FAST"] } }),
        ),
        (
            "a window whose end precedes its start",
            json!({ "listSearchesFilters": {
                "startedAfter": 1_700_000_500_i64,
                "startedBefore": 1_700_000_100_i64,
            } }),
        ),
    ] {
        let (status, answer) = list_raw(&router, WORKSPACE, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({body}): expected 400, got {status} {answer}"
        );
    }
}

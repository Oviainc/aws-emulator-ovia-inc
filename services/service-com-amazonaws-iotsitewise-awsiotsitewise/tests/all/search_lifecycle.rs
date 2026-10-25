// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise workspace SEARCH slice
//! (`StartSearch` -> `DescribeSearch`), driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a started search is PERSISTED and reads back exactly as it was started — the
//!   query statement, the search strategy (`QUICK` when the request named none),
//!   the group it was filed under and the filters it carried — under a freshly
//!   minted `searchId` that satisfies the modelled `SearchId` constraints;
//! * `StartSearch` is idempotent on `clientToken`: a repeat of a call carrying one
//!   answers from the search that token already started instead of starting a
//!   second, so the stored search is the FIRST call's;
//! * a search is addressed by `(workspaceName, searchId)`, so it is readable only
//!   through the workspace it was started in and a pair no search is stored under
//!   is a modelled not-found refusal, as is a workspace the store does not hold;
//! * the lifecycle is a transition the CLOCK owes: the call answers `QUEUED`, the
//!   search reaches `RUNNING` and then the terminal `SUCCEEDED`, and polling it
//!   never moves it along;
//! * a `timeIntervals` filter entry whose end precedes its start is refused, and a
//!   refused call starts no search.
//!
//! The HTTP harness (`call`, `router`, `error_code`) is the one the asset suite
//! already has; this suite reuses it rather than growing a second.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, DEFAULT_SEARCH_TYPE, MAX_SEARCHES, MAX_SEARCHES_PER_WORKSPACE,
    SEARCH_QUEUED_FOR, SEARCH_RUNS_FOR,
};

use crate::asset_lifecycle::{call, error_code, router, router_with_engine};

/// The workspace every suite below works in unless it is about workspace scoping.
const WORKSPACE: &str = "plant-floor";

/// `POST /workspaces/{workspaceName}/searches`, as `OPERATIONS.md` reports the
/// route.
fn searches_uri(workspace: &str) -> String {
    format!("/workspaces/{workspace}/searches")
}

/// `GET /workspaces/{workspaceName}/searches/{searchId}`.
fn search_uri(workspace: &str, search_id: &str) -> String {
    format!("/workspaces/{workspace}/searches/{search_id}")
}

/// `StartSearch` with `body`.
async fn start(router: &MultiServiceRouter, workspace: &str, body: Value) -> (u16, Value) {
    call(router, "POST", &searches_uri(workspace), Some(body)).await
}

/// `DescribeSearch`.
async fn describe(router: &MultiServiceRouter, workspace: &str, search_id: &str) -> (u16, Value) {
    call(router, "GET", &search_uri(workspace, search_id), None).await
}

/// `CreateWorkspace` with the service's default encryption, asserted accepted:
/// a search runs against a workspace's data, so every suite below creates the
/// workspaces it searches first.
async fn create_workspace(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "workspaceName": name,
            "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateWorkspace {name}: got {status} {body}"
    );
}

/// A `clientToken` of `length` characters: the modelled `ClientToken` is
/// `len 36..64, pattern ^\S{36,64}$` (smithy_query query="operation"
/// operation="StartSearch"), so a token is padded to the length under test with
/// non-space characters.
fn token(seed: &str, length: usize) -> String {
    let mut value = seed.to_string();
    while value.len() < length {
        value.push('x');
    }
    value.truncate(length);
    value
}

/// `true` when `id` satisfies every constraint the model puts on a `SearchId`:
/// `len 23..36, pattern ^[a-zA-Z0-9]+(-[a-zA-Z0-9]+)*$` (smithy_query
/// query="shape" shape="SearchId").
///
/// Hand-rolled as an INDEPENDENT oracle rather than compared against whatever the
/// handler minted, so an id of the wrong shape cannot agree with itself and pass.
fn is_modelled_search_id(id: &str) -> bool {
    (23..=36).contains(&id.len())
        && id.split('-').all(|segment| {
            !segment.is_empty() && segment.chars().all(|character| character.is_ascii_alphanumeric())
        })
}

/// A `TimeInNanos` body: `timeInSeconds` (range 1..) plus its nanosecond offset.
fn at(seconds: i64, offset: i64) -> Value {
    json!({ "timeInSeconds": seconds, "offsetInNanos": offset })
}

/// One row of the start-and-read-back table: the request a `StartSearch` sends.
#[derive(Debug, Clone)]
struct StartCase {
    label: &'static str,
    workspace: &'static str,
    query_statement: String,
    search_type: Option<&'static str>,
    group_id: Option<&'static str>,
    filters: Option<Value>,
}

impl StartCase {
    /// The request body this case sends: only the members it actually carries, so
    /// a read-back comparison is against exactly those.
    fn body(&self) -> Value {
        let mut value = json!({ "queryStatement": self.query_statement });
        let object = value.as_object_mut().expect("an object body");
        if let Some(search_type) = self.search_type {
            object.insert("searchType".to_string(), json!(search_type));
        }
        if let Some(group_id) = self.group_id {
            object.insert("groupId".to_string(), json!(group_id));
        }
        if let Some(filters) = self.filters.clone() {
            object.insert("searchFilters".to_string(), filters);
        }
        value
    }

    /// The `searchType` this search must run under: the one the request named, or
    /// the documented default when it named none.
    fn expected_search_type(&self) -> &str {
        self.search_type.unwrap_or(DEFAULT_SEARCH_TYPE)
    }
}

/// A time-series id of the modelled length (`TimeSeriesId`, len 36..73).
const TIME_SERIES_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A dataset id of the modelled form (`DatasetId`, a lowercase-hex UUID).
const DATASET_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The rows: the request spanning what `StartSearch` takes — `searchType` absent
/// and both values the `SearchType` enum names (smithy_query query="shape"
/// shape="SearchType": `DEEP`, `QUICK`), `groupId` absent and at each published
/// `GroupId` length bound (2 and 36), `searchFilters` absent and each member of
/// the `SearchFilters` shape, and `queryStatement` at both published
/// `SearchQueryStatement` length bounds (1 and 5000) as well as ordinary prose.
fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase {
            label: "bare-minimum",
            workspace: WORKSPACE,
            query_statement: "turbine vibration last week".to_string(),
            search_type: None,
            group_id: None,
            filters: None,
        },
        StartCase {
            label: "shortest-query-statement",
            workspace: WORKSPACE,
            // The published `SearchQueryStatement` minimum length.
            query_statement: "a".to_string(),
            search_type: Some("QUICK"),
            group_id: None,
            filters: None,
        },
        StartCase {
            label: "longest-query-statement",
            workspace: WORKSPACE,
            // The published `SearchQueryStatement` maximum length.
            query_statement: "q".repeat(5000),
            search_type: Some("DEEP"),
            group_id: None,
            filters: None,
        },
        StartCase {
            label: "deep-with-shortest-group",
            workspace: WORKSPACE,
            query_statement: "pump pressure anomalies".to_string(),
            search_type: Some("DEEP"),
            // The published `GroupId` minimum length.
            group_id: Some("ab"),
            filters: None,
        },
        StartCase {
            label: "quick-with-longest-group",
            workspace: WORKSPACE,
            query_statement: "pump pressure anomalies".to_string(),
            search_type: Some("QUICK"),
            // The published `GroupId` maximum length.
            group_id: Some("g23456789-123456789-123456789-12345"),
            filters: None,
        },
        StartCase {
            label: "time-series-filter",
            workspace: WORKSPACE,
            query_statement: "readings for one stream".to_string(),
            search_type: None,
            group_id: Some("stream-group"),
            filters: Some(json!({ "timeSeriesIds": [TIME_SERIES_ID] })),
        },
        StartCase {
            label: "dataset-filter",
            workspace: WORKSPACE,
            query_statement: "documents in one dataset".to_string(),
            search_type: Some("DEEP"),
            group_id: None,
            filters: Some(json!({ "datasetIds": [DATASET_ID] })),
        },
        StartCase {
            label: "interval-filter",
            workspace: WORKSPACE,
            query_statement: "anything in this window".to_string(),
            search_type: None,
            group_id: None,
            filters: Some(json!({
                "timeIntervals": [
                    { "startTime": at(1_700_000_000, 0), "endTime": at(1_700_003_600, 0) },
                    { "startTime": at(1_700_010_000, 1), "endTime": at(1_700_010_000, 2) },
                ]
            })),
        },
        StartCase {
            label: "every-filter-at-once",
            workspace: "another-workspace_1",
            query_statement: "everything, narrowed".to_string(),
            search_type: Some("QUICK"),
            group_id: Some("all-filters"),
            filters: Some(json!({
                "timeSeriesIds": [TIME_SERIES_ID],
                "datasetIds": [DATASET_ID],
                "timeIntervals": [
                    { "startTime": at(1, 0), "endTime": at(9_223_372_036_854_774i64, 999_999_999) },
                ]
            })),
        },
    ]
}

#[tokio::test]
async fn a_started_search_is_persisted_and_reads_back() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;
    create_workspace(&router, "another-workspace_1").await;
    let mut minted: HashSet<String> = HashSet::new();

    for case in start_cases() {
        let (status, started) = start(&router, case.workspace, case.body()).await;
        assert_eq!(
            status, 202,
            "case {}: StartSearch must accept the request, got {status} {started}",
            case.label
        );

        let search_id = started["searchId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_search_id(&search_id),
            "case {}: searchId must satisfy the modelled SearchId constraints, got {started}",
            case.label
        );
        assert!(
            minted.insert(search_id.clone()),
            "case {}: every start must mint a fresh searchId, but {search_id} repeated",
            case.label
        );
        assert_eq!(
            started["status"].as_str().unwrap_or(""),
            "QUEUED",
            "case {}: a newly started search is QUEUED, got {started}",
            case.label
        );
        assert_eq!(
            started["workspaceName"].as_str().unwrap_or(""),
            case.workspace,
            "case {}: the response names the workspace the search runs against, got {started}",
            case.label
        );
        assert_eq!(
            started.get("groupId").and_then(Value::as_str),
            case.group_id,
            "case {}: the response carries the groupId the request supplied, got {started}",
            case.label
        );

        let (status, described) = describe(&router, case.workspace, &search_id).await;
        assert_eq!(
            status, 200,
            "case {}: the started search must read back, got {status} {described}",
            case.label
        );
        assert_eq!(
            described["searchId"].as_str().unwrap_or(""),
            search_id,
            "case {}: the read answers for the search that was started, got {described}",
            case.label
        );
        assert_eq!(
            described["workspaceName"].as_str().unwrap_or(""),
            case.workspace,
            "case {}: the read names the search's workspace, got {described}",
            case.label
        );
        assert_eq!(
            described["queryStatement"].as_str().unwrap_or(""),
            case.query_statement,
            "case {}: the read answers the queryStatement the search was started from, got {}",
            case.label,
            described["queryStatement"]
        );
        assert_eq!(
            described["searchType"].as_str().unwrap_or(""),
            case.expected_search_type(),
            "case {}: a search runs under the searchType it was started with, {} when omitted, got {described}",
            case.label,
            DEFAULT_SEARCH_TYPE
        );
        assert_eq!(
            described.get("groupId").and_then(Value::as_str),
            case.group_id,
            "case {}: the read carries the groupId the search was filed under, got {described}",
            case.label
        );
        assert_eq!(
            described["status"].as_str().unwrap_or(""),
            "QUEUED",
            "case {}: a search just started has not begun running, got {described}",
            case.label
        );
        assert!(
            described.get("startedAt").is_some_and(Value::is_number),
            "case {}: the read reports when the search was started, got {described}",
            case.label
        );
        assert!(
            described.get("statusReason").is_none(),
            "case {}: a statusReason is populated only for a FAILED search, got {described}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_search_is_reachable_only_through_its_own_workspace() {
    let router = router();
    for workspace in ["plant-one", "plant-two", "plant-three"] {
        create_workspace(&router, workspace).await;
    }
    let (status, started) = start(
        &router,
        "plant-one",
        json!({ "queryStatement": "only in plant one" }),
    )
    .await;
    assert_eq!(status, 202, "StartSearch: got {status} {started}");
    let search_id = started["searchId"].as_str().unwrap_or("").to_string();

    for (label, workspace, expected) in [
        ("its-own-workspace", "plant-one", 200),
        ("another-workspace", "plant-two", 404),
        ("a-workspace-no-call-named", "plant-three", 404),
    ] {
        let (status, body) = describe(&router, workspace, &search_id).await;
        assert_eq!(
            status, expected,
            "case {label}: DescribeSearch through {workspace} must answer {expected}, got {status} {body}"
        );
        if expected == 404 {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {label}: a search no workspace holds is a modelled not-found, got {body}"
            );
        }
    }

    // A well-formed id no start ever minted is the same refusal.
    let (status, body) = describe(&router, "plant-one", "00000000-1111-2222-3333-444444444444").await;
    assert_eq!(status, 404, "an unknown searchId must be refused, got {status} {body}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "an unknown searchId is a modelled not-found, got {body}"
    );
}

#[tokio::test]
async fn repeating_a_client_token_answers_from_the_original_search() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;

    // The rows differ along the token itself: each published `ClientToken` length
    // bound (36 and 64) and a token of mixed non-space characters.
    for (label, client_token) in [
        ("shortest-token", token("first-call-token-", 36)),
        ("longest-token", token("second-call-token-", 64)),
        ("punctuated-token", token("third/call+token=", 40)),
    ] {
        let (status, first) = start(
            &router,
            WORKSPACE,
            json!({
                "queryStatement": format!("original statement for {label}"),
                "searchType": "DEEP",
                "groupId": "original",
                "clientToken": client_token,
            }),
        )
        .await;
        assert_eq!(status, 202, "case {label}: StartSearch: got {status} {first}");
        let search_id = first["searchId"].as_str().unwrap_or("").to_string();

        // The repeat carries the same token and a DIFFERENT request: it must start
        // nothing and answer from the search the token already started.
        let (status, repeat) = start(
            &router,
            WORKSPACE,
            json!({
                "queryStatement": format!("replacement statement for {label}"),
                "searchType": "QUICK",
                "groupId": "repeat",
                "clientToken": client_token,
            }),
        )
        .await;
        assert_eq!(status, 202, "case {label}: the repeat is accepted, got {status} {repeat}");
        assert_eq!(
            repeat["searchId"].as_str().unwrap_or(""),
            search_id,
            "case {label}: a repeated clientToken answers from the original search, got {repeat}"
        );
        assert_eq!(
            repeat["groupId"].as_str().unwrap_or(""),
            "original",
            "case {label}: the repeat reports the ORIGINAL search's members, got {repeat}"
        );

        let (status, described) = describe(&router, WORKSPACE, &search_id).await;
        assert_eq!(status, 200, "case {label}: DescribeSearch: got {status} {described}");
        assert_eq!(
            described["queryStatement"].as_str().unwrap_or(""),
            format!("original statement for {label}"),
            "case {label}: the repeat must not overwrite the stored search, got {}",
            described["queryStatement"]
        );
        assert_eq!(
            described["searchType"].as_str().unwrap_or(""),
            "DEEP",
            "case {label}: the repeat must not overwrite the stored search, got {described}"
        );

        // A token no call has used yet starts a search of its own.
        let (status, other) = start(
            &router,
            WORKSPACE,
            json!({
                "queryStatement": "a different search",
                "clientToken": token(&format!("{label}-other-"), 36),
            }),
        )
        .await;
        assert_eq!(status, 202, "case {label}: StartSearch: got {status} {other}");
        assert_ne!(
            other["searchId"].as_str().unwrap_or(""),
            search_id,
            "case {label}: a fresh clientToken must start a NEW search, got {other}"
        );
    }

    // Two calls carrying no token at all are two searches: the idempotency is the
    // token's, not the request's.
    let body = json!({ "queryStatement": "untokened" });
    let (_, first) = start(&router, WORKSPACE, body.clone()).await;
    let (_, second) = start(&router, WORKSPACE, body).await;
    assert_ne!(
        first["searchId"].as_str().unwrap_or(""),
        second["searchId"].as_str().unwrap_or("x"),
        "case no-token: a call carrying no clientToken starts a search of its own, got {first} and {second}"
    );
}

#[tokio::test]
async fn a_search_runs_and_succeeds_on_the_clock() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;
    let (status, started) = start(
        &router,
        WORKSPACE,
        json!({ "queryStatement": "watch me settle" }),
    )
    .await;
    assert_eq!(status, 202, "StartSearch: got {status} {started}");
    let search_id = started["searchId"].as_str().unwrap_or("").to_string();

    // The transition the start owes is not instant: a poll taken straight after it
    // still observes QUEUED, and polling twice does not move the search along.
    for poll in 1..=2 {
        let (status, described) = describe(&router, WORKSPACE, &search_id).await;
        assert_eq!(status, 200, "poll {poll}: DescribeSearch: got {status} {described}");
        assert_eq!(
            described["status"].as_str().unwrap_or(""),
            "QUEUED",
            "poll {poll}: a search stays QUEUED until its run falls due, and a read never moves it, got {described}"
        );
    }

    // Far enough past the queued span to be running, and well short of the end of
    // the run.
    tokio::time::sleep(SEARCH_QUEUED_FOR + SEARCH_RUNS_FOR / 2).await;
    let (status, running) = describe(&router, WORKSPACE, &search_id).await;
    assert_eq!(status, 200, "DescribeSearch: got {status} {running}");
    assert_eq!(
        running["status"].as_str().unwrap_or(""),
        "RUNNING",
        "the owed QUEUED -> RUNNING transition must fall due, got {running}"
    );

    tokio::time::sleep(SEARCH_RUNS_FOR + Duration::from_millis(200)).await;
    for poll in 1..=2 {
        let (status, succeeded) = describe(&router, WORKSPACE, &search_id).await;
        assert_eq!(status, 200, "poll {poll}: DescribeSearch: got {status} {succeeded}");
        assert_eq!(
            succeeded["status"].as_str().unwrap_or(""),
            "SUCCEEDED",
            "poll {poll}: the owed RUNNING -> SUCCEEDED transition must fall due and be terminal, got {succeeded}"
        );
        assert!(
            succeeded.get("statusReason").is_none(),
            "poll {poll}: a search that did not fail carries no statusReason, got {succeeded}"
        );
    }
}

#[tokio::test]
async fn an_interval_that_ends_before_it_starts_is_refused() {
    let (router, engine) = router_with_engine();
    create_workspace(&router, WORKSPACE).await;
    let held = || {
        let state = service_state(&*engine).expect("the iotsitewise store");
        // The binding is load-bearing, not redundant: the guard is a temporary of
        // the block's tail expression, so returning the `.len()` call directly
        // keeps it alive past `state` and does not borrow-check.
        let stored = state.lock().searches.len();
        stored
    };

    // The rows differ along the ordering of ONE interval's bounds, at second and at
    // nanosecond granularity, and along how many intervals the filter carries.
    let cases: Vec<(&str, Value, u16)> = vec![
        (
            "ascending-seconds",
            json!([{ "startTime": at(1_700_000_000, 0), "endTime": at(1_700_000_001, 0) }]),
            202,
        ),
        (
            "equal-bounds",
            json!([{ "startTime": at(1_700_000_000, 7), "endTime": at(1_700_000_000, 7) }]),
            202,
        ),
        (
            "ascending-offsets",
            json!([{ "startTime": at(1_700_000_000, 7), "endTime": at(1_700_000_000, 8) }]),
            202,
        ),
        (
            "descending-seconds",
            json!([{ "startTime": at(1_700_000_001, 0), "endTime": at(1_700_000_000, 0) }]),
            400,
        ),
        (
            "descending-offsets",
            json!([{ "startTime": at(1_700_000_000, 8), "endTime": at(1_700_000_000, 7) }]),
            400,
        ),
        (
            "one-descending-among-ascending",
            json!([
                { "startTime": at(1_700_000_000, 0), "endTime": at(1_700_000_010, 0) },
                { "startTime": at(1_700_000_030, 0), "endTime": at(1_700_000_020, 0) },
            ]),
            400,
        ),
    ];

    for (label, intervals, expected) in cases {
        let before = held();
        let (status, body) = start(
            &router,
            WORKSPACE,
            json!({
                "queryStatement": format!("search for {label}"),
                "searchFilters": { "timeIntervals": intervals },
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {label}: StartSearch must answer {expected}, got {status} {body}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {label}: an interval that ends before it starts is a modelled bad request, got {body}"
            );
            assert_eq!(
                held(),
                before,
                "case {label}: a refused call must start no search, got {} stored",
                held()
            );
        } else {
            assert_eq!(
                held(),
                before + 1,
                "case {label}: an accepted call stores exactly one search, got {} stored",
                held()
            );
        }
    }
}

#[tokio::test]
async fn the_per_workspace_bound_refuses_past_its_cap() {
    let router = router();
    create_workspace(&router, "crowded").await;
    create_workspace(&router, "roomy").await;
    for ordinal in 1..=MAX_SEARCHES_PER_WORKSPACE {
        let (status, body) = start(
            &router,
            "crowded",
            json!({ "queryStatement": format!("search {ordinal}") }),
        )
        .await;
        assert_eq!(
            status, 202,
            "search {ordinal} is within the cap of {MAX_SEARCHES_PER_WORKSPACE}, got {status} {body}"
        );
    }

    let (status, body) = start(&router, "crowded", json!({ "queryStatement": "one too many" })).await;
    assert_eq!(
        status, 410,
        "past {MAX_SEARCHES_PER_WORKSPACE} searches the workspace is full, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "a full workspace is the modelled quota refusal, got {body}"
    );

    // The bound is the WORKSPACE's: every other workspace still accepts a search.
    let (status, body) = start(&router, "roomy", json!({ "queryStatement": "elsewhere" })).await;
    assert_eq!(
        status, 202,
        "one full workspace must not close another, got {status} {body}"
    );
}

#[tokio::test]
async fn the_whole_store_bound_refuses_past_its_cap() {
    let router = router();
    // The store is filled through SEVERAL workspaces, each up to its own cap, so
    // what refuses the next start is the whole-store total rather than any one
    // workspace's count.
    let workspaces = MAX_SEARCHES / MAX_SEARCHES_PER_WORKSPACE;
    for ordinal in 1..=workspaces {
        let workspace = format!("workspace-{ordinal}");
        create_workspace(&router, &workspace).await;
        for search in 1..=MAX_SEARCHES_PER_WORKSPACE {
            let (status, body) = start(
                &router,
                &workspace,
                json!({ "queryStatement": format!("search {ordinal}-{search}") }),
            )
            .await;
            assert_eq!(
                status, 202,
                "search {ordinal}-{search} is within the cap of {MAX_SEARCHES}, got {status} {body}"
            );
        }
    }

    // A workspace holding NO search at all, so only the whole-store bound can
    // refuse this one.
    create_workspace(&router, "a-workspace-of-its-own").await;
    let (status, body) = start(
        &router,
        "a-workspace-of-its-own",
        json!({ "queryStatement": "one too many" }),
    )
    .await;
    assert_eq!(
        status, 410,
        "past {MAX_SEARCHES} searches the store is full, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "a full store is the modelled quota refusal, got {body}"
    );
}

#[tokio::test]
async fn a_workspace_the_store_does_not_hold_is_refused() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;
    let (status, started) = start(
        &router,
        WORKSPACE,
        json!({ "queryStatement": "in a workspace that exists" }),
    )
    .await;
    assert_eq!(status, 202, "StartSearch: got {status} {started}");
    let search_id = started["searchId"].as_str().unwrap_or("").to_string();

    // The rows differ along the workspace named: one no call ever created, one
    // whose name only resembles the created workspace, and one at each published
    // `WorkspaceName` length bound (1 and 64).
    for (label, workspace) in [
        ("never-created", "no-such-workspace"),
        ("near-miss", "plant-floor-2"),
        ("shortest-name", "w"),
        ("longest-name", "w23456789-123456789-123456789-123456789-123456789-123456789-1234"),
    ] {
        let (status, body) = start(
            &router,
            workspace,
            json!({ "queryStatement": format!("search for {label}") }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label}: StartSearch against an absent workspace must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label}: an absent workspace is a modelled not-found, got {body}"
        );

        // The same for a read: the workspace is what is missing, so naming a real
        // search under it is still refused.
        let (status, body) = describe(&router, workspace, &search_id).await;
        assert_eq!(
            status, 404,
            "case {label}: DescribeSearch through an absent workspace must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label}: an absent workspace is a modelled not-found, got {body}"
        );
    }
}

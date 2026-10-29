// SPDX-License-Identifier: CC0-1.0

//! What a `clientToken` already spent on a DIFFERENT `StartSearch` request means,
//! driven over HTTP through the same `MultiServiceRouter` the emulator boots with,
//! so every assertion is about the wire behaviour a real client sees.
//!
//! The rule these rows pin: a `clientToken` is spent by ONE call. A repeat of the
//! SAME request under that token is a RETRY of that call — answered from the search
//! it started, writing nothing — while a request differing in any member it is
//! recognised by belongs to another call and is refused
//! `ConflictingOperationException` (409) naming the search the token's own call
//! started. A refused request starts no search, so the answer a later
//! `GetSearchResults` or `DescribeSearch` gives is never the answer to a query
//! nobody ran.
//!
//! The rows span what the rule generalises over — the member the second request
//! differs in: the `queryStatement`, the `searchType`, a `groupId` the first call
//! named, a `groupId` the first call named none of, each member of `searchFilters`
//! (the time series it restricts to, the datasets, the time intervals), filters
//! dropped entirely, and the `workspaceName` the search runs against.
//!
//! The HTTP harness (`call`, `error_code`, `router`) is `asset_lifecycle`'s and the
//! `StartSearch` / `DescribeSearch` / `CreateWorkspace` wrappers are
//! `search_lifecycle`'s; both are reused rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, router};
use crate::search_lifecycle::{create_workspace, describe, start};

/// The workspace most rows start their searches in.
const WORKSPACE: &str = "plant-floor";

/// A second workspace, for the row whose differing request names another one: a
/// search runs against the data of ONE workspace, so the workspace is a member the
/// request is recognised by.
const OTHER_WORKSPACE: &str = "plant_two";

/// A `clientToken` built from `seed`: the modelled `ClientToken` is `len 36..64,
/// pattern ^\S{36,64}$` (smithy_query query="operation" operation="StartSearch"),
/// so the seed is padded with non-space characters to the shortest admitted length.
fn client_token(seed: &str) -> String {
    let mut token: String = seed.chars().filter(|character| !character.is_whitespace()).collect();
    while token.len() < 36 {
        token.push('z');
    }
    token.truncate(36);
    token
}

/// `body` with `clientToken` added, as `StartSearch` takes it in the payload.
fn with_token(body: &Value, token: &str) -> Value {
    let mut carried = body.clone();
    carried
        .as_object_mut()
        .expect("a StartSearch body is an object")
        .insert("clientToken".to_string(), json!(token));
    carried
}

/// A `TimeInNanos` body: `timeInSeconds` (range 1..) and its nanosecond offset.
fn at(seconds: i64) -> Value {
    json!({ "timeInSeconds": seconds, "offsetInNanos": 0 })
}

/// `ListSearches` on `workspace`, as `OPERATIONS.md` reports its route, and the
/// `searchId`s it reports.
async fn listed_ids(router: &MultiServiceRouter, workspace: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/searches/list"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "listing {workspace}: got {status} {body}"
    );
    body["searchSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("listing {workspace}: no searchSummaries in {body}"))
        .iter()
        .map(|summary| {
            summary["searchId"]
                .as_str()
                .unwrap_or_else(|| panic!("listing {workspace}: a summary without a searchId: {summary}"))
                .to_string()
        })
        .collect()
}

/// One row: the request a token is spent on, and a DIFFERENT request of the same
/// operation sent under that same, now spent, token.
struct Row {
    case: &'static str,
    /// The workspace the first request searches, and the body it carries (without
    /// a token).
    workspace: &'static str,
    body: Value,
    /// The workspace the DIFFERING request searches — another one, for the row
    /// that differs in the workspace it addresses — and the body it carries.
    other_workspace: &'static str,
    other_body: Value,
}

/// The rows: each is the same operation sent twice under one token, differing in
/// exactly one member the request is recognised by.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "another query statement",
            workspace: WORKSPACE,
            body: json!({ "queryStatement": "turbine vibration last week" }),
            other_workspace: WORKSPACE,
            other_body: json!({ "queryStatement": "pump pressure last week" }),
        },
        Row {
            case: "another search strategy",
            workspace: WORKSPACE,
            // Both values the `SearchType` enum names (smithy_query query="shape"
            // shape="SearchType": DEEP, QUICK).
            body: json!({ "queryStatement": "compressor temperature", "searchType": "QUICK" }),
            other_workspace: WORKSPACE,
            other_body: json!({ "queryStatement": "compressor temperature", "searchType": "DEEP" }),
        },
        Row {
            case: "another group",
            workspace: WORKSPACE,
            body: json!({ "queryStatement": "grouped search", "groupId": "ab" }),
            other_workspace: WORKSPACE,
            other_body: json!({
                "queryStatement": "grouped search",
                // The published `GroupId` length ceiling (36).
                "groupId": "g".repeat(36),
            }),
        },
        Row {
            case: "a group where the first call named none",
            workspace: WORKSPACE,
            body: json!({ "queryStatement": "ungrouped search" }),
            other_workspace: WORKSPACE,
            other_body: json!({ "queryStatement": "ungrouped search", "groupId": "late-group" }),
        },
        Row {
            case: "another time series filter",
            workspace: WORKSPACE,
            body: json!({
                "queryStatement": "filtered by series",
                "searchFilters": { "timeSeriesIds": ["11111111-2222-3333-4444-555555555555"] },
            }),
            other_workspace: WORKSPACE,
            other_body: json!({
                "queryStatement": "filtered by series",
                "searchFilters": { "timeSeriesIds": ["66666666-7777-8888-9999-aaaaaaaaaaaa"] },
            }),
        },
        Row {
            case: "another dataset filter",
            workspace: WORKSPACE,
            body: json!({
                "queryStatement": "filtered by dataset",
                "searchFilters": { "datasetIds": ["aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"] },
            }),
            other_workspace: WORKSPACE,
            other_body: json!({
                "queryStatement": "filtered by dataset",
                "searchFilters": {
                    "datasetIds": [
                        "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                        "bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
                    ],
                },
            }),
        },
        Row {
            case: "another time interval",
            workspace: WORKSPACE,
            body: json!({
                "queryStatement": "filtered by window",
                "searchFilters": {
                    "timeIntervals": [{ "startTime": at(1_700_000_000), "endTime": at(1_700_003_600) }],
                },
            }),
            other_workspace: WORKSPACE,
            other_body: json!({
                "queryStatement": "filtered by window",
                "searchFilters": {
                    "timeIntervals": [{ "startTime": at(1_700_000_000), "endTime": at(1_700_007_200) }],
                },
            }),
        },
        Row {
            case: "the filters dropped",
            workspace: WORKSPACE,
            body: json!({
                "queryStatement": "filters then none",
                "searchFilters": { "timeSeriesIds": ["11111111-2222-3333-4444-555555555555"] },
            }),
            other_workspace: WORKSPACE,
            other_body: json!({ "queryStatement": "filters then none" }),
        },
        Row {
            case: "another workspace",
            workspace: WORKSPACE,
            body: json!({ "queryStatement": "same statement, another workspace" }),
            other_workspace: OTHER_WORKSPACE,
            other_body: json!({ "queryStatement": "same statement, another workspace" }),
        },
    ]
}

/// `true` when `arn` is an ARN of this service naming the search `search_id` of
/// `workspace`: the modelled `ResourceArn` form
/// (`^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`, smithy_query query="shape"
/// shape="ConflictingOperationException") whose resource section carries both
/// names.
///
/// Hand-rolled as an INDEPENDENT oracle rather than compared against whatever the
/// handler built, so an ARN naming some other resource cannot agree with itself and
/// pass.
fn names_the_search(arn: &str, workspace: &str, search_id: &str) -> bool {
    let sections: Vec<&str> = arn.split(':').collect();
    sections.len() >= 6
        && sections[0] == "arn"
        && sections[1].starts_with("aws")
        && sections[2] == "iotsitewise"
        && arn
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-:/_.".contains(&byte))
        && sections[5..].join(":").ends_with(&format!("/{workspace}/{search_id}"))
}

#[tokio::test]
async fn a_token_one_start_spent_refuses_a_different_start_and_starts_nothing() {
    for row in rows() {
        // A router per row, so the listings below report exactly what this row's
        // own calls left behind.
        let router = router();
        create_workspace(&router, row.workspace).await;
        if row.other_workspace != row.workspace {
            create_workspace(&router, row.other_workspace).await;
        }
        let token = client_token(row.case);

        let (status, first) = start(
            &router,
            row.workspace,
            with_token(&row.body, &token),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {:?}: the first start expected 202, got {status} {first}",
            row.case
        );
        let search_id = first["searchId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no searchId in {first}", row.case))
            .to_string();

        // The SAME request under that token is a retry of the call above: it is
        // answered from the search that call started.
        let (status, retried) = start(
            &router,
            row.workspace,
            with_token(&row.body, &token),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {:?}: the retry expected 202, got {status} {retried}",
            row.case
        );
        assert_eq!(
            retried["searchId"].as_str().unwrap_or(""),
            search_id,
            "case {:?}: a retry answers from the search its token started, got {retried}",
            row.case
        );
        assert_eq!(
            retried["groupId"], first["groupId"],
            "case {:?}: a retry reports the first call's groupId, got {retried}",
            row.case
        );

        // The DIFFERING request reusing that spent token is the modelled conflict,
        // naming the search the token's own call started.
        let (status, refused) = start(
            &router,
            row.other_workspace,
            with_token(&row.other_body, &token),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused)),
            (409, "ConflictingOperationException"),
            "case {:?}: a spent token expected the modelled conflict, got {status} {refused}",
            row.case
        );
        assert_eq!(
            refused["resourceId"].as_str().unwrap_or(""),
            search_id,
            "case {:?}: the conflict names the search the first call started, got {refused}",
            row.case
        );
        let reported_arn = refused["resourceArn"].as_str().unwrap_or("");
        assert!(
            names_the_search(reported_arn, row.workspace, &search_id),
            "case {:?}: expected an ARN naming the search {search_id} of {}, got {reported_arn:?}",
            row.case,
            row.workspace
        );

        // The refused request started nothing: the stored search is still the one
        // the first call started, and no workspace holds a second.
        let (status, described) = describe(&router, row.workspace, &search_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the read of the first search got {status} {described}",
            row.case
        );
        assert_eq!(
            described["queryStatement"], row.body["queryStatement"],
            "case {:?}: the refused request must not rewrite the stored search, got {described}",
            row.case
        );
        let held = listed_ids(&router, row.workspace).await;
        assert_eq!(
            held,
            vec![search_id.clone()],
            "case {:?}: {} must hold only the search the first call started, holds {held:?}",
            row.case,
            row.workspace
        );
        if row.other_workspace != row.workspace {
            let elsewhere = listed_ids(&router, row.other_workspace).await;
            assert!(
                elsewhere.is_empty(),
                "case {:?}: the refused request must start no search in {}, holds {elsewhere:?}",
                row.case,
                row.other_workspace
            );
        }

        // The refusal spent nothing of its own: the token still names the first
        // call, so a retry of THAT request is still answered from its search.
        let (status, again) = start(
            &router,
            row.workspace,
            with_token(&row.body, &token),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {:?}: the retry after the refusal expected 202, got {status} {again}",
            row.case
        );
        assert_eq!(
            again["searchId"].as_str().unwrap_or(""),
            search_id,
            "case {:?}: a refused request must not unspend the token, got {again}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_token_no_call_has_spent_starts_a_search_of_its_own() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;
    let body = json!({ "queryStatement": "a statement every row sends" });

    // Rows: fresh tokens at each published `ClientToken` length bound and one in
    // between, plus two requests carrying no token at all. None of them repeats a
    // call, so each must start a search of its own even though every request body
    // is otherwise identical.
    let mut tokens: Vec<Option<String>> = vec![None, None];
    for length in [36_usize, 50, 64] {
        let mut token = client_token(&format!("fresh-token-of-{length}-characters"));
        while token.len() < length {
            token.push('f');
        }
        assert_eq!(
            token.len(),
            length,
            "case {length}: this row needs a token of {length} characters, got {token:?}"
        );
        tokens.push(Some(token));
    }

    let mut minted: Vec<String> = Vec::new();
    for token in &tokens {
        let request = match token {
            Some(token) => with_token(&body, token),
            None => body.clone(),
        };
        let case = token.as_deref().unwrap_or("no token");
        let (status, started) = start(&router, WORKSPACE, request).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202, got {status} {started}"
        );
        let search_id = started["searchId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no searchId in {started}"))
            .to_string();
        assert!(
            !minted.contains(&search_id),
            "case {case:?}: a call that repeats none must start a search of its own, \
             got {search_id} again"
        );
        minted.push(search_id);
    }

    let held = listed_ids(&router, WORKSPACE).await;
    assert_eq!(
        held.len(),
        minted.len(),
        "every row started a search of its own: {minted:?} started, {held:?} held"
    );
}

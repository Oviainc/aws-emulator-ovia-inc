// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListBuilds`, the account-wide build id list, driven over the
//! wire the router serves (`POST /` with `X-Amz-Target: CodeBuild_20161006.ListBuilds`).
//!
//! RULE under test: `ListBuilds` reports the ids of EVERY build the account holds,
//! across all of its projects, ordered by build id in the requested direction
//! (descending unless the request asks for `ASCENDING`), one page of at most the
//! modelled 100 entries at a time plus a `nextToken` that resumes after that page; a
//! token naming no stored build is refused with the modelled `InvalidInputException`,
//! and the call reports stored state without changing it.
//!
//! The harness (`emulator`, `call`, `create_project`, `start_build`, `error_type`) is
//! the one `project_build_lifecycle` already drives this service with.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;

use crate::project_build_lifecycle::{
    call, create_project, emulator, error_type, start_build, PAST_THE_SETTLE_SPAN,
};

/// Start `count` builds of `project`, which must already exist, and hand back their ids.
async fn start_builds(
    router: &smithy_server_runtime::MultiServiceRouter,
    project: &str,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        let build = start_build(router, json!({"projectName": project})).await;
        ids.push(
            build["id"]
                .as_str()
                .unwrap_or_else(|| panic!("StartBuild {project} must report a build id, got {build}"))
                .to_string(),
        );
    }
    ids
}

/// The `ids` member of a `ListBuilds` answer, as strings.
///
/// A non-string entry is a defect rather than something to paper over, so it panics
/// instead of mapping to a placeholder.
fn listed_ids(body: &Value) -> Vec<String> {
    match body["ids"].as_array() {
        None => Vec::new(),
        Some(ids) => ids
            .iter()
            .map(|id| {
                id.as_str()
                    .unwrap_or_else(|| panic!("every entry of ids must be a string, got {id}"))
                    .to_string()
            })
            .collect(),
    }
}

/// Every build of every project, in the direction the request asked for.
///
/// The table varies the number of projects and the number of builds each one holds, so
/// a handler that listed only one project's builds, or listed per-project rather than
/// account-wide, shows up; and it varies `sortOrder` over the enum's published values
/// (`SortOrderType`: ASCENDING | DESCENDING) plus the absent case.
#[tokio::test]
async fn list_builds_reports_every_stored_build_in_the_requested_order() {
    // (case, builds started per project)
    let cases: [(&str, &[usize]); 4] = [
        ("one project, one build", &[1]),
        ("one project, several builds", &[4]),
        ("several projects, uneven counts", &[1, 3, 2]),
        ("a project with no builds at all", &[2, 0]),
    ];

    for (case, per_project) in cases {
        let router = emulator();
        let mut all_ids: Vec<String> = Vec::new();
        for (index, count) in per_project.iter().enumerate() {
            let project = format!("proj-{index}");
            create_project(&router, &project, json!({})).await;
            all_ids.extend(start_builds(&router, &project, *count).await);
        }
        // The documented criterion is the build id itself ("ASCENDING: List the build
        // IDs in ascending order by build ID"), not the start order.
        let mut ascending = all_ids.clone();
        ascending.sort();
        let descending: Vec<String> = ascending.iter().rev().cloned().collect();

        // (what the request carries for sortOrder, the order expected back)
        let orders: [(Value, &Vec<String>); 3] = [
            (Value::Null, &descending),
            (json!("ASCENDING"), &ascending),
            (json!("DESCENDING"), &descending),
        ];
        for (sort_order, expected) in orders {
            let mut request = json!({});
            if !sort_order.is_null() {
                request["sortOrder"] = sort_order.clone();
            }
            let (status, body) = call(&router, "ListBuilds", request).await;
            assert_eq!(
                status, 200,
                "case {case:?} / sortOrder {sort_order}: expected 200, got {status} with {body}"
            );
            assert_eq!(
                &listed_ids(&body),
                expected,
                "case {case:?} / sortOrder {sort_order}: expected {expected:?}, got {body}"
            );
            assert!(
                body["nextToken"].is_null(),
                "case {case:?} / sortOrder {sort_order}: one page holds every id, got {body}"
            );
        }

        // Every id listed names a build an implemented reader hands back — the ids are
        // the stored builds', not strings minted for the list.
        let (status, fetched) = call(
            &router,
            "BatchGetBuilds",
            json!({"ids": ascending.clone()}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetBuilds of the listed ids: expected 200, got {status} with {fetched}"
        );
        let found = fetched["builds"].as_array().map_or(0, Vec::len);
        assert_eq!(
            found,
            ascending.len(),
            "case {case:?}: every listed id must resolve to a stored build, got {fetched}"
        );
        assert!(
            fetched["buildsNotFound"].is_null(),
            "case {case:?}: no listed id may be unknown to BatchGetBuilds, got {fetched}"
        );
    }
}

/// An account with no builds has no page of ids: the model bounds `BuildIds` at
/// `@length(min: 1, max: 100)`, so the member is absent rather than an empty list.
#[tokio::test]
async fn list_builds_on_an_account_with_no_builds_omits_the_ids_member() {
    let router = emulator();

    // (case, how the account was set up)
    for case in ["no projects at all", "a project with no builds"] {
        if case == "a project with no builds" {
            create_project(&router, "empty-proj", json!({})).await;
        }
        let (status, body) = call(&router, "ListBuilds", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert!(
            body["ids"].is_null(),
            "case {case:?}: ids is bounded at 1..100, so it must be absent, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: an empty list carries no nextToken, got {body}"
        );
    }
}

/// Paging: one page of at most `MAX_PAGE_SIZE` ids (the model's `BuildIds` bound) plus
/// a token that resumes after it, in both directions; and the pages joined are exactly
/// the stored ids, once each.
#[tokio::test]
async fn list_builds_pages_over_every_stored_build_id() {
    let router = emulator();

    // Builds spread over two projects, so a page boundary falls inside the account-wide
    // list and not at a project boundary: 105 > the 100-entry page.
    let page_size = state::MAX_PAGE_SIZE;
    let total = page_size + 5;
    let mut all_ids: Vec<String> = Vec::new();
    create_project(&router, "page-a", json!({})).await;
    create_project(&router, "page-b", json!({})).await;
    all_ids.extend(start_builds(&router, "page-a", 60).await);
    all_ids.extend(start_builds(&router, "page-b", total - 60).await);
    let mut ascending = all_ids.clone();
    ascending.sort();
    let descending: Vec<String> = ascending.iter().rev().cloned().collect();

    // (case, sortOrder, the whole list in that order)
    let cases: [(&str, &str, &Vec<String>); 2] = [
        ("ascending", "ASCENDING", &ascending),
        ("descending", "DESCENDING", &descending),
    ];
    for (case, sort_order, expected) in cases {
        let (status, first) = call(&router, "ListBuilds", json!({"sortOrder": sort_order})).await;
        assert_eq!(
            status, 200,
            "case {case:?} page 1: expected 200, got {status} with {first}"
        );
        let page_one = listed_ids(&first);
        assert_eq!(
            page_one.len(),
            page_size,
            "case {case:?} page 1: expected {page_size} ids, got {}",
            page_one.len()
        );
        assert_eq!(
            &page_one,
            &expected[..page_size],
            "case {case:?} page 1 must be the first page of the ordered ids, got {first}"
        );
        let token = first["nextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?} page 1 must carry a nextToken, got {first}"))
            .to_string();

        let (status, second) = call(
            &router,
            "ListBuilds",
            json!({"sortOrder": sort_order, "nextToken": token}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} page 2: expected 200, got {status} with {second}"
        );
        let page_two = listed_ids(&second);
        assert_eq!(
            &page_two,
            &expected[page_size..],
            "case {case:?} page 2 must resume after page 1, got {second}"
        );
        assert!(
            second["nextToken"].is_null(),
            "case {case:?} page 2 is the last page, got {second}"
        );

        let joined: Vec<String> = page_one.into_iter().chain(page_two).collect();
        assert_eq!(
            &joined, expected,
            "case {case:?}: the pages joined must be every stored id, once each"
        );
    }
}

/// A `nextToken` that names no stored build is a refusable request, not an empty page:
/// `ListBuilds` models `InvalidInputException` (HTTP 400) and nothing else.
#[tokio::test]
async fn list_builds_refuses_a_next_token_naming_no_stored_build() {
    let router = emulator();
    create_project(&router, "token-proj", json!({})).await;
    let started = start_builds(&router, "token-proj", 2).await;

    // (case, the token sent)
    let cases: [(&str, String); 5] = [
        ("empty", String::new()),
        ("not a token at all", "not-a-token".to_string()),
        ("a build id with a changed suffix", format!("{}x", started[0])),
        ("a project name rather than a build id", "token-proj".to_string()),
        (
            "a build id of a project that holds none",
            "other-proj:00000000-0000-8000-8000-000000000000".to_string(),
        ),
    ];
    for (case, token) in cases {
        let (status, body) = call(&router, "ListBuilds", json!({"nextToken": token})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body}"
        );
    }

    // The refusals changed nothing: the stored ids are still all of them.
    let (status, body) = call(&router, "ListBuilds", json!({"sortOrder": "ASCENDING"})).await;
    let mut ascending = started.clone();
    ascending.sort();
    assert_eq!(status, 200, "after the refusals: expected 200, got {status}");
    assert_eq!(
        listed_ids(&body),
        ascending,
        "after the refusals every stored id must still be listed, got {body}"
    );
}

/// `ListBuilds` reports; it does not change what a later caller sees. Listing twice —
/// and listing between two reads of a build — leaves both the list and the build as
/// they were.
#[tokio::test]
async fn list_builds_does_not_change_what_a_later_caller_sees() {
    let router = emulator();
    create_project(&router, "read-proj", json!({})).await;
    let ids = start_builds(&router, "read-proj", 3).await;

    // Past the span the clock owes each build's transition out of `IN_PROGRESS`, so the
    // two reads below differ only if a call changed something — not because the clock
    // settled a build between them.
    tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;

    let (_, before) = call(&router, "BatchGetBuilds", json!({"ids": ids.clone()})).await;
    let (_, first) = call(&router, "ListBuilds", json!({})).await;
    let (_, again) = call(&router, "ListBuilds", json!({})).await;
    assert_eq!(
        first, again,
        "two ListBuilds calls must answer identically, got {first} then {again}"
    );
    let (_, after) = call(&router, "BatchGetBuilds", json!({"ids": ids.clone()})).await;
    assert_eq!(
        before["builds"], after["builds"],
        "ListBuilds must not change a stored build: {} then {}",
        before["builds"], after["builds"]
    );
    assert_eq!(
        listed_ids(&first).len(),
        ids.len(),
        "ListBuilds must list every started build, got {first}"
    );
}

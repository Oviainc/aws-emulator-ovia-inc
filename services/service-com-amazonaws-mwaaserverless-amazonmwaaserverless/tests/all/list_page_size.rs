// SPDX-License-Identifier: CC0-1.0

//! The one page-size derivation every paginated listing of this service shares.
//!
//! The rule under test: a listing serves at most `MaxResults` items per page, held to
//! the bounds that member's model declares — `@range(min: 1, max: 100)` with
//! `@default: 20` on `ListWorkflows`, `ListWorkflowRuns`, `ListWorkflowVersions` and
//! `ListTaskInstances` alike (`smithy_query(query="shape",
//! shape="ListWorkflowsRequest,ListWorkflowRunsRequest,ListWorkflowVersionsRequest,
//! ListTaskInstancesRequest")`) — so a walk at any page size reports exactly the items
//! the store holds, each once, in one order, and never stalls on a page size of zero.
//! Every one of those four listings reads that page size through
//! `crate::state::page_size`, so the derivation has one home and cannot drift between
//! them.
//!
//! The tables span what the rule generalises over: which listing is walked (one per
//! former copy of the derivation), the page size (both published bounds, sizes between
//! them, a size covering the whole set, and the modelled default), and — on the shared
//! derivation itself — every `MaxResults` value an i32 can carry, including each
//! published bound and one past it in both directions.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    page_size, MAX_PAGE_SIZE, MIN_PAGE_SIZE, TASK_INSTANCE_STATUS_NONE,
};

use crate::run_lifecycle_progression::with_run;
use crate::task_instance_reads::{instance, seed};
use crate::workflow_lifecycle::Harness;
use crate::workflow_run_listing::{appended_version, created_workflow, started_run};

/// The page size the model applies when a request names none: every paginated request
/// of this service declares `MaxResults` with `@default: 20`.
const DEFAULT_PAGE_SIZE: i64 = 20;

/// The published bounds of the modelled `MaxResults` range, read from the one place
/// the service declares them, so the bound under test lives in a single place.
const PUBLISHED_MIN: i64 = MIN_PAGE_SIZE as i64;

/// The largest page size that same range allows.
const PUBLISHED_MAX: i64 = MAX_PAGE_SIZE as i64;

/// How many items each walked listing is given, so the small page sizes need several
/// pages and the large ones a single page.
const ITEMS: usize = 5;

/// One walk's page size: the `MaxResults` sent, or `None` for "let the model default
/// apply".
///
/// The rows are the published bounds, sizes between them, the exact size of the set and
/// the modelled default.
fn page_sizes() -> Vec<Option<i64>> {
    vec![
        Some(PUBLISHED_MIN),
        Some(2),
        Some(3),
        Some(ITEMS as i64),
        Some(PUBLISHED_MAX),
        None,
    ]
}

/// One paginated listing to walk: the operation, the request members that name what it
/// walks, the `@paginated` `items` member it answers in, and the member identifying one
/// item.
struct Listing {
    operation: &'static str,
    request: serde_json::Value,
    items: &'static str,
    id: &'static str,
}

/// Walk one listing all the way through the cursors it issues, asserting no page exceeds
/// the page size in force; report the item ids seen, in the order reported.
async fn walk(harness: &Harness, listing: &Listing, max_results: Option<i64>) -> Vec<String> {
    let effective = max_results.unwrap_or(DEFAULT_PAGE_SIZE);
    let case = format!(
        "{}/MaxResults={}",
        listing.operation,
        max_results.map_or_else(|| "default".to_string(), |size| size.to_string())
    );
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One page per item plus the first and a slack page: a walk needing more than that
    // is not making progress.
    let budget = ITEMS + 2;

    for step in 0..=budget {
        assert!(
            step < budget,
            "case {case:?}: the walk must finish within {budget} pages, got {seen:?}"
        );
        let mut body = listing.request.clone();
        if let Some(size) = max_results {
            body["MaxResults"] = serde_json::Value::from(size);
        }
        if let Some(cursor) = &token {
            body["NextToken"] = serde_json::Value::String(cursor.clone());
        }
        let (status, page) = harness.call(listing.operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the page to succeed, got {status} {page:?}"
        );
        let items = page[listing.items]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a {} list, got {page:?}", listing.items))
            .clone();
        assert!(
            i64::try_from(items.len()).is_ok_and(|count| count <= effective),
            "case {case:?}: a page may carry at most {effective} items, got {}",
            items.len()
        );
        for item in &items {
            let id = item[listing.id].as_str().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: every item carries its {}, got {item:?}",
                    listing.id
                )
            });
            seen.push(id.to_string());
        }
        match page["NextToken"].as_str() {
            Some(next) => {
                assert!(
                    !items.is_empty(),
                    "case {case:?}: an empty page must not carry a cursor, got {page:?}"
                );
                token = Some(next.to_string());
            }
            None => return seen,
        }
    }
    unreachable!("the loop returns or asserts")
}

/// A workflow holding [`ITEMS`] workflows in its own endpoint's store, for the
/// `ListWorkflows` walk.
async fn workflows_listing(harness: &Harness) -> Listing {
    for index in 0..ITEMS {
        created_workflow(harness, &format!("pageSizeWorkflow{index}")).await;
    }
    Listing {
        operation: "ListWorkflows",
        request: serde_json::json!({}),
        items: "Workflows",
        id: "WorkflowArn",
    }
}

/// A workflow holding [`ITEMS`] runs, for the `ListWorkflowRuns` walk. Each run is
/// pinned so no clock-owed status change falls due mid-walk.
async fn runs_listing(harness: &Harness) -> Listing {
    let arn = created_workflow(harness, "pageSizeRuns").await;
    for _ in 0..ITEMS {
        let run_id = started_run(harness, &arn, None).await;
        with_run(harness, &arn, &run_id, |run| {
            run.next_status_at = SystemTime::now() + Duration::from_secs(3_600);
        });
    }
    Listing {
        operation: "ListWorkflowRuns",
        request: serde_json::json!({"WorkflowArn": arn}),
        items: "WorkflowRuns",
        id: "RunId",
    }
}

/// A workflow holding [`ITEMS`] versions, for the `ListWorkflowVersions` walk: the one a
/// create mints plus one per update.
async fn versions_listing(harness: &Harness) -> Listing {
    let arn = created_workflow(harness, "pageSizeVersions").await;
    for index in 1..ITEMS {
        appended_version(harness, &arn, &format!("dags/page{index}.yaml")).await;
    }
    Listing {
        operation: "ListWorkflowVersions",
        request: serde_json::json!({"WorkflowArn": arn}),
        items: "WorkflowVersions",
        id: "WorkflowVersion",
    }
}

/// A run holding [`ITEMS`] task instances, for the `ListTaskInstances` walk. This run's
/// version staged no definition, so it declares no task and the run materialises none:
/// the records are seeded into the store the handler reads, through the same seam the
/// `GetTaskInstance` suite uses.
async fn task_instances_listing(harness: &Harness) -> Listing {
    let arn = created_workflow(harness, "pageSizeTasks").await;
    let run_id = started_run(harness, &arn, None).await;
    with_run(harness, &arn, &run_id, |run| {
        run.next_status_at = SystemTime::now() + Duration::from_secs(3_600);
    });
    for index in 0..ITEMS {
        seed(
            harness,
            &arn,
            &run_id,
            instance(&format!("task-{index}"), TASK_INSTANCE_STATUS_NONE),
        );
    }
    Listing {
        operation: "ListTaskInstances",
        request: serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
        items: "TaskInstances",
        id: "TaskInstanceId",
    }
}

#[tokio::test]
async fn every_paginated_listing_serves_the_same_walk_at_every_published_page_size() {
    // Each listing gets its own store, so what `ListWorkflows` walks is only the
    // workflows created for it.
    let workflows = Harness::new("us-east-2");
    let runs = Harness::new("eu-west-2");
    let versions = Harness::new("ap-northeast-1");
    let tasks = Harness::new("us-west-1");

    let cases: Vec<(&Harness, Listing)> = vec![
        (&workflows, workflows_listing(&workflows).await),
        (&runs, runs_listing(&runs).await),
        (&versions, versions_listing(&versions).await),
        (&tasks, task_instances_listing(&tasks).await),
    ];

    for (harness, listing) in &cases {
        // The whole set in one page, at the largest size the model publishes: the
        // sequence every other page size must reproduce.
        let whole = walk(harness, listing, Some(PUBLISHED_MAX)).await;
        assert_eq!(
            whole.len(),
            ITEMS,
            "case {}/whole: expected {ITEMS} items in the store, got {whole:?}",
            listing.operation
        );

        for max_results in page_sizes() {
            let seen = walk(harness, listing, max_results).await;
            let case = format!(
                "{}/MaxResults={}",
                listing.operation,
                max_results.map_or_else(|| "default".to_string(), |size| size.to_string())
            );
            assert_eq!(
                seen, whole,
                "case {case:?}: a walk must report every item exactly once in one order, \
                 got {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_shared_page_size_derivation_holds_every_max_results_to_the_published_bounds() {
    // The rows are each published bound of the modelled `@range(min: 1, max: 100)`, the
    // modelled default, a size between the bounds, and one value past each bound in both
    // directions — including the extremes an i32 can carry, which no request reaches
    // because the boundary refuses them, but which must still leave a walk able to make
    // progress.
    let cases: &[(&str, i32, usize)] = &[
        ("the published minimum", 1, MIN_PAGE_SIZE),
        ("one past the published minimum", 0, MIN_PAGE_SIZE),
        ("a negative page size", -1, MIN_PAGE_SIZE),
        ("the smallest i32", i32::MIN, MIN_PAGE_SIZE),
        ("a size between the bounds", 7, 7),
        ("the modelled default", 20, 20),
        ("the published maximum", 100, MAX_PAGE_SIZE),
        ("one past the published maximum", 101, MAX_PAGE_SIZE),
        ("the largest i32", i32::MAX, MAX_PAGE_SIZE),
    ];

    for (case, max_results, expected) in cases {
        let actual = page_size(*max_results);
        assert_eq!(
            actual, *expected,
            "case {case:?}: expected MaxResults {max_results} to derive a page size of \
             {expected}, got {actual}"
        );
        assert!(
            actual >= MIN_PAGE_SIZE,
            "case {case:?}: a page size must let a walk make progress, got {actual}"
        );
    }
}

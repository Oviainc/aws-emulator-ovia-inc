// SPDX-License-Identifier: CC0-1.0

//! `ListWorkflows`: the names of the workflows this account holds.
//!
//! RULE: the operation "lists names of workflows created in the account", so it reports
//! the name of every workflow the store holds — one `MaxResults`-sized page at a time in
//! name order, with a `NextToken` that resumes the walk where the previous page stopped
//! and no token once the listing is exhausted. It reports only: a listing changes nothing
//! a later caller sees.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many workflows are stored,
//! the page size asked for (including the published bounds 1 and 25, a size past the
//! stored count, and one past each bound), and the form of the stored names.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateWorkflowInput;
use service_com_amazonaws_glue_awsglue::state::{ComAmazonawsGlueAwsglueState, WorkflowEntry};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, ok};

/// The largest page `MaxResults` accepts: `OrchestrationPageSize25`, "range 1..25"
/// (`smithy_query operation=ListWorkflows`).
const MAX_PAGE_SIZE: i32 = 25;

/// An emulator and the engine its store is held on, so a suite can seed workflows without
/// driving one request per workflow.
fn emulator_with_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(engine.clone()), engine)
}

/// Record one workflow directly, through the same [`WorkflowEntry::from_create_input`] a
/// `CreateWorkflow` registers one with.
fn given_workflow(engine: &Arc<engine::InMemoryEngine>, name: &str) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    state.lock().insert_workflow(WorkflowEntry::from_create_input(
        &CreateWorkflowInput {
            name: name.to_string(),
            ..CreateWorkflowInput::default()
        },
        SystemTime::now(),
    ));
}

/// The names one answer reports, in the order the service returned them.
///
/// `Workflows` is present in every answer, as an empty list when the listing has nothing to
/// report, so an absent member would itself be a defect — [`crate::response_members`] is
/// where that is asserted; this reads the list.
fn listed_names(page: &Value) -> Vec<String> {
    match page.get("Workflows") {
        None | Some(Value::Null) => Vec::new(),
        Some(list) => list
            .as_array()
            .unwrap_or_else(|| panic!("Workflows must be a list, got {page}"))
            .iter()
            .map(|name| {
                name.as_str()
                    .unwrap_or_else(|| panic!("a workflow name is a string, got {page}"))
                    .to_string()
            })
            .collect(),
    }
}

/// Walk every page the service hands out, following its own `NextToken`, and return the
/// names in the order they were reported.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    page_size: Option<i32>,
    limit: usize,
) -> Vec<String> {
    let mut listed: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "ListWorkflows", &request).await;
        let names = listed_names(&page);
        let bound = usize::try_from(page_size.unwrap_or(MAX_PAGE_SIZE)).unwrap_or(usize::MAX);
        assert!(
            names.len() <= bound,
            "case {case:?}: a page must not exceed the page size {bound}, got {names:?}"
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
            listed.len() <= limit,
            "case {case:?}: the walk must terminate, got {listed:?} for {limit} workflows"
        );
    }
    listed
}

/// Every stored workflow name is reported, in name order, across whatever page size the
/// request asked for.
#[tokio::test]
async fn every_stored_workflow_name_is_listed_and_paged() {
    // (case, how many workflows are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no workflows at all", 0, None),
        ("one workflow, no page size", 1, None),
        // The published lower bound of `MaxResults` (`OrchestrationPageSize25`, 1..25).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page larger than the stored count", 3, Some(20)),
        // The published upper bound, with more stored than one page holds.
        ("the largest page the model accepts", 30, Some(MAX_PAGE_SIZE)),
        ("no page size, more stored than one page holds", 28, None),
    ];

    for (case, stored, page_size) in rows {
        let (router, engine) = emulator_with_store();
        // Seeded out of name order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("workflow-{index:02}"))
            .collect();
        for name in expected.iter().rev() {
            given_workflow(&engine, name);
        }

        let listed = walk(&router, case, *page_size, *stored).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected every stored workflow name in name order, got {listed:?}"
        );
    }
}

/// One page reports at most `MaxResults` names and carries a token only while names
/// remain; the token resumes the walk strictly after the last name of the page.
#[tokio::test]
async fn a_page_reports_at_most_max_results_names_and_a_token_only_when_more_remain() {
    let (router, engine) = emulator_with_store();
    for index in 0..4 {
        given_workflow(&engine, &format!("paged-{index}"));
    }

    // (case, MaxResults, the names that page reports, whether a token follows)
    let rows: &[(&str, i32, &[&str], bool)] = &[
        ("one at a time", 1, &["paged-0"], true),
        ("two at a time", 2, &["paged-0", "paged-1"], true),
        (
            "exactly the stored count",
            4,
            &["paged-0", "paged-1", "paged-2", "paged-3"],
            false,
        ),
        (
            "more than the stored count",
            MAX_PAGE_SIZE,
            &["paged-0", "paged-1", "paged-2", "paged-3"],
            false,
        ),
    ];

    for (case, max_results, expected, more) in rows {
        let page = ok(
            &router,
            "ListWorkflows",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        let names = listed_names(&page);
        assert_eq!(
            names,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the first {max_results} stored names, got {page}"
        );
        assert_eq!(
            page.get("NextToken").and_then(Value::as_str).is_some(),
            *more,
            "case {case:?}: a token must be reported only while names remain, got {page}"
        );
        if let Some(token) = page.get("NextToken").and_then(Value::as_str) {
            let next = ok(
                &router,
                "ListWorkflows",
                &json!({ "MaxResults": max_results, "NextToken": token }),
            )
            .await;
            let continued = listed_names(&next);
            assert!(
                continued
                    .first()
                    .is_some_and(|name| name.as_str() > *expected.last().expect("a listed name")),
                "case {case:?}: the next page must resume after {:?}, got {continued:?}",
                expected.last()
            );
        }
    }
}

/// A listing of an account holding no workflow, and a page past the end of the listing,
/// report an EMPTY `Workflows` list and no token: an empty listing is an empty collection,
/// not an omitted member.
#[tokio::test]
async fn an_exhausted_listing_reports_an_empty_workflows_member() {
    let (router, engine) = emulator_with_store();

    let empty = ok(&router, "ListWorkflows", &json!({})).await;
    assert_eq!(
        empty.get("Workflows"),
        Some(&json!([])),
        "case no workflows at all: expected an empty Workflows list, got {empty}"
    );
    assert!(
        empty.get("NextToken").is_none_or(Value::is_null),
        "case no workflows at all: expected no NextToken, got {empty}"
    );

    // A cursor at or past the last stored name leaves nothing to report either.
    given_workflow(&engine, "only-workflow");
    for (case, cursor) in [
        ("a cursor at the last stored name", "only-workflow"),
        ("a cursor past every stored name", "zzz-beyond"),
    ] {
        let page = ok(&router, "ListWorkflows", &json!({ "NextToken": cursor })).await;
        assert_eq!(
            page.get("Workflows"),
            Some(&json!([])),
            "case {case:?}: expected an empty Workflows list, got {page}"
        );
        assert!(
            page.get("NextToken").is_none_or(Value::is_null),
            "case {case:?}: expected no NextToken, got {page}"
        );
    }
}

/// The listing reports what the store holds NOW — a workflow a create wrote appears, one
/// the store no longer holds does not — and reporting changes nothing a later caller sees.
#[tokio::test]
async fn the_listing_reports_what_the_store_holds_now() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Each name is a different form the rule has to hold over: plain, whitespace-bearing,
    // digits, and mixed case (the model documents no case fold for a workflow name).
    let created = [
        "Nightly-Ingest",
        "ingest 2\tstage",
        "workflow-000",
        "zeta",
    ];
    for name in created {
        ok(&router, "CreateWorkflow", &json!({ "Name": name })).await;
    }

    let mut expected: Vec<String> = created.iter().map(|name| (*name).to_string()).collect();
    expected.sort();

    let before = walk(&router, "after four creates", None, created.len()).await;
    assert_eq!(
        before, expected,
        "case after four creates: expected every created workflow in name order, got {before:?}"
    );

    // Listing is reporting only: two listings in a row agree, and the store still holds
    // exactly what was listed.
    let again = walk(&router, "listed twice", None, created.len()).await;
    assert_eq!(
        again, before,
        "case listed twice: a listing must change nothing, got {again:?} then {before:?}"
    );
    let held: Vec<String> = engine
        .state::<ComAmazonawsGlueAwsglueState>()
        .lock()
        .workflows
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        held, before,
        "case listed twice: the store must hold exactly what was listed, got {held:?}"
    );

    // `DeleteWorkflow` is still a scaffold, so the record is dropped from the store the
    // listing reads rather than through that operation.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().workflows.remove("zeta");
    }
    expected.retain(|name| name != "zeta");
    let after = walk(&router, "after a record is dropped", None, created.len()).await;
    assert_eq!(
        after, expected,
        "case after a record is dropped: a workflow the store no longer holds must not be \
         listed, got {after:?}"
    );
}

/// A page size one past either published bound is refused by the modelled constraint
/// rather than served as some other page size; each bound itself is served.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    ok(
        &router,
        "CreateWorkflow",
        &json!({ "Name": "bounded-workflow" }),
    )
    .await;

    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 26)] {
        let (status, body) = call(
            &router,
            "ListWorkflows",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
    }

    for (case, max_results) in [("the minimum", 1), ("the maximum", MAX_PAGE_SIZE)] {
        let page = ok(
            &router,
            "ListWorkflows",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            listed_names(&page),
            vec!["bounded-workflow".to_string()],
            "case {case:?}: MaxResults={max_results} must be served, got {page}"
        );
    }
}

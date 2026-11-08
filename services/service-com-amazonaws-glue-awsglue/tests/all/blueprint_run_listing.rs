// SPDX-License-Identifier: CC0-1.0

//! `GetBlueprintRuns`: the runs of ONE blueprint, "the details of blueprint runs for a
//! specified blueprint".
//!
//! RULE: the operation reports every run the named blueprint holds, newest started first,
//! one `MaxResults`-sized page at a time with a `NextToken` that resumes the walk where the
//! previous page stopped, each run projected exactly as the single-run read projects it; a
//! blueprint this account holds none of is refused with the modelled
//! `EntityNotFoundException`, and the listing reports only, so the one state it materialises
//! is what the clock already owed a run.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many runs the blueprint holds,
//! the page size asked for (including each published bound of `MaxResults` and one past
//! each), whether the runs have reached their terminal state, what the starts carried, and
//! how a requested blueprint misses.
//!
//! The run-slice helpers this suite drives the service through are the ones
//! [`crate::blueprint_runs`] already defines, so a run is started and read back here exactly
//! as it is there.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    messages, ComAmazonawsGlueAwsglueState, BLUEPRINT_RUN_SETTLES_AFTER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::blueprint_runs::{emulator_with_engine, get_run, register_active, rewind, start};
use crate::wire::{call, error_code, error_message, ok};

/// The published bounds of `MaxResults`: `PageSize`, "range 1..1000" (`smithy_query
/// operation=GetBlueprintRuns`).
const MIN_PAGE_SIZE: i32 = 1;
const MAX_PAGE_SIZE: i32 = 1_000;

/// The `BlueprintRunState` a run is in until the clock owes it the next one, and the one it
/// owes a run that created the workflow it laid out: the modelled values "Running — The
/// blueprint run is in progress" and "Succeeded — The blueprint run completed successfully"
/// (`smithy_query shape=BlueprintRun`, member `State`).
const STATE_RUNNING: &str = "RUNNING";
const STATE_SUCCEEDED: &str = "SUCCEEDED";

/// An IAM role ARN of the form `OrchestrationIAMRoleArn`'s pattern requires.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GlueBlueprintRole";

/// The run ids one answer reports, in the order the service returned them.
fn listed_run_ids(page: &Value) -> Vec<String> {
    page["BlueprintRuns"]
        .as_array()
        .unwrap_or_else(|| panic!("BlueprintRuns must be a list, got {page}"))
        .iter()
        .map(|run| {
            run["RunId"]
                .as_str()
                .unwrap_or_else(|| panic!("BlueprintRun.RunId must be a string, got {run}"))
                .to_string()
        })
        .collect()
}

/// Start `count` runs of `blueprint`, each laying out a workflow of its own, and return the
/// ids the service minted, oldest first.
async fn start_runs(router: &MultiServiceRouter, blueprint: &str, count: usize) -> Vec<String> {
    let mut started = Vec::with_capacity(count);
    for index in 0..count {
        started.push(
            start(
                router,
                &json!({
                    "BlueprintName": blueprint,
                    "RoleArn": ROLE_ARN,
                    "Parameters": format!("{{\"WorkflowName\":\"{blueprint}-wf-{index:03}\"}}"),
                }),
            )
            .await,
        );
    }
    started
}

/// Give the runs of `blueprint` start instants one second apart, in the order they were
/// recorded, so the newest-first order the listing serves is fixed and this suite does not
/// depend on how close together two starts landed on the clock.
///
/// Only `StartedOn` is moved: what the clock owes each run stays where the start put it, so
/// the runs are still in flight.
fn spread_start_instants(engine: &Arc<engine::InMemoryEngine>, blueprint: &str) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .blueprint_mut(blueprint)
        .unwrap_or_else(|| panic!("{blueprint} was registered"));
    let base = SystemTime::now();
    let count = entry.runs.len();
    for (index, run) in entry.runs.iter_mut().enumerate() {
        let age = Duration::from_secs((count - index) as u64);
        run.started_on = base.checked_sub(age).expect("an instant in range");
    }
}

/// Walk every page the service hands out for `blueprint`, following its own `NextToken`, and
/// return the run ids in the order they were reported.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    blueprint: &str,
    page_size: Option<i32>,
    held: usize,
) -> Vec<String> {
    let mut listed: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "BlueprintName": blueprint });
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "GetBlueprintRuns", &request).await;
        let ids = listed_run_ids(&page);
        if let Some(size) = page_size {
            assert!(
                ids.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case:?}: a page must not exceed MaxResults {size}, got {ids:?}"
            );
        }
        listed.extend(ids);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            listed.len() <= held,
            "case {case:?}: the walk must terminate, got {listed:?} for {held} runs"
        );
    }
    listed
}

#[tokio::test]
async fn every_run_of_the_blueprint_is_listed_newest_first_and_paged() {
    // (case, how many runs the blueprint holds, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no run has been started", 0, None),
        ("one run, no page size", 1, None),
        // The published lower bound of `MaxResults` (`PageSize`, range 1..1000).
        ("the smallest page the model accepts", 5, Some(MIN_PAGE_SIZE)),
        ("a page smaller than the run count", 7, Some(3)),
        ("a page larger than the run count", 3, Some(50)),
        // The published upper bound, which holds the whole history in one page.
        ("the largest page the model accepts", 4, Some(MAX_PAGE_SIZE)),
        ("several pages of runs, five to a page", 12, Some(5)),
    ];

    for (case, held, page_size) in rows {
        let (router, engine) = emulator_with_engine();
        let blueprint = "paged-history";
        register_active(&router, &engine, blueprint).await;
        let started = start_runs(&router, blueprint, *held).await;
        spread_start_instants(&engine, blueprint);

        // Newest first: the last run started is the first reported.
        let expected: Vec<String> = started.iter().rev().cloned().collect();
        let listed = walk(&router, case, blueprint, *page_size, *held).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected every run of the blueprint newest first, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_token_follows_a_page_only_while_runs_remain() {
    let (router, engine) = emulator_with_engine();
    let blueprint = "tokenised";
    register_active(&router, &engine, blueprint).await;
    let started = start_runs(&router, blueprint, 4).await;
    spread_start_instants(&engine, blueprint);
    let newest_first: Vec<String> = started.iter().rev().cloned().collect();

    // (case, MaxResults, how many runs that page reports, whether a token follows)
    let rows: &[(&str, i32, usize, bool)] = &[
        ("one at a time", 1, 1, true),
        ("two at a time", 2, 2, true),
        ("exactly the run count", 4, 4, false),
        ("more than the run count", 10, 4, false),
    ];

    for (case, max_results, reported, more) in rows {
        let page = ok(
            &router,
            "GetBlueprintRuns",
            &json!({ "BlueprintName": blueprint, "MaxResults": max_results }),
        )
        .await;
        let ids = listed_run_ids(&page);
        assert_eq!(
            ids,
            newest_first[..*reported].to_vec(),
            "case {case:?}: expected the newest {reported} runs, got {page}"
        );
        assert_eq!(
            page.get("NextToken").and_then(Value::as_str).is_some(),
            *more,
            "case {case:?}: a token must be reported only while runs remain, got {page}"
        );
        // The token resumes the walk: the next page starts at the run after the last one
        // this page reported, and never repeats it.
        if let Some(token) = page.get("NextToken").and_then(Value::as_str) {
            let next = ok(
                &router,
                "GetBlueprintRuns",
                &json!({
                    "BlueprintName": blueprint,
                    "MaxResults": max_results,
                    "NextToken": token,
                }),
            )
            .await;
            let continued = listed_run_ids(&next);
            assert_eq!(
                continued.first().map(String::as_str),
                newest_first.get(*reported).map(String::as_str),
                "case {case:?}: the next page must resume after {ids:?}, got {continued:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_listed_run_is_the_run_the_single_read_reports() {
    let (router, engine) = emulator_with_engine();
    let blueprint = "projected";
    register_active(&router, &engine, blueprint).await;

    // The rows differ along what the start carried, so the projection is asserted over runs
    // that are not alike: a start with no `Parameters`, one naming the workflow it lays out,
    // and one whose parameters name none.
    let bodies: &[(&str, Value)] = &[
        (
            "required members only",
            json!({ "BlueprintName": blueprint, "RoleArn": ROLE_ARN }),
        ),
        (
            "parameters naming the workflow",
            json!({
                "BlueprintName": blueprint,
                "RoleArn": ROLE_ARN,
                "Parameters": r#"{"WorkflowName":"projected-ingest","DatabaseName":"sales"}"#,
            }),
        ),
        (
            "parameters naming no workflow",
            json!({
                "BlueprintName": blueprint,
                "RoleArn": ROLE_ARN,
                "Parameters": r#"{"DatabaseName":"sales"}"#,
            }),
        ),
    ];
    let mut cases: Vec<(&str, String)> = Vec::new();
    for (case, body) in bodies {
        cases.push((case, start(&router, body).await));
    }

    let page = ok(
        &router,
        "GetBlueprintRuns",
        &json!({ "BlueprintName": blueprint }),
    )
    .await;
    let listed = page["BlueprintRuns"]
        .as_array()
        .unwrap_or_else(|| panic!("BlueprintRuns must be a list, got {page}"))
        .clone();
    assert_eq!(
        listed.len(),
        cases.len(),
        "case every start: expected one listed run per start, got {page}"
    );

    for (case, run_id) in &cases {
        let read = get_run(&router, blueprint, run_id).await;
        let from_listing = listed
            .iter()
            .find(|run| run["RunId"].as_str() == Some(run_id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the listing must report run {run_id}, got {page}")
            });
        assert_eq!(
            from_listing, &read,
            "case {case:?}: the listed run must be the run the single read reports, got \
             {from_listing} and {read}"
        );
        assert_eq!(
            from_listing["BlueprintName"].as_str(),
            Some(blueprint),
            "case {case:?}: a listed run must carry the blueprint it is of, got {from_listing}"
        );
    }
}

#[tokio::test]
async fn the_listing_reports_the_state_the_clock_owes_and_changes_nothing_else() {
    let (router, engine) = emulator_with_engine();
    let blueprint = "settling";
    register_active(&router, &engine, blueprint).await;
    let started = start_runs(&router, blueprint, 2).await;

    // (case, how long ago each run started, the state the listing must report, whether the
    // run carries a completion instant)
    let rows: &[(&str, Duration, &str, bool)] = &[
        ("read straight after the start", Duration::ZERO, STATE_RUNNING, false),
        (
            "read once the clock owes the run its next state",
            BLUEPRINT_RUN_SETTLES_AFTER,
            STATE_SUCCEEDED,
            true,
        ),
    ];

    for (case, elapsed, state, completed) in rows {
        if !elapsed.is_zero() {
            rewind(&engine, blueprint, *elapsed);
        }
        let page = ok(
            &router,
            "GetBlueprintRuns",
            &json!({ "BlueprintName": blueprint }),
        )
        .await;
        let runs = page["BlueprintRuns"]
            .as_array()
            .unwrap_or_else(|| panic!("BlueprintRuns must be a list, got {page}"))
            .clone();
        assert_eq!(
            runs.len(),
            started.len(),
            "case {case:?}: expected every started run, got {page}"
        );
        for run in &runs {
            assert_eq!(
                run["State"].as_str(),
                Some(*state),
                "case {case:?}: expected state {state}, got {run}"
            );
            assert_eq!(
                run.get("CompletedOn").is_some(),
                *completed,
                "case {case:?}: a completion instant is reported only once the run finished, \
                 got {run}"
            );
        }

        // Reporting only: a second listing agrees with the first, so the read moved nothing
        // the clock had not already made due.
        let again = ok(
            &router,
            "GetBlueprintRuns",
            &json!({ "BlueprintName": blueprint }),
        )
        .await;
        assert_eq!(
            again, page,
            "case {case:?}: two listings in a row must agree, got {again} then {page}"
        );
    }
}

#[tokio::test]
async fn listing_the_runs_of_an_unknown_blueprint_is_refused() {
    let (router, engine) = emulator_with_engine();
    register_active(&router, &engine, "registered").await;

    // The rows differ along how the requested name misses: never registered, a name that
    // differs from a registered one only in case, and the longest name the request admits
    // (`BlueprintName` is `NameString`, len 1..255).
    let long_name = "n".repeat(255);
    let rows: &[(&str, &str)] = &[
        ("never registered", "absent"),
        ("differs only in case", "REGISTERED"),
        ("the longest name the request admits", long_name.as_str()),
    ];

    for (case, name) in rows {
        let (status, body) = call(
            &router,
            "GetBlueprintRuns",
            &json!({ "BlueprintName": name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a blueprint nothing registered, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            messages::BLUEPRINT_NOT_FOUND,
            "case {case:?}: expected the recorded blueprint miss, got {body}"
        );
    }
}

#[tokio::test]
async fn a_blueprint_no_run_has_been_started_of_reports_an_empty_listing() {
    let (router, engine) = emulator_with_engine();
    let blueprint = "never-run";
    register_active(&router, &engine, blueprint).await;

    let page = ok(
        &router,
        "GetBlueprintRuns",
        &json!({ "BlueprintName": blueprint }),
    )
    .await;
    assert_eq!(
        listed_run_ids(&page),
        Vec::<String>::new(),
        "case no run started: expected no run, got {page}"
    );
    assert!(
        page.get("NextToken").is_none(),
        "case no run started: no token follows a listing that reached its end, got {page}"
    );
}

#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let (router, engine) = emulator_with_engine();
    let blueprint = "bounded";
    register_active(&router, &engine, blueprint).await;
    let started = start_runs(&router, blueprint, 1).await;

    // `MaxResults` is `PageSize`, range 1..1000: one past each published bound is refused by
    // the modelled constraint rather than served as some other page size.
    for (case, max_results) in [
        ("below the minimum", MIN_PAGE_SIZE - 1),
        ("above the maximum", MAX_PAGE_SIZE + 1),
    ] {
        let (status, body) = call(
            &router,
            "GetBlueprintRuns",
            &json!({ "BlueprintName": blueprint, "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected the boundary's ValidationException for \
             MaxResults={max_results}, got {} with {body}",
            error_code(&body)
        );
    }

    // Each published bound itself is served.
    for (case, max_results) in [("the minimum", MIN_PAGE_SIZE), ("the maximum", MAX_PAGE_SIZE)] {
        let page = ok(
            &router,
            "GetBlueprintRuns",
            &json!({ "BlueprintName": blueprint, "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            listed_run_ids(&page),
            started,
            "case {case:?}: MaxResults={max_results} must be served, got {page}"
        );
    }
}

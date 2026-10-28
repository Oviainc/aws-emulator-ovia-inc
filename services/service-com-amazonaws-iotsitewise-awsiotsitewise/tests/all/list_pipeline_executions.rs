// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListPipelineExecutions`, driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with, so every assertion is about
//! the wire behaviour a real client sees.
//!
//! The rules these suites pin, each over a SET of executions rather than one:
//!
//! * the listing enumerates exactly the executions the addressed pipeline of the
//!   addressed workspace holds, reporting for each the values a read of that one
//!   execution reports — never a fabricated one, and never an execution of another
//!   pipeline;
//! * `state` selects exactly the executions standing in that `PipelineExecutionState`
//!   and a request naming none returns executions in all states;
//! * a `startTime` bound selects on the instant an execution started — inclusive
//!   lower, exclusive upper — and an `endTime` bound on the instant it ended, so an
//!   execution that has not reached the bounded instant is never selected;
//! * the two time-range filter GROUPS cannot be combined: a request naming a bound
//!   from each is the modelled `InvalidRequestException`;
//! * the enumeration pages: `maxResults` bounds a page, `nextToken` resumes the same
//!   order, and a token this listing never issued is a refusal rather than an empty
//!   page;
//! * a workspace or pipeline this service does not hold is the modelled not-found
//!   refusal, while a pipeline that holds no execution is an empty page.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`, `hex_token`) and the
//! pipeline helpers (`create_workspace`, `create_task`, `create_pipeline`,
//! `start_execution`, `start_plain`, `describe_execution`, `Node`), together with the
//! two store writers the cancellation suite already has (`set_run_state`,
//! `set_cancel_requested_at`), are the ones the sibling suites define; this suite
//! reuses them rather than growing a second copy.

use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, ACTIVATION_DELAY};

use crate::asset_lifecycle::{call, error_code, hex_token, router_with_engine};
use crate::hierarchies_etags_and_refusals::error_message;
use crate::pipeline_execution_cancellation::{set_cancel_requested_at, set_run_state};
use crate::pipeline_executions::{
    create_pipeline, create_task, create_workspace, describe_execution, start_execution,
    start_plain, Node,
};

/// The workspace every suite below works in.
const WORKSPACE: &str = "list-executions-workspace";

/// The pipeline every suite that needs only one starts its executions of.
const PIPELINE: &str = "hourly-rollup";

/// A second pipeline of the same workspace, so the listing can be shown to
/// enumerate one pipeline's executions and not another's.
const OTHER_PIPELINE: &str = "weekly-rollup";

/// The task every pipeline below runs on its only compute node.
const TASK: &str = "transform";

/// Every value of `PipelineExecutionState`, the enum the `state` filter takes.
///
/// Source: `smithy_query(query="shape", shape="PipelineExecutionState")` —
/// NOT_STARTED | RUNNING | SUCCEEDED | FAILED | CANCELLING | CANCELLED.
const EXECUTION_STATES: [&str; 6] = [
    "NOT_STARTED",
    "RUNNING",
    "SUCCEEDED",
    "FAILED",
    "CANCELLING",
    "CANCELLED",
];

/// The published maximum of `maxResults` (`@range(min: 1, max: 250)`;
/// `smithy_query(query="operation", operation="ListPipelineExecutions")`).
const MAX_RESULTS_MAX: usize = 250;

/// The page size the listing serves when the caller names no `maxResults`, which
/// the member's own model declares ("Default: 50").
const DEFAULT_MAX_RESULTS: usize = 50;

/// `GET /workspaces/{workspaceName}/pipelines/{pipelineName}/executions`, with
/// `query` (already in `a=b&c=d` form, possibly empty) appended.
fn list_uri(workspace: &str, pipeline: &str, query: &str) -> String {
    let base = format!("/workspaces/{workspace}/pipelines/{pipeline}/executions");
    if query.is_empty() {
        base
    } else {
        format!("{base}?{query}")
    }
}

/// `ListPipelineExecutions`, returning the raw `(status, body)` so a row can
/// assert a refusal too.
async fn list(
    router: &MultiServiceRouter,
    workspace: &str,
    pipeline: &str,
    query: &str,
) -> (u16, Value) {
    call(router, "GET", &list_uri(workspace, pipeline, query), None).await
}

/// The same, asserting the listing was served.
async fn listed(router: &MultiServiceRouter, query: &str) -> Value {
    let (status, body) = list(router, WORKSPACE, PIPELINE, query).await;
    assert_eq!(
        status, 200,
        "case {query:?}: expected 200, got {status} {body}"
    );
    body
}

/// The execution ids one answer reports, in the order it reported them.
fn listed_ids(body: &Value) -> Vec<String> {
    body["pipelineExecutionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a pipelineExecutionSummaries list, got {body}"))
        .iter()
        .map(|summary| {
            summary["pipelineExecutionId"]
                .as_str()
                .unwrap_or_else(|| panic!("expected an execution id in {summary}"))
                .to_string()
        })
        .collect()
}

/// The same, sorted, so a row can compare SETS where the order is not what it is
/// about.
fn listed_id_set(body: &Value) -> Vec<String> {
    let mut ids = listed_ids(body);
    ids.sort();
    ids
}

/// `ids`, sorted, as the oracle a set comparison is made against.
fn id_set(ids: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = ids.iter().map(|id| (*id).to_string()).collect();
    sorted.sort();
    sorted
}

/// The RFC 3339 form a query-bound timestamp takes on the wire: the time-range
/// members carry no `@timestampFormat`, so an `@httpQuery` timestamp uses Smithy's
/// default `date-time` format.
fn query_date(at: SystemTime) -> String {
    DateTime::from(at)
        .fmt(Format::DateTime)
        .expect("a formattable timestamp")
}

/// Put the instants a stored execution's own run began and finished on it, so the
/// time-range rows below have executions at known instants to bound.
fn set_run_window(
    engine: &engine::InMemoryEngine,
    execution_id: &str,
    start_time: Option<SystemTime>,
    end_time: Option<SystemTime>,
) {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    let execution = store
        .pipeline_executions
        .get_mut(execution_id)
        .unwrap_or_else(|| panic!("the store holds {execution_id}"));
    execution.start_time = start_time;
    execution.end_time = end_time;
}

/// The workspace, the task and the two pipelines every suite below starts
/// executions of.
async fn fixture(router: &MultiServiceRouter) {
    create_workspace(router, WORKSPACE).await;
    create_task(router, WORKSPACE, TASK, &[], &[]).await;
    for pipeline in [PIPELINE, OTHER_PIPELINE] {
        let (status, body) = create_pipeline(
            router,
            WORKSPACE,
            pipeline,
            &[],
            &[Node::new("only", TASK)],
        )
        .await;
        assert_eq!(
            status, 200,
            "create pipeline {pipeline}: got {status} {body}"
        );
    }
}

#[tokio::test]
async fn every_execution_of_the_pipeline_is_listed_as_a_read_of_it_reports_it() {
    let (router, _engine) = router_with_engine();
    fixture(&router).await;

    // The rows differ in the `executionPriority` the start named — each published
    // bound of the modelled range, the value between them, and none at all — so the
    // listing is not four copies of one execution.
    let priorities: [(&str, Option<i64>); 4] = [
        ("no priority", None),
        ("the lowest priority", Some(0)),
        ("the middle priority", Some(1)),
        ("the highest priority", Some(2)),
    ];
    let mut started: Vec<(&str, String)> = Vec::new();
    for (case, priority) in priorities {
        let body = match priority {
            Some(priority) => json!({ "executionPriority": priority }),
            None => json!({}),
        };
        let (status, answer) = start_execution(&router, WORKSPACE, PIPELINE, body).await;
        assert_eq!(status, 202, "case {case:?}: start got {status} {answer}");
        started.push((
            case,
            answer["pipelineExecutionId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no execution id in {answer}"))
                .to_string(),
        ));
    }

    // An execution of the OTHER pipeline, which this listing must not report.
    let elsewhere = start_plain(&router, WORKSPACE, OTHER_PIPELINE).await;

    let body = listed(&router, &format!("maxResults={MAX_RESULTS_MAX}")).await;
    let reported = listed_id_set(&body);
    let expected = id_set(
        &started
            .iter()
            .map(|(_, id)| id.as_str())
            .collect::<Vec<&str>>(),
    );
    assert_eq!(
        reported, expected,
        "the listing reports every execution of the pipeline exactly once, got {reported:?}"
    );
    assert!(
        !reported.contains(&elsewhere),
        "case \"another pipeline's execution\": {elsewhere} must not be listed, got {reported:?}"
    );

    // Every member of a summary is the one a read of that execution reports.
    let summaries = body["pipelineExecutionSummaries"]
        .as_array()
        .expect("a summary list");
    for (case, id) in &started {
        let summary = summaries
            .iter()
            .find(|summary| summary["pipelineExecutionId"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: no summary for {id} in {body}"));
        let (status, read) = describe_execution(&router, WORKSPACE, PIPELINE, id, "").await;
        assert_eq!(status, 200, "case {case:?}: describe got {status} {read}");
        for member in [
            "pipelineVersion",
            "executionPriority",
            "startTime",
            "endTime",
        ] {
            assert_eq!(
                summary[member], read[member],
                "case {case:?}: the summary's {member} must be the read's, got {summary} \
                 against {read}"
            );
        }
        assert_eq!(
            summary["status"], read["status"],
            "case {case:?}: the summary's status must be the read's, got {summary} against {read}"
        );
    }

    // A read changes nothing a later caller sees.
    let again = listed(&router, &format!("maxResults={MAX_RESULTS_MAX}")).await;
    assert_eq!(
        again, body,
        "a second listing answers the same, got {again} against {body}"
    );
}

#[tokio::test]
async fn the_state_filter_selects_exactly_the_executions_in_that_state() {
    let (router, engine) = router_with_engine();
    fixture(&router).await;

    // One execution per value of the modelled enum: the four a run reaches, set on
    // the stored record because no container runs here, and the two a recorded
    // cancellation reaches, put at the instant each is due.
    let mut by_state: Vec<(&str, String)> = Vec::new();
    for state in EXECUTION_STATES {
        let id = start_plain(&router, WORKSPACE, PIPELINE).await;
        match state {
            // `CANCELLING` is the TRANSIENT half of a cancellation; its recorded
            // instant is put on the record immediately before each listing below,
            // so the row reports the state the clock has not settled yet whatever
            // the suite's wall-clock cost.
            "CANCELLING" => {}
            "CANCELLED" => {
                set_cancel_requested_at(
                    &engine,
                    &id,
                    Some(SystemTime::now() - 4 * ACTIVATION_DELAY),
                );
            }
            run_state => set_run_state(&engine, &id, run_state),
        }
        by_state.push((state, id));
    }

    // The id of the row standing in `CANCELLING`, the TRANSIENT half of a
    // cancellation: the clock settles it `ACTIVATION_DELAY` after the cancel was
    // recorded, so its recorded instant is refreshed before EVERY listing below.
    // That holds the row open for the whole table whatever the suite costs in wall
    // time, and keeps it out of the `CANCELLED` row's set.
    let cancelling = by_state
        .iter()
        .find(|(state, _)| *state == "CANCELLING")
        .map(|(_, id)| id.clone())
        .expect("the table holds a CANCELLING row");

    for (state, id) in &by_state {
        set_cancel_requested_at(&engine, &cancelling, Some(SystemTime::now()));
        let body = listed(&router, &format!("state={state}&maxResults={MAX_RESULTS_MAX}")).await;
        assert_eq!(
            listed_id_set(&body),
            id_set(&[id.as_str()]),
            "case {state:?}: the filter selects exactly the executions in that state, got {body}"
        );
        let reported = body["pipelineExecutionSummaries"][0]["status"]["state"].as_str();
        assert_eq!(
            reported,
            Some(*state),
            "case {state:?}: a selected summary reports that state, got {body}"
        );
    }

    // "If not specified, executions in all states are returned."
    let body = listed(&router, &format!("maxResults={MAX_RESULTS_MAX}")).await;
    assert_eq!(
        listed_id_set(&body),
        id_set(
            &by_state
                .iter()
                .map(|(_, id)| id.as_str())
                .collect::<Vec<&str>>()
        ),
        "no state filter returns executions in all states, got {body}"
    );

    // A pipeline holding no execution is an EMPTY page, whatever the filter — an
    // absence of executions is not an absence of the pipeline.
    for state in EXECUTION_STATES {
        let (status, body) = list(
            &router,
            WORKSPACE,
            OTHER_PIPELINE,
            &format!("state={state}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {state:?}: a pipeline with no executions is served, got {status} {body}"
        );
        assert!(
            listed_ids(&body).is_empty(),
            "case {state:?}: a pipeline with no executions reports none, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {state:?}: an exhausted page issues no token, got {body}"
        );
    }
}

/// One execution of the time-range fixture: the name a row refers to it by, how
/// long before `now` its run started, how long before `now` it ended, and the
/// `PipelineExecutionState` its run stands in.
struct Windowed {
    label: &'static str,
    started_ago: Option<Duration>,
    ended_ago: Option<Duration>,
    run_state: &'static str,
}

/// The time-range fixture: four executions at known instants, and the id each is
/// stored under, by label.
async fn windowed_fixture(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    now: SystemTime,
) -> Vec<(&'static str, String)> {
    // The rows span the forms an execution's window takes: three finished runs at
    // distinct instants, and one that has not started, which no bound on an instant
    // can select.
    let rows = [
        Windowed {
            label: "oldest",
            started_ago: Some(Duration::from_secs(300)),
            ended_ago: Some(Duration::from_secs(280)),
            run_state: "SUCCEEDED",
        },
        Windowed {
            label: "middle",
            started_ago: Some(Duration::from_secs(200)),
            ended_ago: Some(Duration::from_secs(180)),
            run_state: "SUCCEEDED",
        },
        Windowed {
            label: "newest",
            started_ago: Some(Duration::from_secs(100)),
            ended_ago: Some(Duration::from_secs(80)),
            run_state: "FAILED",
        },
        Windowed {
            label: "never started",
            started_ago: None,
            ended_ago: None,
            run_state: "NOT_STARTED",
        },
    ];
    let mut ids: Vec<(&'static str, String)> = Vec::new();
    for row in rows {
        let id = start_plain(router, WORKSPACE, PIPELINE).await;
        set_run_state(engine, &id, row.run_state);
        set_run_window(
            engine,
            &id,
            row.started_ago.map(|ago| now - ago),
            row.ended_ago.map(|ago| now - ago),
        );
        ids.push((row.label, id));
    }
    ids
}

/// The id the time-range fixture stored under `label`.
fn id_of<'a>(ids: &'a [(&'static str, String)], label: &str) -> &'a str {
    ids.iter()
        .find(|(known, _)| *known == label)
        .map(|(_, id)| id.as_str())
        .unwrap_or_else(|| panic!("the fixture holds no execution labelled {label}"))
}

#[tokio::test]
async fn a_time_range_filter_bounds_the_instant_it_names() {
    let (router, engine) = router_with_engine();
    fixture(&router).await;
    let now = SystemTime::now();
    let ids = windowed_fixture(&router, &engine, now).await;

    let start_of = |ago: u64| query_date(now - Duration::from_secs(ago));
    let oldest = id_of(&ids, "oldest");
    let middle = id_of(&ids, "middle");
    let newest = id_of(&ids, "newest");

    // The rows differ in WHICH bound is named and where it falls relative to the
    // fixture's instants: on an execution's own instant, one second either side of
    // it, a window of both bounds of one group, and a bound combined with `state`.
    // `startTimeAfter` is the INCLUSIVE lower bound and `startTimeBefore` the
    // EXCLUSIVE upper one, and the same for the endTime pair; an execution that
    // never started, and one that never ended, is selected by no bound at all.
    let cases: [(&str, String, Vec<&str>); 12] = [
        (
            "startTimeAfter on the oldest start, inclusive",
            format!("startTimeAfter={}", start_of(300)),
            vec![oldest, middle, newest],
        ),
        (
            "startTimeAfter one second past the oldest start",
            format!("startTimeAfter={}", start_of(299)),
            vec![middle, newest],
        ),
        (
            "startTimeAfter past every start",
            format!("startTimeAfter={}", start_of(1)),
            vec![],
        ),
        (
            "startTimeBefore the newest start, exclusive",
            format!("startTimeBefore={}", start_of(100)),
            vec![oldest, middle],
        ),
        (
            "startTimeBefore one second past the newest start",
            format!("startTimeBefore={}", start_of(99)),
            vec![oldest, middle, newest],
        ),
        (
            "startTimeBefore the oldest start, exclusive",
            format!("startTimeBefore={}", start_of(300)),
            vec![],
        ),
        (
            "a window of both startTime bounds",
            format!(
                "startTimeAfter={}&startTimeBefore={}",
                start_of(300),
                start_of(100)
            ),
            vec![oldest, middle],
        ),
        (
            "endTimeAfter on the oldest end, inclusive",
            format!("endTimeAfter={}", start_of(280)),
            vec![oldest, middle, newest],
        ),
        (
            "endTimeAfter one second past the oldest end",
            format!("endTimeAfter={}", start_of(279)),
            vec![middle, newest],
        ),
        (
            "endTimeBefore the newest end, exclusive",
            format!("endTimeBefore={}", start_of(80)),
            vec![oldest, middle],
        ),
        (
            "a window of both endTime bounds",
            format!(
                "endTimeAfter={}&endTimeBefore={}",
                start_of(280),
                start_of(80)
            ),
            vec![oldest, middle],
        ),
        (
            "a startTime bound combined with state",
            format!("state=SUCCEEDED&startTimeAfter={}", start_of(300)),
            vec![oldest, middle],
        ),
    ];

    for (case, query, expected) in cases {
        let body = listed(&router, &format!("{query}&maxResults={MAX_RESULTS_MAX}")).await;
        assert_eq!(
            listed_id_set(&body),
            id_set(&expected),
            "case {case:?}: expected {expected:?}, got {body}"
        );
        assert!(
            !listed_ids(&body).contains(&id_of(&ids, "never started").to_string()),
            "case {case:?}: an execution at no instant is selected by no bound, got {body}"
        );
    }

    // An execution whose run has not ended has no `endTime`, so "endTime filters
    // only return executions in terminal states": a bound that spans every instant
    // still leaves it out.
    let in_flight = start_plain(&router, WORKSPACE, PIPELINE).await;
    let body = listed(
        &router,
        &format!(
            "endTimeAfter={}&maxResults={MAX_RESULTS_MAX}",
            start_of(100_000)
        ),
    )
    .await;
    assert!(
        !listed_ids(&body).contains(&in_flight),
        "an execution with no endTime is selected by no endTime bound, got {body}"
    );
}

#[tokio::test]
async fn the_two_time_range_filter_groups_cannot_be_combined() {
    let (router, engine) = router_with_engine();
    fixture(&router).await;
    let now = SystemTime::now();
    windowed_fixture(&router, &engine, now).await;
    let at = |ago: u64| query_date(now - Duration::from_secs(ago));

    // Every cross pair of the two groups, and all four bounds at once: "Combining
    // startTime and endTime filters returns an InvalidRequestException."
    let refused: [(&str, String); 5] = [
        (
            "startTimeAfter with endTimeAfter",
            format!("startTimeAfter={}&endTimeAfter={}", at(300), at(280)),
        ),
        (
            "startTimeAfter with endTimeBefore",
            format!("startTimeAfter={}&endTimeBefore={}", at(300), at(80)),
        ),
        (
            "startTimeBefore with endTimeAfter",
            format!("startTimeBefore={}&endTimeAfter={}", at(100), at(280)),
        ),
        (
            "startTimeBefore with endTimeBefore",
            format!("startTimeBefore={}&endTimeBefore={}", at(100), at(80)),
        ),
        (
            "all four bounds at once",
            format!(
                "startTimeAfter={}&startTimeBefore={}&endTimeAfter={}&endTimeBefore={}",
                at(300),
                at(100),
                at(280),
                at(80)
            ),
        ),
    ];
    for (case, query) in refused {
        let (status, body) = list(&router, WORKSPACE, PIPELINE, &query).await;
        assert_eq!(status, 400, "case {case:?}: got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: got {body}"
        );
        assert!(
            error_message(&body).contains("startTime"),
            "case {case:?}: the refusal names the grouped filters, got {body}"
        );
    }

    // Either group ON ITS OWN, and a `state` filter beside it, is served: it is the
    // combination that is refused, not the bounds.
    let served: [(&str, String); 4] = [
        ("the startTime group", format!("startTimeAfter={}", at(300))),
        (
            "both startTime bounds",
            format!("startTimeAfter={}&startTimeBefore={}", at(300), at(100)),
        ),
        ("the endTime group", format!("endTimeBefore={}", at(80))),
        (
            "the endTime group with state",
            format!("state=SUCCEEDED&endTimeAfter={}", at(280)),
        ),
    ];
    for (case, query) in served {
        let (status, body) = list(&router, WORKSPACE, PIPELINE, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: one group alone is served, got {status} {body}"
        );
    }

    // A range whose upper bound precedes its lower one names no instant at all.
    for (case, query) in [
        (
            "an inverted startTime window",
            format!("startTimeAfter={}&startTimeBefore={}", at(100), at(300)),
        ),
        (
            "an inverted endTime window",
            format!("endTimeAfter={}&endTimeBefore={}", at(80), at(280)),
        ),
    ] {
        let (status, body) = list(&router, WORKSPACE, PIPELINE, &query).await;
        assert_eq!(status, 400, "case {case:?}: got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: got {body}"
        );
    }
}

/// How many executions the paging fixture starts.
const ENTRIES: usize = 7;

#[tokio::test]
async fn the_executions_page_one_stable_enumeration_whatever_the_page_size() {
    let (router, _engine) = router_with_engine();
    fixture(&router).await;
    for _ in 0..ENTRIES {
        start_plain(&router, WORKSPACE, PIPELINE).await;
    }

    let whole = listed(&router, &format!("maxResults={MAX_RESULTS_MAX}")).await;
    let reference = listed_ids(&whole);
    assert_eq!(
        reference.len(),
        ENTRIES,
        "the whole enumeration is {ENTRIES} executions, got {reference:?}"
    );
    assert!(
        whole["nextToken"].is_null(),
        "a page that exhausts the enumeration issues no token, got {whole}"
    );

    // The rows differ in page size: the published minimum, sizes that divide the
    // enumeration and sizes that do not, exactly it, one past it, the published
    // maximum, and no `maxResults` at all (the model's own default of 50).
    let page_sizes = [
        Some(1_usize),
        Some(2),
        Some(3),
        Some(ENTRIES),
        Some(ENTRIES + 1),
        Some(MAX_RESULTS_MAX),
        None,
    ];
    for page_size in page_sizes {
        let case = format!("page size {page_size:?}");
        let limit = page_size.unwrap_or(DEFAULT_MAX_RESULTS);
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = &token {
                query.push(format!("nextToken={token}"));
            }
            let body = listed(&router, &query.join("&")).await;
            let reported = listed_ids(&body);
            pages += 1;
            assert!(
                reported.len() <= limit,
                "case {case:?}: a page holds at most {limit} executions, got {} in {body}",
                reported.len()
            );
            seen.extend(reported.iter().cloned());
            token = body["nextToken"].as_str().map(str::to_string);
            match &token {
                // A token is issued only while another execution follows the page,
                // and such a page is a FULL page.
                Some(_) => assert_eq!(
                    reported.len(),
                    limit,
                    "case {case:?}: a page followed by a nextToken must be full, got {body}"
                ),
                None => break,
            }
            assert!(
                pages <= ENTRIES + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            seen, reference,
            "case {case:?}: the pages joined must be the whole enumeration once, in one order, \
             got {seen:?}"
        );
        let expected_pages = ENTRIES.div_ceil(limit);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages over {ENTRIES} executions, walked \
             {pages}"
        );
    }

    // The token a page issues resumes at the execution AFTER it.
    let first = listed(&router, "maxResults=1").await;
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a first page of one of {ENTRIES} carries a token, got {first}"));
    let resumed = listed(
        &router,
        &format!("maxResults={MAX_RESULTS_MAX}&nextToken={token}"),
    )
    .await;
    assert_eq!(
        listed_ids(&resumed),
        reference[1..].to_vec(),
        "the token resumes at the second execution, got {resumed}"
    );
}

#[tokio::test]
async fn a_token_this_listing_never_issued_is_refused() {
    let (router, _engine) = router_with_engine();
    fixture(&router).await;
    let mine = start_plain(&router, WORKSPACE, PIPELINE).await;
    // An execution of another pipeline: its id is a well-formed resume key of no
    // page of THIS enumeration.
    let elsewhere = start_plain(&router, WORKSPACE, OTHER_PIPELINE).await;

    // The rows differ in HOW the token is wrong: not hex at all, an odd-length hex
    // run, the hex of an id nothing minted, and the hex of an execution id this
    // enumeration does not hold.
    let forged: [(&str, String); 4] = [
        ("not hex at all", "zzzz".to_string()),
        ("an odd-length hex run", "abc".to_string()),
        (
            "the hex of an id nothing minted",
            hex_token("11111111-2222-3333-4444-555555555555"),
        ),
        (
            "the hex of another pipeline's execution",
            hex_token(&elsewhere),
        ),
    ];
    for (case, token) in forged {
        let (status, body) = list(
            &router,
            WORKSPACE,
            PIPELINE,
            &format!("nextToken={token}"),
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: got {body}"
        );
    }

    // The refusals left the enumeration as it was.
    let body = listed(&router, &format!("maxResults={MAX_RESULTS_MAX}")).await;
    assert_eq!(
        listed_ids(&body),
        vec![mine.clone()],
        "the refusals leave the enumeration as it was, got {body}"
    );

    // The token this listing DID issue resumes it.
    let token = hex_token(&mine);
    let (status, body) = list(
        &router,
        WORKSPACE,
        PIPELINE,
        &format!("nextToken={token}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "a token naming an execution of this enumeration resumes it, got {status} {body}"
    );
    assert_eq!(
        listed_ids(&body),
        vec![mine],
        "the resumed page starts at the execution the token names, got {body}"
    );
}

#[tokio::test]
async fn a_workspace_or_pipeline_this_service_does_not_hold_is_refused() {
    let (router, _engine) = router_with_engine();
    fixture(&router).await;
    start_plain(&router, WORKSPACE, PIPELINE).await;

    // The rows span the ways the request addresses no pipeline of this service, and
    // which absence each reports: the outermost one.
    let cases: [(&str, &str, &str, String); 3] = [
        (
            "a workspace nothing created",
            "no-such-workspace",
            PIPELINE,
            "Workspace with ID no-such-workspace not found".to_string(),
        ),
        (
            "a workspace nothing created and a pipeline it could not hold",
            "no-such-workspace",
            "no-such-pipeline",
            "Workspace with ID no-such-workspace not found".to_string(),
        ),
        (
            "a pipeline the workspace does not hold",
            WORKSPACE,
            "no-such-pipeline",
            "Pipeline with ID no-such-pipeline not found".to_string(),
        ),
    ];
    for (case, workspace, pipeline, expected) in cases {
        let (status, body) = list(&router, workspace, pipeline, "").await;
        assert_eq!(status, 404, "case {case:?}: got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: got {body}"
        );
    }
}

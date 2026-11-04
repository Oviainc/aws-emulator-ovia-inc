// SPDX-License-Identifier: CC0-1.0

//! `ListExperimentRuns` as a REPORT over stored state: it answers one summary per
//! experiment run the addressed experiment definition holds, newest run first, narrowed by
//! `Status`, cut into pages a `NextToken` walks exactly once.
//!
//! Every case drives real HTTP requests through the router the emulator's front door uses,
//! so what is asserted is what an AppConfig client sees on the wire. The rules under test,
//! each asserted over a SET of inputs rather than over one recorded call:
//!
//! 1. a summary reports the stored run — `ExperimentDefinitionId`, `Run`, `Description`,
//!    `Status`, `StartedAt`, `UpdatedAt` and `EndedAt` exactly as `GetExperimentRun`
//!    reports them — and carries no member outside that modelled set;
//! 2. a listing reports exactly the runs of the definition it addresses, in descending run
//!    order, however that definition and its application are addressed (id or name);
//! 3. `Status` narrows the listing to the runs holding that status;
//! 4. `MaxResults` bounds a page and `NextToken` resumes after it, so a walk reports every
//!    run exactly once and still newest first, at any page size;
//! 5. an application or a definition that addresses nothing is 404
//!    `ResourceNotFoundException`, a `NextToken` this listing did not issue is 400
//!    `BadRequestException`, and a `MaxResults` outside the modelled range is refused;
//! 6. listing is a read: it changes nothing a later caller sees.
//!
//! The tables span what those rules generalise over: HOW MANY runs the definition holds
//! (none, one, several, and more than a page), WHICH status each run is in, WHICH page size
//! the request asks for (absent, the published bounds of `MaxResults`, values in between
//! and one past each bound), WHETHER a run carries a description and at which published
//! length bound, and WHICH level of the path resolves to nothing.
//!
//! A run history longer than one run is seeded through this service's own state: the only
//! writer that creates a run is `StartExperimentRun`, a definition serves one run at a
//! time, and the operation that ends a run is still a scaffold stub — so no sequence of
//! requests yet leaves a definition holding a finished run. What these rules are about is
//! what the LISTING reports over a history, not how the history was written, and every
//! listing, read and refusal below goes over the wire.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::EXPERIMENT_RUN_STATUS_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    self, ExperimentRun, MAX_LIST_PAGE_SIZE,
};

use crate::appconfig_workflow::{create_application, json_call, router_on, Answer};
use crate::experiment_definitions::{scope, Scope};
use crate::resource_tags::{filler, pct};
use crate::start_experiment_run::{create_test_definition, get_run, id_of, start_run};

/// The members `ExperimentRunSummary` carries
/// (`smithy_query(query="shape",shape="ExperimentRunSummary")`). A summary reports each of
/// them off the stored run, and nothing else.
const SUMMARY_MEMBERS: &[&str] = &[
    "ExperimentDefinitionId",
    "Run",
    "Description",
    "Status",
    "StartedAt",
    "UpdatedAt",
    "EndedAt",
];

/// The published bounds of this operation's `MaxResults`
/// (`range 1..50`, `smithy_query(query="operation",operation="ListExperimentRuns")`), read
/// off the service's own constant for the upper one so a case cannot drift from it.
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = MAX_LIST_PAGE_SIZE as i64;

/// How far apart the seeded runs of one definition are started, so each run's instants are
/// its own and a listing that reported another run's would be caught.
const RUN_SPACING: Duration = Duration::from_secs(3_600);

/// A router over an engine the suite keeps a handle on, so a case can seed a run history
/// in this service's state and then read it back over the wire.
fn engine_and_router() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    (engine, router)
}

/// `GET /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}/experimentruns`
/// with `query` as the request's `httpQuery` members (`max_results`, `next_token`,
/// `status`). `pub(crate)` because more than one suite in this target reads a definition's
/// run history.
pub(crate) async fn list(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    query: &[(&str, String)],
) -> Answer {
    let mut uri = format!(
        "/applications/{}/experimentdefinitions/{}/experimentruns",
        pct(application),
        pct(definition)
    );
    if !query.is_empty() {
        let rendered: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={}", pct(value)))
            .collect();
        uri.push('?');
        uri.push_str(&rendered.join("&"));
    }
    json_call(router, "GET", &uri, &Value::Null).await
}

/// The `Items` of a successful listing.
fn items(case: &str, answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListExperimentRuns expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no Items array in {}", answer.json()))
}

/// The `Run` number of every summary in `page`, in the order they were served.
fn numbers(case: &str, page: &[Value]) -> Vec<i64> {
    page.iter()
        .map(|summary| {
            summary
                .get("Run")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| panic!("case {case:?}: a summary carries no Run: {summary}"))
        })
        .collect()
}

/// Write `descriptions.len()` FINISHED runs into the history of the definition
/// `definition_identifier` addresses, numbered after the runs it already holds, and answer
/// the numbers they were given.
///
/// Each run is `DONE` with an end instant, carries the row's description, and holds the
/// definition's own snapshot — the record `StartExperimentRun` writes, moved on to the
/// status the end of a run leaves. See this module's header for why the history is seeded
/// rather than driven over the wire.
fn seed_finished_runs(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    application_identifier: &str,
    definition_identifier: &str,
    descriptions: &[Option<String>],
) -> Vec<i32> {
    let service_state =
        state::service_state(&**engine).expect("the test engine must carry this service's state");
    let mut store = service_state.lock();
    let application_id = store
        .resolve_application_id(application_identifier)
        .unwrap_or_else(|| {
            panic!("case {case:?}: no application answers to {application_identifier:?}")
        });
    let definition_id = store
        .application(&application_id)
        .and_then(|application| application.resolve_experiment_definition_id(definition_identifier))
        .unwrap_or_else(|| {
            panic!("case {case:?}: no definition answers to {definition_identifier:?}")
        });
    let definition = store
        .application_mut(&application_id)
        .and_then(|application| application.experiment_definitions.get_mut(&definition_id))
        .expect("the resolved definition is held under the resolved application");

    let snapshot = definition.snapshot();
    let mut numbers = Vec::new();
    for description in descriptions {
        let number = definition.next_run_number;
        definition.next_run_number += 1;
        let started_at = SystemTime::UNIX_EPOCH
            + RUN_SPACING * u32::try_from(number).expect("a run number is positive");
        let mut run = ExperimentRun {
            run: number,
            application_id: application_id.clone(),
            experiment_definition_id: definition_id.clone(),
            description: description.clone(),
            status: state::EXPERIMENT_RUN_DONE.to_string(),
            exposure_percentage: state::UNEXPOSED_AUDIENCE,
            treatment_overrides: None,
            result: None,
            started_at,
            updated_at: started_at + Duration::from_secs(30),
            ended_at: Some(started_at + Duration::from_secs(600)),
            definition_snapshot: snapshot.clone(),
            tags: HashMap::new(),
            event_log: Vec::new(),
            events_recorded: 0,
        };
        // The timeline entry the start of a run writes, so a seeded run's log is the one
        // its own writer would have left rather than an empty one.
        run.record_start_event();
        assert!(
            definition.record_run(run).is_some(),
            "case {case:?}: run {number} must be a number the definition does not hold yet"
        );
        numbers.push(number);
    }
    numbers
}

/// Start one run of `definition` over the wire and answer the number it was given: the
/// `RUNNING` run of a history whose earlier runs are finished.
async fn start_running_run(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    definition: &str,
) -> i64 {
    let started = start_run(router, application, definition, &json!({})).await;
    assert_eq!(
        started.status,
        201,
        "case {case:?}: StartExperimentRun must answer 201, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    started
        .json()
        .get("Run")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: StartExperimentRun answered no Run"))
}

/// The run document `GetExperimentRun` reports, as a 200 body.
async fn read_run(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    definition: &str,
    run: i64,
) -> Value {
    let answer = get_run(router, application, definition, run).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: GetExperimentRun must report run {run}, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// A scope, a definition in it, and the history that definition holds: the finished runs
/// seeded into it and the running one started over the wire.
struct History {
    scope: Scope,
    definition_name: String,
    definition_id: String,
    /// Every run the definition holds, newest first — the order the listing must report.
    newest_first: Vec<i64>,
}

/// One definition under `scope` holding `finished` finished runs and, when `running`, one
/// more run still in flight.
async fn history(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    label: &str,
    finished: &[Option<String>],
    running: bool,
) -> History {
    let scope = scope(router, label).await;
    let name = format!("definition-{label}");
    let definition = create_test_definition(router, &scope, &name).await;
    let definition_id = id_of(label, &definition);

    let mut numbers: Vec<i64> = seed_finished_runs(
        engine,
        label,
        &scope.application,
        &definition_id,
        finished,
    )
    .into_iter()
    .map(i64::from)
    .collect();
    if running {
        numbers.push(start_running_run(router, label, &scope.application, &definition_id).await);
    }
    numbers.reverse();

    History {
        scope,
        definition_name: name,
        definition_id,
        newest_first: numbers,
    }
}

/// Descriptions spanning what a run may carry: none at all, and the published bounds of
/// `Description` (len 0..1024,
/// `smithy_query(query="operation",operation="StartExperimentRun")`).
fn described(count: usize) -> Vec<Option<String>> {
    let shapes: Vec<Option<String>> = vec![
        Some("the first rollout attempt".to_string()),
        None,
        Some(String::new()),
        Some(filler(1024, 3)),
        Some("ünïcödé description".to_string()),
    ];
    (0..count).map(|index| shapes[index % shapes.len()].clone()).collect()
}

/// RULE 1: each summary reports the stored run exactly as the reader does, and carries no
/// member outside the modelled summary — over a history whose runs differ in status and in
/// the description they carry.
#[tokio::test]
async fn a_summary_reports_the_stored_run_as_the_reader_does() {
    let (engine, router) = engine_and_router();
    let held = history(&engine, &router, "summaries", &described(5), true).await;

    let answer = list(&router, &held.scope.application, &held.definition_id, &[]).await;
    let page = items("every run", &answer);
    assert_eq!(
        page.len(),
        held.newest_first.len(),
        "case \"every run\": expected {} summaries, got {}: {}",
        held.newest_first.len(),
        page.len(),
        answer.json()
    );

    for summary in &page {
        let number = summary
            .get("Run")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("a summary carries no Run: {summary}"));
        let case = format!("run {number}");

        // The reader is the other document describing this same run: every member the
        // summary carries must be the one it reports, so a summary is a report over state
        // rather than a value made up at listing time.
        let read = read_run(
            &router,
            &case,
            &held.scope.application,
            &held.definition_id,
            number,
        )
        .await;
        let expected: Value = Value::Object(
            SUMMARY_MEMBERS
                .iter()
                .filter_map(|member| {
                    read.get(*member)
                        .map(|value| ((*member).to_string(), value.clone()))
                })
                .collect(),
        );
        assert_eq!(
            summary, &expected,
            "case {case:?}: expected the reader's members {expected}, got {summary}"
        );
    }
}

/// RULE 2: a listing reports exactly the runs of the definition it addresses, newest first
/// — and RULE 6: repeating it answers the same thing, and the runs it reported still read
/// back unchanged.
#[tokio::test]
async fn a_listing_reports_only_that_definitions_runs_newest_first() {
    let (engine, router) = engine_and_router();

    // A sibling definition under its own application, holding its own runs, so a listing
    // that read the whole store rather than one definition's history fails here.
    let sibling = history(&engine, &router, "sibling", &described(3), true).await;

    // Rows differ along how many runs the definition holds: none at all, one still
    // running, finished runs with no running one, and a history longer than a default page.
    for (label, finished, running) in [
        ("empty", 0_usize, false),
        ("one-running", 0, true),
        ("one-finished", 1, false),
        ("a-few", 3, true),
        ("more-than-a-page", MAX_LIST_PAGE_SIZE + 2, false),
    ] {
        let held = history(&engine, &router, label, &described(finished), running).await;
        let expected_first_page: Vec<i64> = held
            .newest_first
            .iter()
            .copied()
            .take(MAX_LIST_PAGE_SIZE)
            .collect();

        // Rows also differ along HOW the two path identifiers address their resources:
        // both are `Identifier`s — the resource's "ID or name" — so each form reaches the
        // same history.
        for (addressing, application, definition) in [
            (
                "by id",
                held.scope.application.clone(),
                held.definition_id.clone(),
            ),
            (
                "by name",
                format!("app-{label}"),
                held.definition_name.clone(),
            ),
        ] {
            let case = format!("{label}, addressed {addressing}");
            let answer = list(&router, &application, &definition, &[]).await;
            let page = items(&case, &answer);
            assert_eq!(
                numbers(&case, &page),
                expected_first_page,
                "case {case:?}: expected {expected_first_page:?}, got {:?}",
                numbers(&case, &page)
            );
            for summary in &page {
                assert_eq!(
                    summary.get("ExperimentDefinitionId").and_then(Value::as_str),
                    Some(held.definition_id.as_str()),
                    "case {case:?}: every summary must name the addressed definition, got {summary}"
                );
            }
            assert_eq!(
                answer.json().get("NextToken").is_some(),
                held.newest_first.len() > MAX_LIST_PAGE_SIZE,
                "case {case:?}: a NextToken is handed back only when runs are left over, got {}",
                answer.json()
            );

            // A read changes nothing: the same call answers the same thing.
            let again = list(&router, &application, &definition, &[]).await;
            assert_eq!(
                again.json(),
                answer.json(),
                "case {case:?}: listing twice must answer the same, got {}",
                again.json()
            );
        }

        // And the runs it reported are unchanged: the reader still reports each of them.
        for number in &held.newest_first {
            let case = format!("{label}, run {number} after listing");
            let read = read_run(
                &router,
                &case,
                &held.scope.application,
                &held.definition_id,
                *number,
            )
            .await;
            assert_eq!(
                read.get("Run").and_then(Value::as_i64),
                Some(*number),
                "case {case:?}: the reader must still report run {number}, got {read}"
            );
        }
    }

    // The sibling is untouched by every listing above, and still reports only its own runs.
    let case = "the sibling definition";
    let answer = list(
        &router,
        &sibling.scope.application,
        &sibling.definition_id,
        &[],
    )
    .await;
    let page = items(case, &answer);
    assert_eq!(
        numbers(case, &page),
        sibling.newest_first,
        "case {case:?}: expected the sibling's own runs newest first, got {}",
        answer.json()
    );
}

/// RULE 3: `Status` narrows the listing to the runs in that status — asserted for every
/// value the modelled enum lists, and for a request naming none.
#[tokio::test]
async fn the_status_filter_reports_only_runs_in_that_status() {
    let (engine, router) = engine_and_router();

    // Rows differ along the shape of the history the filter is applied to: only finished
    // runs, only a running one, both, and nothing at all.
    for (label, finished, running) in [
        ("filter-empty", 0_usize, false),
        ("filter-finished-only", 4, false),
        ("filter-running-only", 0, true),
        ("filter-mixed", 4, true),
    ] {
        let held = history(&engine, &router, label, &described(finished), running).await;
        let all = list(&router, &held.scope.application, &held.definition_id, &[]).await;
        let unfiltered = items(label, &all);
        assert_eq!(
            numbers(label, &unfiltered),
            held.newest_first,
            "case {label:?}: the unfiltered listing must report every run newest first, got {}",
            all.json()
        );

        // Every value of the modelled `ExperimentRunStatus` enum, read off the service's
        // own value list rather than hand-copied.
        for status in EXPERIMENT_RUN_STATUS_VALUES {
            let case = format!("{label}, Status {status}");
            let expected: Vec<i64> = unfiltered
                .iter()
                .filter(|summary| summary.get("Status").and_then(Value::as_str) == Some(*status))
                .map(|summary| {
                    summary
                        .get("Run")
                        .and_then(Value::as_i64)
                        .expect("a summary carries a Run")
                })
                .collect();

            let answer = list(
                &router,
                &held.scope.application,
                &held.definition_id,
                &[("status", (*status).to_string())],
            )
            .await;
            let page = items(&case, &answer);
            assert_eq!(
                numbers(&case, &page),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                numbers(&case, &page)
            );
            for summary in &page {
                assert_eq!(
                    summary.get("Status").and_then(Value::as_str),
                    Some(*status),
                    "case {case:?}: every summary must hold the filtered status, got {summary}"
                );
            }
        }

        // A status outside the modelled enum is refused by the schema walk before the
        // listing runs, rather than answered as an empty page.
        let case = format!("{label}, Status outside the enum");
        let refused = list(
            &router,
            &held.scope.application,
            &held.definition_id,
            &[("status", "PAUSED".to_string())],
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
    }
}

/// RULE 4: `MaxResults` bounds a page and `NextToken` resumes after it, so a walk reports
/// every run exactly once and still newest first — at every page size, including the
/// published bounds of `MaxResults`.
#[tokio::test]
async fn a_next_token_walk_reports_every_run_exactly_once_newest_first() {
    let (engine, router) = engine_and_router();

    // Rows span the run count and, for each, every page size from the published minimum up
    // past the count, plus the published maximum and no `MaxResults` at all.
    for (label, finished, running) in [
        ("walk-empty", 0_usize, false),
        ("walk-one", 0, true),
        ("walk-a-few", 4, true),
        ("walk-past-a-page", MAX_LIST_PAGE_SIZE + 3, false),
    ] {
        let held = history(&engine, &router, label, &described(finished), running).await;
        let expected = held.newest_first.clone();

        let mut page_sizes: Vec<Option<i64>> = (MAX_RESULTS_MIN
            ..=(expected.len() as i64 + 1).min(MAX_RESULTS_MAX))
            .map(Some)
            .collect();
        page_sizes.push(Some(MAX_RESULTS_MAX));
        page_sizes.push(None);

        for page_size in page_sizes {
            let case = format!("{label}, MaxResults {page_size:?}");
            let mut seen: Vec<i64> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let mut query: Vec<(&str, String)> = Vec::new();
                if let Some(page_size) = page_size {
                    query.push(("max_results", page_size.to_string()));
                }
                if let Some(token) = &next_token {
                    query.push(("next_token", token.clone()));
                }
                let answer = list(
                    &router,
                    &held.scope.application,
                    &held.definition_id,
                    &query,
                )
                .await;
                let page = items(&case, &answer);
                let bound = page_size.unwrap_or(MAX_RESULTS_MAX);
                assert!(
                    page.len() as i64 <= bound,
                    "case {case:?}: a page must carry at most {bound} items, got {}",
                    page.len()
                );
                seen.extend(numbers(&case, &page));
                pages += 1;
                assert!(
                    pages <= expected.len() + 2,
                    "case {case:?}: pagination did not terminate after {pages} pages"
                );
                next_token = answer.string("NextToken");
                if next_token.is_none() {
                    break;
                }
                assert!(
                    !page.is_empty(),
                    "case {case:?}: a page that resumes must carry items, got {}",
                    answer.json()
                );
            }

            assert_eq!(
                seen, expected,
                "case {case:?}: the walk must report every run exactly once newest first, expected {expected:?}, got {seen:?}"
            );
        }
    }
}

/// RULE 5: a path level that addresses nothing is 404, a `NextToken` this listing did not
/// issue is 400, and a `MaxResults` outside the published range is refused.
#[tokio::test]
async fn a_listing_that_addresses_nothing_or_carries_a_foreign_token_is_refused() {
    let (engine, router) = engine_and_router();
    let held = history(&engine, &router, "refusals", &described(3), true).await;
    let other_application = create_application(&router, "app-refusals-other").await;

    // Rows differ along WHICH level of the path resolves to nothing.
    for (case, application, definition) in [
        (
            "an application no resource answers to",
            "nosuchapp".to_string(),
            held.definition_id.clone(),
        ),
        (
            "a definition name nothing holds",
            held.scope.application.clone(),
            "no-such-definition".to_string(),
        ),
        (
            "a definition addressed under the wrong application",
            other_application.clone(),
            held.definition_id.clone(),
        ),
    ] {
        let refused = list(&router, &application, &definition, &[]).await;
        assert_eq!(
            refused.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_code()
        );
    }

    // A token this listing did not mint resumes nothing: rows differ along WHERE the token
    // came from — text that is no token at all, and a token another listing of this same
    // service minted.
    let extensions = json_call(&router, "GET", "/extensions?max_results=1", &Value::Null).await;
    let foreign_token = extensions
        .string("NextToken")
        .expect("ListExtensions must hand back a token over the AWS-authored catalogue");
    for (case, token) in [
        ("text that is no token at all", "not-a-token".to_string()),
        ("a token another listing minted", foreign_token),
    ] {
        let refused = list(
            &router,
            &held.scope.application,
            &held.definition_id,
            &[("next_token", token)],
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
    }

    // `MaxResults` is bounded 1..50: each published bound is served and each value one past
    // it is refused before the listing runs.
    for (case, page_size, expected_status) in [
        ("the lower published bound", MAX_RESULTS_MIN, 200_u16),
        ("the upper published bound", MAX_RESULTS_MAX, 200),
        ("one below the lower bound", MAX_RESULTS_MIN - 1, 400),
        ("one past the upper bound", MAX_RESULTS_MAX + 1, 400),
    ] {
        let answer = list(
            &router,
            &held.scope.application,
            &held.definition_id,
            &[("max_results", page_size.to_string())],
        )
        .await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: MaxResults {page_size} expected {expected_status}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // Every refusal above wrote nothing: the history is still exactly what it was.
    let case = "the history after every refusal";
    let answer = list(&router, &held.scope.application, &held.definition_id, &[]).await;
    let page = items(case, &answer);
    assert_eq!(
        numbers(case, &page),
        held.newest_first,
        "case {case:?}: expected {:?}, got {}",
        held.newest_first,
        answer.json()
    );
}

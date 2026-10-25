// SPDX-License-Identifier: CC0-1.0

//! `DescribeSearch` as a REPORTING read, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The sibling `search_lifecycle` suite pins what a started search reads back as.
//! These suites pin the rule that makes the read a read:
//!
//! * a describe answers from the search the start persisted, so every member it
//!   reports is the SAME on every poll — `startedAt` in particular is the instant
//!   the start recorded, never a fresh clock reading per call — and the only
//!   member that may differ between two polls is the status the clock owes, which
//!   never moves backwards;
//! * a describe writes NOTHING: the whole stored search map is byte-identical
//!   before and after any number of reads, through every phase of the lifecycle,
//!   because the status is derived from the instants the start recorded rather
//!   than stored and advanced by whoever polls;
//! * each search's status is derived from its OWN start, so searches started at
//!   different instants report independently and reading one never drags another
//!   along.
//!
//! The HTTP harness (`call`, `router_with_engine`) is the asset suite's, and the
//! `StartSearch` / `DescribeSearch` / `CreateWorkspace` wrappers are the search
//! suite's; this suite reuses both rather than growing a third copy.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, SEARCH_QUEUED, SEARCH_QUEUED_FOR, SEARCH_RUNNING, SEARCH_RUNS_FOR,
    SEARCH_SUCCEEDED,
};

use crate::asset_lifecycle::router_with_engine;
use crate::search_lifecycle::{create_workspace, describe, start};

/// The workspace most rows below search in.
const WORKSPACE: &str = "read-only-plant";

/// A second workspace, so the table spans more than one search scope.
const OTHER_WORKSPACE: &str = "read-only-plant_2";

/// Every search the store holds, one line per entry and sorted, so two snapshots
/// compare as a WHOLE: what a read must leave untouched is not one member of one
/// record but the entire map.
///
/// Rendered through `Debug` rather than compared field by field so a record that
/// grows a member is covered without this suite being updated.
fn stored_searches(shared: &Arc<engine::InMemoryEngine>) -> Vec<String> {
    let state = service_state(&**shared).expect("the iotsitewise store");
    let guard = state.lock();
    let mut rendered: Vec<String> = guard
        .searches
        .iter()
        .map(|(key, record)| format!("{key:?} => {record:?}"))
        .collect();
    drop(guard);
    rendered.sort();
    rendered
}

/// How far along the lifecycle a reported status is, so a sequence of polls can be
/// checked for never going backwards. Source: the `SearchStatus` values the model
/// names (smithy_query query="shape" shape="SearchStatus": FAILED | QUEUED |
/// RUNNING | SUCCEEDED), in the order the owed transitions take them. `FAILED` is
/// modelled but nothing in this emulator reports it, so it is diagnosed rather
/// than ranked.
fn rank(status: &str) -> usize {
    match status {
        SEARCH_QUEUED => 0,
        SEARCH_RUNNING => 1,
        SEARCH_SUCCEEDED => 2,
        other => panic!("DescribeSearch reported a status this emulator never reports: {other:?}"),
    }
}

/// `answer` without its `status` member: everything a poll must report identically
/// whatever the clock has done since the previous one.
fn without_status(answer: &Value) -> Value {
    let mut stripped = answer.clone();
    stripped
        .as_object_mut()
        .expect("DescribeSearch answers a JSON object")
        .remove("status");
    stripped
}

/// One row of the read-stability table: the search to start and how many times to
/// poll it.
#[derive(Debug, Clone)]
struct ReadCase {
    label: &'static str,
    workspace: &'static str,
    body: Value,
    polls: usize,
}

/// The rows differ along what the rule generalises over: how many times the search
/// is polled, which workspace it runs in, which members the start carried
/// (`searchType`, `groupId`, `searchFilters`) and how large its query statement
/// is — a read is no more stable for a small search than for a fully specified one.
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            label: "bare-start-polled-twice",
            workspace: WORKSPACE,
            body: json!({ "queryStatement": "compressor trend" }),
            polls: 2,
        },
        ReadCase {
            label: "deep-with-group-polled-three-times",
            workspace: WORKSPACE,
            body: json!({
                "queryStatement": "pump pressure anomalies",
                "searchType": "DEEP",
                "groupId": "pumps",
            }),
            polls: 3,
        },
        ReadCase {
            label: "quick-with-filters-polled-five-times",
            workspace: WORKSPACE,
            body: json!({
                "queryStatement": "readings in one window",
                "searchType": "QUICK",
                "searchFilters": {
                    "timeIntervals": [{
                        "startTime": { "timeInSeconds": 1_700_000_000i64, "offsetInNanos": 0 },
                        "endTime": { "timeInSeconds": 1_700_003_600i64, "offsetInNanos": 7 },
                    }],
                },
            }),
            polls: 5,
        },
        ReadCase {
            label: "longest-query-statement-in-another-workspace",
            workspace: OTHER_WORKSPACE,
            // The published `SearchQueryStatement` maximum length (len 1..5000).
            body: json!({ "queryStatement": "q".repeat(5000), "groupId": "ab" }),
            polls: 4,
        },
    ]
}

/// `StartSearch` with `query_statement` alone, asserted accepted, answering the
/// minted search id.
async fn start_one(router: &MultiServiceRouter, workspace: &str, query_statement: &str) -> String {
    let (status, body) = start(
        router,
        workspace,
        json!({ "queryStatement": query_statement }),
    )
    .await;
    assert_eq!(
        status, 202,
        "StartSearch {query_statement:?}: expected 202, got {status} {body}"
    );
    body["searchId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartSearch {query_statement:?}: no searchId in {body}"))
        .to_string()
}

/// The status `DescribeSearch` reports for one search, asserted readable.
async fn status_of(router: &MultiServiceRouter, workspace: &str, search_id: &str) -> String {
    let (status, described) = describe(router, workspace, search_id).await;
    assert_eq!(
        status, 200,
        "DescribeSearch {search_id}: expected 200, got {status} {described}"
    );
    described["status"]
        .as_str()
        .unwrap_or_else(|| panic!("DescribeSearch {search_id}: no status in {described}"))
        .to_string()
}

#[tokio::test]
async fn every_poll_answers_the_same_search_and_writes_nothing() {
    let (router, shared) = router_with_engine();
    for workspace in [WORKSPACE, OTHER_WORKSPACE] {
        create_workspace(&router, workspace).await;
    }

    let mut started: Vec<(ReadCase, String)> = Vec::new();
    for case in read_cases() {
        let (status, body) = start(&router, case.workspace, case.body.clone()).await;
        assert_eq!(
            status, 202,
            "case {}: StartSearch must accept the request, got {status} {body}",
            case.label
        );
        let search_id = body["searchId"].as_str().unwrap_or_default().to_string();
        assert!(
            !search_id.is_empty(),
            "case {}: StartSearch must mint a searchId, got {body}",
            case.label
        );
        started.push((case, search_id));
    }

    let before = stored_searches(&shared);
    assert_eq!(
        before.len(),
        started.len(),
        "the store must hold one search per start, expected {}, got {before:?}",
        started.len()
    );

    for (case, search_id) in &started {
        let mut answers: Vec<Value> = Vec::new();
        for poll in 1..=case.polls {
            let (status, described) = describe(&router, case.workspace, search_id).await;
            assert_eq!(
                status, 200,
                "case {} poll {poll}: DescribeSearch must answer the stored search, got {status} {described}",
                case.label
            );
            assert_eq!(
                described["searchId"].as_str().unwrap_or_default(),
                search_id.as_str(),
                "case {} poll {poll}: the read must answer for the search it names, got {described}",
                case.label
            );
            answers.push(described);
        }

        let first = answers.first().expect("every row polls at least once").clone();
        for (at, answer) in answers.iter().enumerate() {
            assert_eq!(
                without_status(answer),
                without_status(&first),
                "case {} poll {}: every member but the status must be identical on every poll, expected {}, got {answer}",
                case.label,
                at + 1,
                first
            );
            assert_eq!(
                answer["startedAt"], first["startedAt"],
                "case {} poll {}: startedAt is the instant the start recorded, so it must not move, expected {}, got {}",
                case.label,
                at + 1,
                first["startedAt"],
                answer["startedAt"]
            );
            assert!(
                rank(answer["status"].as_str().unwrap_or_default())
                    >= rank(first["status"].as_str().unwrap_or_default()),
                "case {} poll {}: a status may only move forwards, started at {} and got {}",
                case.label,
                at + 1,
                first["status"],
                answer["status"]
            );
        }
    }

    assert_eq!(
        stored_searches(&shared),
        before,
        "no number of reads may change the stored searches, expected {before:?}"
    );
}

#[tokio::test]
async fn each_search_reports_the_lifecycle_its_own_start_owes() {
    let (router, shared) = router_with_engine();
    create_workspace(&router, WORKSPACE).await;

    let earlier = start_one(&router, WORKSPACE, "the earlier search").await;
    // Far enough past the earlier start for its queued span to have fallen due,
    // and well short of the end of its run.
    tokio::time::sleep(SEARCH_QUEUED_FOR + SEARCH_RUNS_FOR / 2).await;
    let later = start_one(&router, WORKSPACE, "the later search").await;

    let before = stored_searches(&shared);

    let earlier_status = status_of(&router, WORKSPACE, &earlier).await;
    let later_status = status_of(&router, WORKSPACE, &later).await;
    assert!(
        earlier_status == SEARCH_RUNNING || earlier_status == SEARCH_SUCCEEDED,
        "case past-its-queue: the earlier search must be past {SEARCH_QUEUED}, got {earlier_status}"
    );
    assert_eq!(
        later_status, SEARCH_QUEUED,
        "case just-started: a search started now is {SEARCH_QUEUED} whatever an older one reports, got {later_status}"
    );
    assert!(
        rank(&earlier_status) > rank(&later_status),
        "case independent-clocks: each status is derived from its own start, so the earlier search must be ahead, got {earlier_status} and {later_status}"
    );

    // Past the earlier search's whole lifecycle, and short of the later one's.
    tokio::time::sleep(SEARCH_RUNS_FOR).await;
    let earlier_status = status_of(&router, WORKSPACE, &earlier).await;
    let later_status = status_of(&router, WORKSPACE, &later).await;
    assert_eq!(
        earlier_status, SEARCH_SUCCEEDED,
        "case earlier-settled: the earlier search's owed transitions have all fallen due, got {earlier_status}"
    );
    assert_ne!(
        later_status, SEARCH_SUCCEEDED,
        "case later-still-running: the later search's run is not due yet, got {later_status}"
    );

    // Past both.
    tokio::time::sleep(SEARCH_QUEUED_FOR + SEARCH_RUNS_FOR).await;
    for (label, search_id) in [("earlier", &earlier), ("later", &later)] {
        let status = status_of(&router, WORKSPACE, search_id).await;
        assert_eq!(
            status, SEARCH_SUCCEEDED,
            "case {label}-terminal: every search settles at {SEARCH_SUCCEEDED}, got {status}"
        );
    }

    // The statuses above moved because the clock did; the records themselves were
    // never touched by a read.
    assert_eq!(
        stored_searches(&shared),
        before,
        "reading a search through every phase must leave its record as the start wrote it, expected {before:?}"
    );
}

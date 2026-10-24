// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the status sequence a CREATED deployment moves through.
//!
//! The rule under test: a created deployment walks every non-failure
//! `DeploymentStatus` member the model declares for a create, in the order those phases
//! happen — accepted (`CREATING`), settings checked (`VALIDATING`), resources created
//! (`IN_PROGRESS`), done (`COMPLETED`) — each step owed by the CLOCK one
//! `TRANSITION_SPAN` after the previous one, so every one of them is observable by a
//! polling reader and matchable by a `DEPLOYMENT_STATUS` filter.
//!
//! It is asserted over a TABLE rather than over one call: one row per seeded workload,
//! each deploying a different deployment pattern, so the sequence is a property of a
//! create rather than of the one workload an issue happened to cite. No status value is
//! written by hand without being checked against the model's own `DeploymentStatus`
//! enum, and the step-by-step proof is taken from the RECORDED event instants, which do
//! not depend on when this test read them.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::json;
use service_com_amazonaws_launchwizard_launchwizard::ops::types::{
    DEPLOYMENT_STATUS_VALUES, EVENT_STATUS_VALUES,
};
use service_com_amazonaws_launchwizard_launchwizard::state::TRANSITION_SPAN;

use crate::deployment_lifecycle::{create, get};
// The catalogue table and the event reader are the suite-shared `pub(crate)` helpers
// rather than copies: one dispatch path per concern means two suites cannot disagree
// about what the catalogue holds or how a history is read.
use crate::deployment_update::{events, one_pattern_per_workload};
use crate::harness::Harness;

/// The `DeploymentStatus` members a create moves a deployment through, in order. Each
/// is checked against `DEPLOYMENT_STATUS_VALUES` (the model's enum) by
/// [`the_create_sequence_is_made_of_modelled_statuses`].
const CREATE_SEQUENCE: [&str; 4] = ["CREATING", "VALIDATING", "IN_PROGRESS", "COMPLETED"];

/// `DeploymentStatus` members that belong to another operation's lifecycle, so a create
/// may never report them. Taken from `DEPLOYMENT_STATUS_VALUES` (the model's enum).
const NOT_PART_OF_A_CREATE: [&str; 4] = [
    "UPDATE_IN_PROGRESS",
    "UPDATE_COMPLETED",
    "DELETE_INITIATING",
    "DELETED",
];

/// How long a poll waits for the whole create sequence to finish: the steps the create
/// schedules, with generous slack.
const SEQUENCE_DEADLINE: Duration = Duration::from_millis(8_000);

/// How often the poll re-reads the deployment while the create runs.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The position of `status` in [`CREATE_SEQUENCE`], or `None` for a status that is not
/// part of a create at all.
fn step_of(status: &str) -> Option<usize> {
    CREATE_SEQUENCE.iter().position(|step| *step == status)
}

/// The terminal step of [`CREATE_SEQUENCE`].
fn terminal() -> &'static str {
    CREATE_SEQUENCE[CREATE_SEQUENCE.len() - 1]
}

/// The status `ListDeployments` reports for `id`, so a reader other than
/// `GetDeployment` is checked to agree.
async fn listed_status(harness: &Harness, case: &str, id: &str) -> String {
    let body = harness.ok(case, "/listDeployments", json!({})).await;
    body["deployments"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListDeployments reported no list: {body}"))
        .iter()
        .find(|summary| summary["id"].as_str() == Some(id))
        .and_then(|summary| summary["status"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: ListDeployments must report {id}: {body}"))
        .to_string()
}

/// True when `ListDeployments` filtered by `DEPLOYMENT_STATUS=status` reports `id`.
async fn matched_by_status_filter(harness: &Harness, case: &str, id: &str, status: &str) -> bool {
    let body = harness
        .ok(
            case,
            "/listDeployments",
            json!({"filters": [{"name": "DEPLOYMENT_STATUS", "values": [status]}]}),
        )
        .await;
    body["deployments"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListDeployments reported no list: {body}"))
        .iter()
        .any(|summary| summary["id"].as_str() == Some(id))
}

/// Poll `GetDeployment` until the deployment reaches the terminal step of
/// [`CREATE_SEQUENCE`], returning every distinct status observed, in order.
async fn poll_until_complete(harness: &Harness, case: &str, id: &str) -> Vec<String> {
    let started = Instant::now();
    let mut observed: Vec<String> = Vec::new();
    loop {
        let status = get(harness, case, id).await["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if observed.last() != Some(&status) {
            observed.push(status.clone());
        }
        if status == terminal() {
            return observed;
        }
        assert!(
            started.elapsed() < SEQUENCE_DEADLINE,
            "case {case:?}: the create never reached {:?} within {SEQUENCE_DEADLINE:?}, \
             observed {observed:?}",
            terminal()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn the_create_sequence_is_made_of_modelled_statuses() {
    for status in CREATE_SEQUENCE.iter().chain(NOT_PART_OF_A_CREATE.iter()) {
        assert!(
            DEPLOYMENT_STATUS_VALUES.contains(status),
            "case {status:?}: this suite may only name DeploymentStatus members, got {status:?} \
             which is not in {DEPLOYMENT_STATUS_VALUES:?}"
        );
    }
    // The four are distinct members, which is why a create has four observable steps
    // rather than collapsing two of them.
    let distinct: BTreeSet<&str> = CREATE_SEQUENCE.into_iter().collect();
    assert_eq!(
        distinct.len(),
        CREATE_SEQUENCE.len(),
        "case {CREATE_SEQUENCE:?}: every step of the create sequence must be a distinct \
         DeploymentStatus member, got {distinct:?}"
    );
    for status in NOT_PART_OF_A_CREATE {
        assert_eq!(
            step_of(status),
            None,
            "case {status:?}: a status of another operation's lifecycle is not a create step, \
             got {CREATE_SEQUENCE:?}"
        );
    }
}

#[tokio::test]
async fn a_created_deployment_walks_every_modelled_create_step_on_the_clock() {
    let harness = Harness::new();
    let rows = one_pattern_per_workload(&harness).await;

    for (row, (workload, pattern)) in rows.into_iter().enumerate() {
        let case = format!("{workload}/{pattern}");
        let id = create(
            &harness,
            &case,
            &workload,
            &pattern,
            &format!("create-sequence-{row}"),
        )
        .await;

        // A reader acting straight after the create still observes the first step: no
        // clock-owed step is due at zero.
        let read_back = get(&harness, &case, &id).await["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let listed = listed_status(&harness, &case, &id).await;
        for (reader, status) in [("GetDeployment", &read_back), ("ListDeployments", &listed)] {
            assert_eq!(
                status.as_str(),
                CREATE_SEQUENCE[0],
                "case {case:?}: {reader} must report {:?} straight after the create, got {status:?}",
                CREATE_SEQUENCE[0]
            );
        }

        // Polling from there walks the modelled sequence in order, never rewinding and
        // never reporting a status that is not part of a create. It is an in-order
        // SUBSEQUENCE rather than always every step, because `materialize_due` drains
        // every step the clock already owes in one call: a reader that stalls past a
        // whole span legitimately sees the later step only. The strict step-by-step
        // proof is on the recorded event instants below.
        let observed = poll_until_complete(&harness, &case, &id).await;
        let steps: Vec<usize> = observed
            .iter()
            .map(|status| {
                step_of(status).unwrap_or_else(|| {
                    panic!("case {case:?}: {status:?} is not part of a create, in {observed:?}")
                })
            })
            .collect();
        assert!(
            steps.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case:?}: a reader polling the create must walk {CREATE_SEQUENCE:?} in order \
             without rewinding, got {observed:?}"
        );
        assert_eq!(
            observed.first().map(String::as_str),
            Some(CREATE_SEQUENCE[0]),
            "case {case:?}: the first status a reader sees is {:?}, got {observed:?}",
            CREATE_SEQUENCE[0]
        );
        assert_eq!(
            observed.last().map(String::as_str),
            Some(terminal()),
            "case {case:?}: the create must end at {:?}, got {observed:?}",
            terminal()
        );

        // The record carries one event per step of the sequence, each with a status the
        // model's `EventStatus` enum declares, and each step's instant is one
        // `TRANSITION_SPAN` after the previous one — the clock-owed evidence that the
        // middle steps are steps of their own rather than statuses the create passed
        // through instantly.
        let history = events(&harness, &case, &id).await;
        assert_eq!(
            history.len(),
            CREATE_SEQUENCE.len(),
            "case {case:?}: a create records one event per step of {CREATE_SEQUENCE:?}, \
             got {history:?}"
        );
        for event in &history {
            let status = event["status"].as_str().unwrap_or_default();
            assert!(
                EVENT_STATUS_VALUES.contains(&status),
                "case {case:?}: {status:?} is not an EventStatus value, in {event}"
            );
        }
        assert_eq!(
            history.last().and_then(|event| event["status"].as_str()),
            Some("COMPLETED"),
            "case {case:?}: the last event of a finished create is COMPLETED, got {history:?}"
        );
        let timestamps: Vec<f64> = history
            .iter()
            .map(|event| event["timestamp"].as_f64().unwrap_or_default())
            .collect();
        let span = TRANSITION_SPAN.as_secs_f64();
        for pair in timestamps.windows(2) {
            let gap = pair[1] - pair[0];
            assert!(
                (gap - span).abs() < 0.05,
                "case {case:?}: each create step is owed one span ({span}s) after the previous \
                 one, got a gap of {gap}s in {timestamps:?}"
            );
        }

        // Nothing the create claimed is infrastructure the emulator did not build, and
        // the terminal record is still a live one.
        let settled = get(&harness, &case, &id).await;
        assert!(
            settled["deletedAt"].is_null(),
            "case {case:?}: a created deployment carries no deletedAt, got {}",
            settled["deletedAt"]
        );
        assert_eq!(
            settled["modifiedAt"].as_f64(),
            timestamps.last().copied(),
            "case {case:?}: modifiedAt is the instant the last step was owed, got {} against \
             {timestamps:?}",
            settled["modifiedAt"]
        );
    }
}

#[tokio::test]
async fn a_deployment_status_filter_can_match_every_step_of_a_create() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let case = format!("{workload}/{pattern} status filter");
    let id = create(&harness, &case, &workload, &pattern, "filter-sequence").await;

    // Poll the `DEPLOYMENT_STATUS` filter — not `GetDeployment` — for each step of the
    // sequence, recording the ones it ever matches, until the deployment is terminal.
    // Every step must be matchable at some point, so no phase of a create is invisible
    // to a filtering caller.
    let started = Instant::now();
    let mut matched: BTreeSet<&str> = BTreeSet::new();
    loop {
        for status in CREATE_SEQUENCE {
            if matched_by_status_filter(&harness, &case, &id, status).await {
                matched.insert(status);
            }
        }
        if matched.contains(terminal()) {
            break;
        }
        assert!(
            started.elapsed() < SEQUENCE_DEADLINE,
            "case {case:?}: the create never reached {:?} within {SEQUENCE_DEADLINE:?}, \
             the filter matched {matched:?}",
            terminal()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    for status in CREATE_SEQUENCE {
        assert!(
            matched.contains(status),
            "case {case:?}: a DEPLOYMENT_STATUS filter on {status:?} must match the deployment \
             at some point of the create, it matched only {matched:?}"
        );
    }

    // And a status of another operation's lifecycle never matches a deployment that was
    // only created.
    for status in NOT_PART_OF_A_CREATE {
        let case = format!("{case}: {status}");
        assert!(
            !matched_by_status_filter(&harness, &case, &id, status).await,
            "case {case:?}: a created deployment must never be matched by {status:?}"
        );
    }
}

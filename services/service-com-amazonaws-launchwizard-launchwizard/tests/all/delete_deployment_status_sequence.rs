// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the status sequence a delete of a deployment moves through.
//!
//! The rule under test: `DeleteDeployment` on a deployment that is not already going
//! away reports the status the deployment has JUST entered — `DELETE_INITIATING`, the
//! member the model declares for a delete that has only been accepted — and the
//! deployment then advances on the CLOCK through `DELETE_IN_PROGRESS` to `DELETED`,
//! so every one of those three statuses is observable by a polling reader.
//!
//! It is asserted over a TABLE rather than over one call: the delete arrives while the
//! deployment is in each status a live deployment can be in (`CREATING`,
//! `IN_PROGRESS`, `COMPLETED`, `UPDATE_IN_PROGRESS`), on a different workload and
//! deployment pattern of the seeded catalogue each time. No status value is written by
//! hand without being checked against the model's own `DeploymentStatus` enum.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_launchwizard_launchwizard::ops::types::{
    DEPLOYMENT_STATUS_VALUES, EVENT_STATUS_VALUES,
};
use service_com_amazonaws_launchwizard_launchwizard::state::TRANSITION_SPAN;

use crate::deployment_lifecycle::{create, get};
use crate::harness::{catalogue, Harness};

/// The `DeploymentStatus` members a delete moves a deployment through, in order. Each
/// is checked against `DEPLOYMENT_STATUS_VALUES` (the model's enum) by
/// [`the_delete_sequence_is_made_of_modelled_statuses`].
const DELETE_SEQUENCE: [&str; 3] = ["DELETE_INITIATING", "DELETE_IN_PROGRESS", "DELETED"];

/// Long enough for one clock-owed status step to come due (`state::TRANSITION_SPAN`
/// plus slack). The emulator never sleeps; this is the test waiting for the clock.
const ONE_STEP: Duration = Duration::from_millis(700);

/// How long a poll waits for the whole delete sequence to finish: the two steps the
/// delete schedules, with generous slack.
const SEQUENCE_DEADLINE: Duration = Duration::from_millis(6_000);

/// How often the poll re-reads the deployment while the delete runs.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The position of `status` in [`DELETE_SEQUENCE`], or `None` for a status that is not
/// part of a delete at all.
fn step_of(status: &str) -> Option<usize> {
    DELETE_SEQUENCE.iter().position(|step| *step == status)
}

/// One `(workloadName, deploymentPatternName)` pair per seeded workload, in the order
/// the catalogue reports them — so the rows below differ in the workload and pattern
/// they deploy as well as in when the delete arrives.
async fn one_pattern_per_workload(harness: &Harness) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for workload in catalogue(harness).await {
        let body = harness
            .ok(
                &workload,
                "/listWorkloadDeploymentPatterns",
                json!({"workloadName": workload}),
            )
            .await;
        let pattern = body["workloadDeploymentPatterns"]
            .as_array()
            .unwrap_or_else(|| panic!("case {workload:?}: no patterns reported: {body}"))
            .first()
            .and_then(|summary| summary["deploymentPatternName"].as_str())
            .unwrap_or_else(|| panic!("case {workload:?}: a workload must offer a pattern: {body}"))
            .to_string();
        pairs.push((workload, pattern));
    }
    assert!(
        !pairs.is_empty(),
        "the seeded catalogue must offer a pattern to deploy"
    );
    pairs
}

/// The deployment events of `id`, oldest first.
async fn events(harness: &Harness, case: &str, id: &str) -> Vec<Value> {
    let body = harness
        .ok(case, "/listDeploymentEvents", json!({"deploymentId": id}))
        .await;
    body["deploymentEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListDeploymentEvents reported no list: {body}"))
        .clone()
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

/// Poll `GetDeployment` until the deployment reaches the last step of
/// [`DELETE_SEQUENCE`], returning every status observed, in order.
async fn poll_until_deleted(harness: &Harness, case: &str, id: &str) -> Vec<String> {
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
        if status == DELETE_SEQUENCE[DELETE_SEQUENCE.len() - 1] {
            return observed;
        }
        assert!(
            started.elapsed() < SEQUENCE_DEADLINE,
            "case {case:?}: the delete never reached {:?} within {SEQUENCE_DEADLINE:?}, \
             observed {observed:?}",
            DELETE_SEQUENCE[DELETE_SEQUENCE.len() - 1]
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn the_delete_sequence_is_made_of_modelled_statuses() {
    for status in DELETE_SEQUENCE {
        assert!(
            DEPLOYMENT_STATUS_VALUES.contains(&status),
            "case {status:?}: the delete sequence may only use DeploymentStatus members, \
             got {status:?} which is not in {DEPLOYMENT_STATUS_VALUES:?}"
        );
    }
    // The three are distinct members, which is why a delete has three observable steps
    // rather than collapsing two of them.
    for (index, status) in DELETE_SEQUENCE.iter().enumerate() {
        assert_eq!(
            step_of(status),
            Some(index),
            "case {status:?}: every step of the delete sequence must be distinct, \
             got {DELETE_SEQUENCE:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_reports_delete_initiating_and_then_advances_on_the_clock() {
    let harness = Harness::new();
    let patterns = one_pattern_per_workload(&harness).await;

    // One row per status a live deployment can be in when the delete arrives: the rule
    // is about the delete, not about what the deployment was doing, so each row must
    // report the same first status and walk the same sequence afterwards.
    let rows: [(&str, Option<Duration>, bool); 4] = [
        // label, how long to wait after the create, whether to update first
        ("delete while CREATING", None, false),
        ("delete while IN_PROGRESS", Some(ONE_STEP), false),
        ("delete when COMPLETED", Some(ONE_STEP * 2), false),
        ("delete while UPDATE_IN_PROGRESS", Some(ONE_STEP * 2), true),
    ];

    for (row, (label, wait, update_first)) in rows.into_iter().enumerate() {
        let (workload, pattern) = patterns[row % patterns.len()].clone();
        let case = format!("{label} ({workload}/{pattern})");
        let id = create(
            &harness,
            &case,
            &workload,
            &pattern,
            &format!("delete-sequence-{row}"),
        )
        .await;
        if let Some(wait) = wait {
            tokio::time::sleep(wait).await;
        }
        if update_first {
            // `force` applies the settings despite validation warnings, which is all
            // this row needs: it only has to leave the deployment mid-update.
            harness
                .ok(
                    &case,
                    "/updateDeployment",
                    json!({
                        "deploymentId": id,
                        "specifications": {"KeyPairName": "key-for-delete-sequence"},
                        "force": true,
                    }),
                )
                .await;
        }
        let before = get(&harness, &case, &id).await["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            step_of(&before).is_none(),
            "case {case:?}: the row must delete a deployment that is not already going away, \
             got {before:?}"
        );
        let events_before = events(&harness, &case, &id).await.len();

        // The synchronous answer is the status the deployment has just entered.
        let answered = harness
            .ok(&case, "/deleteDeployment", json!({"deploymentId": id}))
            .await;
        let reported = answered["status"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            reported, DELETE_SEQUENCE[0],
            "case {case:?}: the delete must report the status it just entered, got {reported:?} \
             (the deployment was {before:?})"
        );

        // And it is the status every reader reports straight afterwards, so the first
        // step is observable rather than skipped.
        let read_back = get(&harness, &case, &id).await["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let listed = listed_status(&harness, &case, &id).await;
        for (reader, status) in [("GetDeployment", &read_back), ("ListDeployments", &listed)] {
            assert_eq!(
                status.as_str(), DELETE_SEQUENCE[0],
                "case {case:?}: {reader} must report {:?} straight after the delete, got {status:?}",
                DELETE_SEQUENCE[0]
            );
        }

        // Polling from there walks the modelled sequence in order, never rewinding and
        // never reporting a status that is not part of a delete. It is an in-order
        // SUBSEQUENCE rather than always all three steps, because `materialize_due`
        // drains every step the clock already owes in one call: a reader that stalls
        // past a whole span legitimately sees the later step only. The strict
        // three-step proof is on the recorded event instants below, which do not
        // depend on when this test reads them.
        let observed = poll_until_deleted(&harness, &case, &id).await;
        let steps: Vec<usize> = observed
            .iter()
            .map(|status| {
                step_of(status).unwrap_or_else(|| {
                    panic!("case {case:?}: {status:?} is not part of a delete, in {observed:?}")
                })
            })
            .collect();
        assert!(
            steps.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case:?}: a reader polling the delete must walk {DELETE_SEQUENCE:?} in order \
             without rewinding, got {observed:?}"
        );
        assert_eq!(
            observed.first().map(String::as_str),
            Some(DELETE_SEQUENCE[0]),
            "case {case:?}: the first status a reader sees after the delete is {:?}, got \
             {observed:?}",
            DELETE_SEQUENCE[0]
        );
        assert_eq!(
            observed.last().map(String::as_str),
            Some(DELETE_SEQUENCE[DELETE_SEQUENCE.len() - 1]),
            "case {case:?}: the delete must end at {:?}, got {observed:?}",
            DELETE_SEQUENCE[DELETE_SEQUENCE.len() - 1]
        );

        // The terminal record carries `deletedAt`, and the delete appended one event per
        // step it took, each with a status the model's `EventStatus` enum declares.
        let deleted = get(&harness, &case, &id).await;
        assert!(
            deleted["deletedAt"].as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: a deleted deployment carries deletedAt, got {}",
            deleted["deletedAt"]
        );
        let history = events(&harness, &case, &id).await;
        let appended = &history[events_before..];
        assert_eq!(
            appended.len(),
            DELETE_SEQUENCE.len(),
            "case {case:?}: the delete must record one event per step of {DELETE_SEQUENCE:?}, \
             got {appended:?}"
        );
        for event in appended {
            let status = event["status"].as_str().unwrap_or_default();
            assert!(
                EVENT_STATUS_VALUES.contains(&status),
                "case {case:?}: {status:?} is not an EventStatus value, in {event}"
            );
        }
        assert_eq!(
            appended
                .last()
                .and_then(|event| event["status"].as_str()),
            Some("COMPLETED"),
            "case {case:?}: the last event of a finished delete is COMPLETED, got {appended:?}"
        );
        // Each step is one `TRANSITION_SPAN` after the previous one, which is the
        // clock-owed, record-level evidence that the middle step is a step of its own
        // rather than a status the delete passed through instantly: the recorded
        // instants do not depend on when this test happened to read them.
        let timestamps: Vec<f64> = appended
            .iter()
            .map(|event| event["timestamp"].as_f64().unwrap_or_default())
            .collect();
        let span = TRANSITION_SPAN.as_secs_f64();
        for pair in timestamps.windows(2) {
            let gap = pair[1] - pair[0];
            assert!(
                (gap - span).abs() < 0.05,
                "case {case:?}: each delete step is owed one span ({span}s) after the previous \
                 one, got a gap of {gap}s in {timestamps:?}"
            );
        }
        assert_eq!(
            deleted["deletedAt"].as_f64(),
            timestamps.last().copied(),
            "case {case:?}: deletedAt is the instant the terminal step was owed, got {} against \
             {timestamps:?}",
            deleted["deletedAt"]
        );
    }
}

#[tokio::test]
async fn repeating_a_delete_neither_restarts_the_sequence_nor_rewinds_it() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let case = format!("{workload}/{pattern} repeated delete");
    let id = create(&harness, &case, &workload, &pattern, "delete-twice").await;

    // `@idempotent`: every repeat reports where the delete has got to, never a step
    // before the one already reached, and appends nothing of its own.
    let mut highest = 0usize;
    let mut answered = Vec::new();
    for call in 0..4 {
        let body = harness
            .ok(
                &format!("{case} call {call}"),
                "/deleteDeployment",
                json!({"deploymentId": id}),
            )
            .await;
        let status = body["status"].as_str().unwrap_or_default().to_string();
        let step = step_of(&status).unwrap_or_else(|| {
            panic!("case {case:?} call {call}: {status:?} is not part of a delete, in {body}")
        });
        assert!(
            step >= highest,
            "case {case:?} call {call}: a repeated delete must not rewind the sequence, \
             got {status:?} after step {highest}"
        );
        highest = step;
        answered.push(status);
        tokio::time::sleep(ONE_STEP).await;
    }
    assert_eq!(
        answered.last().map(String::as_str),
        Some(DELETE_SEQUENCE[DELETE_SEQUENCE.len() - 1]),
        "case {case:?}: the deployment must still reach the terminal status, got {answered:?}"
    );

    // Three delete events, not one per call: a repeat restarts nothing.
    let history = events(&harness, &case, &id).await;
    let delete_events = history
        .iter()
        .filter(|event| {
            event["status"].as_str() != Some("CREATED")
                && event["name"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("Delete"))
        })
        .count();
    assert_eq!(
        delete_events,
        DELETE_SEQUENCE.len(),
        "case {case:?}: four deletes of one deployment must still record only the {} steps the \
         sequence has, got {delete_events} in {history:?}",
        DELETE_SEQUENCE.len()
    );
    let deleted_at = get(&harness, &case, &id).await["deletedAt"]
        .as_f64()
        .unwrap_or_default();
    assert!(
        deleted_at > 0.0,
        "case {case:?}: the deleted deployment must carry deletedAt, got {deleted_at}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! What a virtual cluster's teardown does to the resources the cluster HOLDS.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client. Every request helper is the one the suites beside this one drive, so there is
//! one request path under test rather than a copy per suite.
//!
//! The rule under test, asserted over a TABLE of clusters rather than one request:
//! deleting a parent resource tears down the resources that live under it, so a child that
//! has not already reached a terminal state takes the terminal transition of its own kind
//! with the parent — a managed endpoint `TERMINATING` then `TERMINATED`, a job run
//! `CANCEL_PENDING` then `CANCELLED` — while a child that is already over, or already on
//! its way out, keeps the end it had. The rows differ along what a cluster holds: nothing,
//! endpoints in each non-terminal state, runs the clock has started, a run waiting behind
//! the cluster's concurrency limit, a run that had already finished and a cancellation
//! still in flight.

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::ops::types::ENDPOINT_STATE_VALUES;
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::managed_endpoint_listing::{
    create_endpoint, describe_endpoint, described, endpoint_body, list_endpoints, listed_ids,
};
use crate::virtual_cluster_job_run_loop::{field, text, Emr};
use crate::virtual_cluster_scheduler_status::{
    cancel_run, cluster_id, run_id, run_state, scheduler_status, wait_for_the_clock,
};

/// One resource a fixture cluster holds by the time its teardown starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Child {
    /// A managed endpoint the clock has already settled to `ACTIVE`.
    ActiveEndpoint,
    /// A managed endpoint still `CREATING` when the teardown starts.
    CreatingEndpoint,
    /// A job run the clock has already acted on: `RUNNING`, or still `PENDING` while the
    /// cluster's concurrency limit is taken.
    StartedRun,
    /// A job run cancelled and settled before the teardown: already terminal.
    FinishedRun,
    /// A job run whose cancellation is still in flight when the teardown starts.
    CancellingRun,
}

impl Child {
    /// Whether this child is a managed endpoint rather than a job run.
    fn is_endpoint(self) -> bool {
        matches!(self, Child::ActiveEndpoint | Child::CreatingEndpoint)
    }
}

/// One row: a cluster, the scheduler configuration it is created with, and what it holds.
struct TeardownCase {
    label: &'static str,
    scheduler: Option<Value>,
    children: Vec<Child>,
    /// The `(currentInQueueJobRuns, currentConcurrentJobRuns)` the cluster reports once
    /// the row is set up and BEFORE the teardown, so a row that means to leave a run
    /// waiting behind the cluster's concurrency limit proves it did.
    queued_and_running_before: (i64, i64),
}

/// The `state` `DescribeManagedEndpoint` reports for one endpoint.
async fn endpoint_state(emr: &Emr, case: &str, cluster: &str, endpoint: &str) -> String {
    let answer = describe_endpoint(emr, cluster, endpoint).await;
    let endpoint = described(answer.ok(case), case);
    text(&endpoint, "/state", case)
}

/// The `state` `DescribeVirtualCluster` reports for one cluster.
async fn cluster_state(emr: &Emr, case: &str, cluster: &str) -> String {
    let answer = emr.describe_cluster(cluster).await;
    let described = field(answer.ok(case), "/virtualCluster", case).clone();
    text(&described, "/state", case)
}

/// Whether `DescribeJobRun` reports a `finishedAt` for one run: the instant a run that
/// reached a terminal state finished at.
async fn run_has_finished_at(emr: &Emr, case: &str, cluster: &str, run: &str) -> bool {
    let answer = emr.describe_job(cluster, run).await;
    answer
        .ok(case)
        .pointer("/jobRun/finishedAt")
        .is_some_and(|value| !value.is_null())
}

/// Create the cluster of one row and everything it holds, leaving each child in the state
/// the row puts it in. Answers the cluster id and each child beside the id it was minted
/// under.
async fn set_up(emr: &Emr, case: &TeardownCase, index: usize) -> (String, Vec<(Child, String)>) {
    let cluster = cluster_id(
        emr,
        case.label,
        &format!("teardown-cascade-{index}"),
        &format!("tok-cascade-{index}"),
        case.scheduler.clone(),
    )
    .await;
    let mut children: Vec<(Child, String)> = Vec::new();

    // First everything the clock has to act on: the endpoints that must reach ACTIVE and
    // every job run the row carries.
    for (slot, child) in case.children.iter().enumerate() {
        let id = match child {
            Child::ActiveEndpoint => {
                let created = create_endpoint(
                    emr,
                    &cluster,
                    endpoint_body(
                        &format!("cascade-{index}-{slot}"),
                        &format!("ep-tok-cascade-{index}-{slot}"),
                        "JUPYTER_ENTERPRISE_GATEWAY",
                    ),
                )
                .await;
                text(created.ok(case.label), "/id", case.label)
            }
            Child::StartedRun | Child::FinishedRun | Child::CancellingRun => {
                run_id(
                    emr,
                    case.label,
                    &cluster,
                    &format!("run-tok-cascade-{index}-{slot}"),
                )
                .await
            }
            // Created last, so it is still CREATING when the teardown starts.
            Child::CreatingEndpoint => continue,
        };
        children.push((*child, id));
    }
    wait_for_the_clock();

    // The runs the row wants already over are cancelled and settled.
    let mut any_finished = false;
    for (child, id) in &children {
        if *child == Child::FinishedRun {
            cancel_run(emr, &cluster, id).await.ok(case.label);
            any_finished = true;
        }
    }
    if any_finished {
        wait_for_the_clock();
    }

    // Last, the children that must still be in flight when the teardown starts.
    for (slot, child) in case.children.iter().enumerate() {
        if *child == Child::CreatingEndpoint {
            let created = create_endpoint(
                emr,
                &cluster,
                endpoint_body(
                    &format!("cascade-{index}-{slot}"),
                    &format!("ep-tok-cascade-{index}-{slot}"),
                    "JUPYTER_SERVER",
                ),
            )
            .await;
            children.push((
                *child,
                text(created.ok(case.label), "/id", case.label),
            ));
        }
    }
    for (child, id) in &children {
        if *child == Child::CancellingRun {
            cancel_run(emr, &cluster, id).await.ok(case.label);
        }
    }

    (cluster, children)
}

/// THE RULE: a cluster's teardown reaches every resource the cluster holds.
#[tokio::test]
async fn a_cluster_teardown_carries_down_the_endpoints_and_runs_it_holds() {
    let cases = [
        TeardownCase {
            label: "a cluster holding nothing",
            scheduler: None,
            children: Vec::new(),
            queued_and_running_before: (0, 0),
        },
        TeardownCase {
            label: "an active endpoint and a started run",
            scheduler: None,
            children: vec![Child::ActiveEndpoint, Child::StartedRun],
            queued_and_running_before: (0, 1),
        },
        TeardownCase {
            label: "three endpoints, one of them still creating",
            scheduler: None,
            children: vec![
                Child::ActiveEndpoint,
                Child::ActiveEndpoint,
                Child::CreatingEndpoint,
            ],
            queued_and_running_before: (0, 0),
        },
        TeardownCase {
            // `maxConcurrentJobRuns` 1 leaves the second submission waiting for the slot
            // the first holds, so the row carries a run in each of the two live states —
            // which `queued_and_running_before` is what pins.
            label: "a run waiting behind the cluster's concurrency limit",
            scheduler: Some(json!({"maxConcurrentJobRuns": 1})),
            children: vec![
                Child::ActiveEndpoint,
                Child::StartedRun,
                Child::StartedRun,
            ],
            queued_and_running_before: (1, 1),
        },
        TeardownCase {
            label: "a run that had already finished and a cancellation in flight",
            scheduler: None,
            children: vec![
                Child::FinishedRun,
                Child::CancellingRun,
                Child::CreatingEndpoint,
            ],
            // A cancelled run and one whose cancellation is in flight are in neither set.
            queued_and_running_before: (0, 0),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emr = Emr::new();
        let label = case.label;
        let (cluster, children) = set_up(&emr, case, index).await;

        // What the row set up, before the teardown: the premise every assertion below is
        // about.
        for (child, id) in &children {
            let before = if child.is_endpoint() {
                endpoint_state(&emr, label, &cluster, id).await
            } else {
                run_state(&emr, label, &cluster, id).await
            };
            let expected: Vec<&str> = match child {
                Child::ActiveEndpoint => vec![state::ENDPOINT_ACTIVE],
                Child::CreatingEndpoint => vec![state::ENDPOINT_CREATING],
                Child::StartedRun => vec![state::JOB_RUNNING, state::JOB_PENDING],
                Child::FinishedRun => vec![state::JOB_CANCELLED],
                Child::CancellingRun => vec![state::JOB_CANCEL_PENDING],
            };
            assert!(
                expected.contains(&before.as_str()),
                "case {label:?}: {child:?} {id} must be in one of {expected:?} before the \
                 teardown, got {before:?}"
            );
        }
        // Which of those runs the cluster counts as queued and as running: the row that
        // means to leave a submission waiting behind the concurrency limit is pinned here,
        // and this is the pair the teardown has to take to zero.
        let status = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            status, case.queued_and_running_before,
            "case {label:?}: before the teardown the cluster must report \
             {:?} queued and running job runs, got {status:?}",
            case.queued_and_running_before
        );

        emr.delete_cluster(&cluster).await.ok(label);

        // The teardown is in flight: the cluster is TERMINATING, and so is every child
        // that had not already ended.
        let reported = cluster_state(&emr, label, &cluster).await;
        assert_eq!(
            reported,
            state::CLUSTER_TERMINATING,
            "case {label:?}: the delete must leave the cluster TERMINATING, got {reported:?}"
        );
        for (child, id) in &children {
            let (reported, expected) = if child.is_endpoint() {
                (
                    endpoint_state(&emr, label, &cluster, id).await,
                    state::ENDPOINT_TERMINATING,
                )
            } else {
                (
                    run_state(&emr, label, &cluster, id).await,
                    match child {
                        // A run that was already over keeps the end it had; one whose
                        // cancellation was already in flight keeps that cancellation.
                        Child::FinishedRun => state::JOB_CANCELLED,
                        _ => state::JOB_CANCEL_PENDING,
                    },
                )
            };
            assert_eq!(
                reported, expected,
                "case {label:?}: {child:?} {id} must be {expected} while its cluster is \
                 torn down, got {reported:?}"
            );
        }
        // No run of a cluster being torn down is queued or running any more, so the
        // scheduler counts the cluster reports are zero.
        let status = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            status,
            (0, 0),
            "case {label:?}: a cluster being torn down holds no queued or running work, \
             got {status:?}"
        );

        wait_for_the_clock();

        // The teardown has settled: every child reached the terminal state of its kind.
        let reported = cluster_state(&emr, label, &cluster).await;
        assert_eq!(
            reported,
            state::CLUSTER_TERMINATED,
            "case {label:?}: the clock owed this cluster TERMINATED, got {reported:?}"
        );
        for (child, id) in &children {
            if child.is_endpoint() {
                let reported = endpoint_state(&emr, label, &cluster, id).await;
                assert_eq!(
                    reported,
                    state::ENDPOINT_TERMINATED,
                    "case {label:?}: endpoint {id} of a torn-down cluster must be \
                     TERMINATED, got {reported:?}"
                );
                continue;
            }
            let reported = run_state(&emr, label, &cluster, id).await;
            assert_eq!(
                reported,
                state::JOB_CANCELLED,
                "case {label:?}: run {id} of a torn-down cluster must be CANCELLED, got \
                 {reported:?}"
            );
            assert!(
                run_has_finished_at(&emr, label, &cluster, id).await,
                "case {label:?}: run {id} reached a terminal state, so it must report a \
                 finishedAt"
            );
        }
        let status = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            status,
            (0, 0),
            "case {label:?}: a torn-down cluster holds no queued or running work, got \
             {status:?}"
        );

        // The teardown ENDED the children, it did not forget them: the cluster still lists
        // exactly the endpoints it held.
        let held: Vec<String> = children
            .iter()
            .filter(|(child, _)| child.is_endpoint())
            .map(|(_, id)| id.clone())
            .collect();
        let answer = list_endpoints(&emr, &cluster, "").await;
        let body = answer.ok(label).clone();
        let mut listed = listed_ids(&body, label);
        listed.sort();
        let mut expected = held;
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {label:?}: a torn-down cluster must still list the endpoints it held: \
             {body}"
        );
    }
}

/// The states the teardown writes for a managed endpoint are members of the model's own
/// `EndpointState` enum, so no response carries a state a client would not accept.
///
/// Source: the generated `ENDPOINT_STATE_VALUES` catalog in `ops::types::shared_types`.
#[test]
fn the_endpoint_states_a_teardown_writes_are_modelled_ones() {
    for state_value in [state::ENDPOINT_TERMINATING, state::ENDPOINT_TERMINATED] {
        assert!(
            ENDPOINT_STATE_VALUES.contains(&state_value),
            "case {state_value:?}: not an EndpointState; the model allows \
             {ENDPOINT_STATE_VALUES:?}"
        );
    }
    assert!(
        state::endpoint_state_is_terminal(state::ENDPOINT_TERMINATED),
        "case {:?}: the state a teardown settles on must be terminal",
        state::ENDPOINT_TERMINATED
    );
    assert!(
        !state::endpoint_state_is_terminal(state::ENDPOINT_TERMINATING),
        "case {:?}: a teardown in flight has not ended yet",
        state::ENDPOINT_TERMINATING
    );
}

// SPDX-License-Identifier: CC0-1.0

//! WHICH WORDING a refusal carries when the resource EXISTS and only its STATE forbids
//! the call.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop`]), and driven with restJson1 HTTP requests, so
//! routing, deserialization, constraint validation, the handler and serialization all run
//! exactly as they do for an AWS SDK client.
//!
//! This is a DIFFERENT class of refusal from a resource that is not there at all
//! ([`crate::not_found_message`] and [`crate::refusal_wording_by_error_class`] pin that
//! one), and it is worded differently:
//!
//! 1. A `CancelJobRun` for a run whose `JobRunState` a cancellation can no longer reach is
//!    refused with `Job run <id> is not in a cancellable state` — the request's job run id
//!    is the only variable part, and the state the run settled in is NOT named.
//! 2. A call whose target virtual cluster is stored but whose `VirtualClusterState`
//!    forbids it is refused with
//!    `Virtual cluster state <state> is not valid to create resource <Operation>.` — the
//!    cluster's current state and the refused operation are the variable parts, the
//!    cluster's id appears nowhere, and the sentence ends in a period.
//!
//! Both refusals are carried by the modelled `ValidationException` at HTTP 400, and both
//! leave the resource exactly as they found it: a refused call is not a mutation.
//!
//! The tables span what the rules generalise over: for rule 1, which terminal
//! `JobRunState` the run reached and by which route (a cancellation the caller asked for,
//! and the outcome the clock owes a run nobody cancels), plus the boundary — a run still
//! in a state a cancellation can reach is not refused at all. For rule 2, the product of
//! which non-RUNNING state the cluster is in (both phases of a teardown) and which
//! operation is refused (`UpdateVirtualCluster` and `DeleteVirtualCluster`), with a
//! RUNNING cluster as the positive control for each.

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::ops::types::{
    JOB_RUN_STATE_VALUES, VIRTUAL_CLUSTER_STATE_VALUES,
};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::job_run_outcome::poll_until_terminal;
use crate::virtual_cluster_job_run_loop::{field, text, Answer, Emr};
use crate::virtual_cluster_scheduler_status::{
    cancel_run, cluster_id, run_id, run_state, wait_for_the_clock,
};
use crate::virtual_cluster_update::update_cluster;

/// The client error both refusals are carried by: the only client error `CancelJobRun` and
/// `DeleteVirtualCluster` model at all, and the one `UpdateVirtualCluster` reports an
/// invalid request in (smithy_query: operation
/// `CancelJobRun,UpdateVirtualCluster,DeleteVirtualCluster`).
const INVALID_REQUEST: &str = "ValidationException";

/// The `schedulerConfiguration` every call below submits, so a refusal that nevertheless
/// applied the update would show up on the cluster afterwards.
fn submitted_limit() -> Value {
    json!({"maxConcurrentJobRuns": 5})
}

/// AWS's sentence for a job run a cancellation can no longer reach, written here as the
/// wording under test rather than taken from the service, with only the requested id
/// interpolated.
fn not_cancellable(run: &str) -> String {
    format!("Job run {run} is not in a cancellable state")
}

/// AWS's sentence for a call the cluster's state forbids: the state and the operation
/// name, and no cluster id.
fn cluster_state_refusal(cluster_state: &str, operation: &str) -> String {
    format!("Virtual cluster state {cluster_state} is not valid to create resource {operation}.")
}

/// The `message` a refusal carries.
fn message_of(answer: &Answer, case: &str) -> String {
    text(&answer.body, "/message", case)
}

/// The `virtualCluster` `DescribeVirtualCluster` reports for this cluster.
async fn described_cluster(emr: &Emr, case: &str, id: &str) -> Value {
    let answer = emr.send("GET", &format!("/virtualclusters/{id}"), None).await;
    field(answer.ok(case), "/virtualCluster", case).clone()
}

/// The terminal state the run has settled in, waiting for the outcome the clock owes it
/// ([`poll_until_terminal`] is this binary's one such wait).
async fn settled_state(emr: &Emr, case: &str, cluster: &str, run: &str) -> String {
    let settled = poll_until_terminal(emr, case, cluster, run).await;
    text(&settled, "/state", case)
}

/// How a row of rule 1 drives its run into a terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// The caller cancels the run, and the clock carries the cancellation out.
    Cancelled,
    /// Nobody cancels the run, and the clock settles its outcome.
    LeftAlone,
}

/// One row of rule 1: the route into a terminal state, and the state it ends in.
struct TerminalRun {
    label: &'static str,
    route: Route,
    /// The terminal `JobRunState` the route leaves the run in.
    expected_state: &'static str,
}

/// RULE 1: `CancelJobRun` for a run already past cancelling is refused with AWS's
/// sentence, which names the requested run and nothing else — and the run stays exactly
/// where it was.
#[tokio::test]
async fn cancelling_a_terminal_job_run_is_refused_with_the_cancellable_state_wording() {
    let emr = Emr::new();
    let rows = [
        TerminalRun {
            label: "a run the caller cancelled",
            route: Route::Cancelled,
            expected_state: state::JOB_CANCELLED,
        },
        TerminalRun {
            label: "a run nobody cancelled, settled by the clock",
            route: Route::LeftAlone,
            expected_state: state::JOB_FAILED,
        },
    ];
    for row in rows {
        let case = row.label;
        assert!(
            JOB_RUN_STATE_VALUES.contains(&row.expected_state),
            "case {case:?}: {:?} is not a modelled JobRunState",
            row.expected_state
        );
        let cluster = cluster_id(
            &emr,
            case,
            &format!("cancel-wording-{:?}", row.route).to_lowercase(),
            &format!("tok-cancel-wording-{:?}", row.route),
            None,
        )
        .await;
        let run = run_id(
            &emr,
            case,
            &cluster,
            &format!("tok-run-cancel-wording-{:?}", row.route),
        )
        .await;

        let settled = match row.route {
            Route::Cancelled => {
                cancel_run(&emr, &cluster, &run).await.ok(case);
                // THE BOUNDARY: a cancellation already accepted but not yet carried out
                // leaves the run in a state a cancellation can still reach, so the retry
                // is answered rather than refused.
                let pending = run_state(&emr, case, &cluster, &run).await;
                assert_eq!(
                    pending,
                    state::JOB_CANCEL_PENDING,
                    "case {case:?}: an accepted cancellation leaves the run pending, got \
                     {pending:?}"
                );
                cancel_run(&emr, &cluster, &run).await.ok(case);
                wait_for_the_clock();
                settled_state(&emr, case, &cluster, &run).await
            }
            Route::LeftAlone => settled_state(&emr, case, &cluster, &run).await,
        };
        assert_eq!(
            settled, row.expected_state,
            "case {case:?}: expected the run to settle in {:?}, got {settled:?}",
            row.expected_state
        );

        // The refusal, twice: the wording is the same sentence every time, and neither
        // call moves the run.
        let expected = not_cancellable(&run);
        for attempt in 1..=2 {
            let refusal = cancel_run(&emr, &cluster, &run).await;
            refusal.refused(case, 400, INVALID_REQUEST);
            let message = message_of(&refusal, case);
            assert_eq!(
                message, expected,
                "case {case:?} attempt {attempt}: expected message {expected:?}, got \
                 {message:?}"
            );
            // The sentence reports that the run is past cancelling, not which state it
            // settled in.
            assert!(
                !message.contains(&settled),
                "case {case:?} attempt {attempt}: expected a sentence naming no state, got \
                 {message:?}"
            );
            let after = run_state(&emr, case, &cluster, &run).await;
            assert_eq!(
                after, settled,
                "case {case:?} attempt {attempt}: a refused cancel must leave the run in \
                 {settled:?}, got {after:?}"
            );
        }
    }
}

/// Which phase of a teardown a row of rule 2 drives the cluster into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The teardown is in flight: the cluster is TERMINATING.
    InFlight,
    /// The clock has carried the teardown out: the cluster is TERMINATED.
    Settled,
}

/// One operation of rule 2: the request that reaches the cluster, and the name the model
/// gives that operation (which is the name the refusal must carry).
struct ClusterCall {
    operation: &'static str,
    method: &'static str,
    /// The request body, where the operation has one.
    body: Option<Value>,
}

/// RULE 2: a call the cluster's state forbids is refused with AWS's sentence — the state
/// that blocked it and the operation it blocked, with no cluster id — and nothing about
/// the cluster moves.
///
/// The rows are the product of the two teardown phases and the two operations this
/// service refuses on a cluster's state. `ARRESTED`, the fourth modelled
/// `VirtualClusterState`, is a state this emulator never puts a cluster in, so no row can
/// reach it; the refusal takes whichever state the cluster is stored in, so the rule holds
/// for it by construction.
#[tokio::test]
async fn a_call_a_virtual_clusters_state_forbids_names_the_state_and_the_operation() {
    let emr = Emr::new();
    let calls = [
        ClusterCall {
            operation: "UpdateVirtualCluster",
            method: "PATCH",
            body: Some(json!({
                "clientToken": "tok-state-refusal-update",
                "schedulerConfiguration": submitted_limit(),
            })),
        },
        ClusterCall {
            operation: "DeleteVirtualCluster",
            method: "DELETE",
            body: None,
        },
    ];
    for phase in [Phase::InFlight, Phase::Settled] {
        for call in &calls {
            let case = format!("{} on a cluster {:?}", call.operation, phase);
            let case = case.as_str();
            let fixture = format!(
                "state-refusal-{}-{:?}",
                call.operation.to_lowercase(),
                phase
            )
            .to_lowercase();
            let cluster = cluster_id(&emr, case, &fixture, &format!("tok-{fixture}"), None).await;

            // The positive control: the very same call against the cluster while it is
            // RUNNING is ACCEPTED, so what the rows below exercise is the state guard and
            // not a request the service refuses anyway.
            if call.operation == "UpdateVirtualCluster" {
                let accepted = update_cluster(
                    &emr,
                    &cluster,
                    "tok-state-refusal-control",
                    Some(submitted_limit()),
                )
                .await;
                let applied = field(accepted.ok(case), "/virtualCluster", case).clone();
                assert_eq!(
                    applied.get("schedulerConfiguration").cloned(),
                    Some(submitted_limit()),
                    "case {case:?}: the control update must persist the submitted limit, got \
                     {applied}"
                );
            }

            emr.delete_cluster(&cluster).await.ok(case);
            if phase == Phase::Settled {
                wait_for_the_clock();
            }
            let blocking_state = text(&described_cluster(&emr, case, &cluster).await, "/state", case);
            let expected_state = match phase {
                Phase::InFlight => state::CLUSTER_TERMINATING,
                Phase::Settled => state::CLUSTER_TERMINATED,
            };
            assert_eq!(
                blocking_state, expected_state,
                "case {case:?}: expected the cluster in {expected_state:?}, got \
                 {blocking_state:?}"
            );
            assert!(
                VIRTUAL_CLUSTER_STATE_VALUES.contains(&blocking_state.as_str()),
                "case {case:?}: {blocking_state:?} is not a modelled VirtualClusterState"
            );

            let refusal = emr
                .send(call.method, &format!("/virtualclusters/{cluster}"), call.body.clone())
                .await;
            refusal.refused(case, 400, INVALID_REQUEST);
            let message = message_of(&refusal, case);
            let expected = cluster_state_refusal(&blocking_state, call.operation);
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
            // The other half of the wording: this class discloses the state, not the
            // resource the caller asked for.
            assert!(
                !message.contains(&cluster),
                "case {case:?}: expected a sentence naming no cluster id, got {message:?}"
            );

            // The refusal is not a mutation: a cluster being torn down keeps the limit the
            // control update left on it and does not pick up a later one.
            let after = described_cluster(&emr, case, &cluster).await;
            let expected_configuration = match call.operation {
                "UpdateVirtualCluster" => Some(submitted_limit()),
                _ => None,
            };
            assert_eq!(
                after.get("schedulerConfiguration").cloned(),
                expected_configuration,
                "case {case:?}: a refused call must leave the cluster's configuration \
                 untouched, got {after}"
            );
            let after_state = text(&after, "/state", case);
            assert_ne!(
                after_state,
                state::CLUSTER_RUNNING,
                "case {case:?}: a refused call must not bring the cluster back, got {after}"
            );
        }
    }
}

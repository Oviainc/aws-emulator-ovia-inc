// SPDX-License-Identifier: CC0-1.0

//! `VirtualCluster.schedulerStatus` is SCOPED to the cluster that is described.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop`]), and the endpoints are driven through the
//! helpers of [`crate::virtual_cluster_scheduler_status`], so there is one definition of
//! each request rather than a second copy here.
//!
//! The rule under test — the one its sibling suite does not cover, because every case
//! there lives on a single cluster:
//!
//! A cluster's `schedulerStatus` counts the job runs the service holds for THAT cluster.
//! Several clusters hold runs at once, each with a scheduler configuration of its own (or
//! none at all), and no cluster's counts move because a neighbour's runs were submitted,
//! started or cancelled.

use serde_json::json;

use crate::virtual_cluster_job_run_loop::Emr;
use crate::virtual_cluster_scheduler_status::{
    cancel_run, cluster_id, run_id, run_state, scheduler_status, wait_for_the_clock,
};

/// One cluster in the scoping table: the scheduler configuration it is created with, how
/// many runs are submitted to it, and the `(currentInQueueJobRuns,
/// currentConcurrentJobRuns)` it must report once the clock has started what it can.
struct ScopedCluster {
    label: &'static str,
    scheduler: Option<serde_json::Value>,
    runs: i64,
    settled_status: (i64, i64),
}

/// The counts belong to the cluster that is described, not to the service.
///
/// The rows differ along what the rule generalises over: how many runs a cluster holds
/// (none, one, several), whether it limits concurrency at all, and whether its limit
/// leaves part of its work queued — so a count that was taken from the whole store, or
/// from the wrong cluster, cannot match any of them.
#[tokio::test]
async fn scheduler_status_counts_only_the_described_clusters_runs() {
    let emr = Emr::new();
    let case = "four clusters side by side";
    let clusters = [
        ScopedCluster {
            label: "one running slot, three runs: one running and two queued",
            scheduler: Some(json!({"maxConcurrentJobRuns": 1})),
            runs: 3,
            settled_status: (2, 1),
        },
        ScopedCluster {
            label: "two running slots, three runs: two running and one queued",
            scheduler: Some(json!({"maxConcurrentJobRuns": 2, "maxInQueueJobRuns": 100})),
            runs: 3,
            settled_status: (1, 2),
        },
        ScopedCluster {
            label: "no scheduler configuration, two runs: both running",
            scheduler: None,
            runs: 2,
            settled_status: (0, 2),
        },
        ScopedCluster {
            label: "a configured but idle cluster, holding no run of its own",
            scheduler: Some(json!({"maxConcurrentJobRuns": 4})),
            runs: 0,
            settled_status: (0, 0),
        },
    ];

    // Every cluster is created and filled before anything is asserted, so each one's
    // counts are read while its neighbours hold queued and running work of their own.
    let mut ids = Vec::new();
    let mut runs = Vec::new();
    for (index, cluster) in clusters.iter().enumerate() {
        let id = cluster_id(
            &emr,
            cluster.label,
            &format!("scoped-{index}"),
            &format!("tok-scoped-{index}"),
            cluster.scheduler.clone(),
        )
        .await;
        let mut submitted = Vec::new();
        for run in 0..cluster.runs {
            submitted.push(
                run_id(
                    &emr,
                    cluster.label,
                    &id,
                    &format!("tok-scoped-{index}-{run}"),
                )
                .await,
            );
        }
        ids.push(id);
        runs.push(submitted);
    }

    wait_for_the_clock();
    for (cluster, id) in clusters.iter().zip(&ids) {
        let observed = scheduler_status(&emr, cluster.label, id).await;
        assert_eq!(
            observed, cluster.settled_status,
            "case {case:?}: cluster {:?} must report {:?} of its own, got {observed:?}",
            cluster.label, cluster.settled_status
        );
    }

    // Cancelling a run on the FIRST cluster moves that cluster's counts (the cancelled
    // run leaves both sets and the oldest queued run takes its slot) and no other's.
    let busy = &ids[0];
    cancel_run(&emr, busy, &runs[0][0]).await.ok(case);
    wait_for_the_clock();
    let finished = run_state(&emr, case, busy, &runs[0][0]).await;
    assert_eq!(
        finished, "CANCELLED",
        "case {case:?}: the cancelled run must have finished, got {finished:?}"
    );
    let observed = scheduler_status(&emr, case, busy).await;
    assert_eq!(
        observed,
        (1, 1),
        "case {case:?}: the cancelled run must leave the queue to its successor, got \
         {observed:?}"
    );
    for (cluster, id) in clusters.iter().zip(&ids).skip(1) {
        let observed = scheduler_status(&emr, cluster.label, id).await;
        assert_eq!(
            observed, cluster.settled_status,
            "case {case:?}: cluster {:?} must be unmoved by a neighbour's cancellation, \
             expected {:?}, got {observed:?}",
            cluster.label, cluster.settled_status
        );
    }
}

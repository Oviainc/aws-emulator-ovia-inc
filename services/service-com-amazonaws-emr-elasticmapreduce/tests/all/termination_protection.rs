// SPDX-License-Identifier: CC0-1.0

//! `SetTerminationProtection`, end to end over the wire.
//!
//! THE RULE this suite asserts: `SetTerminationProtection` resolves every id in its
//! `JobFlowIds` batch against the service's own clusters, refuses the whole batch when
//! one id names no cluster, and otherwise writes the requested `TerminationProtected`
//! value on each named cluster — so a later `DescribeCluster` reports exactly that value
//! and `TerminateJobFlows` leaves a locked cluster alone until a later call unlocks it.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns. Nothing reaches
//! into the service's state struct: what was written is only ever read back through
//! `DescribeCluster` and through what `TerminateJobFlows` does next.

use std::time::Duration;

use serde_json::json;

use crate::emr_control_plane::{MAX_JOB_FLOW_IDS_PER_REQUEST, UNKNOWN_CLUSTER_IDS};
use crate::job_flow_lifecycle::{assert_refused, at, kept_alive_request, minimal_request, Emr};

/// Longer than the service's clock-owed transition span (500 ms), so a cluster that was
/// `TERMINATING` has reached `TERMINATED` by the time we look.
///
/// `pub(crate)` so a sibling suite waits out the same span through one constant rather than
/// its own copy.
pub(crate) const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// The states a cluster that nothing terminated can be reported in: it was launched
/// `STARTING` and the clock owes it `WAITING` (it is kept alive with no steps) or
/// `RUNNING` (`com.amazonaws.emr#ClusterState`).
const NOT_SHUTTING_DOWN: [&str; 3] = ["STARTING", "WAITING", "RUNNING"];

/// A `RunJobFlow` request launching a cluster with `TerminationProtected` already set to
/// `protected`, so a row can assert the write both as a change and as a no-op.
fn launched_with_protection(name: &str, protected: bool) -> serde_json::Value {
    json!({
        "Name": name,
        "Instances": {
            "InstanceCount": 1,
            "MasterInstanceType": "m5.xlarge",
            "KeepJobFlowAliveWhenNoSteps": true,
            "TerminationProtected": protected,
        },
    })
}

/// `SetTerminationProtection` writes the lock on every cluster it names, and
/// `DescribeCluster` reports what it wrote. The table spans the value the cluster was
/// LAUNCHED with, the requested value, the batch size and a repeat of the same value.
#[tokio::test]
async fn set_termination_protection_writes_what_describe_cluster_reports() {
    let emr = Emr::new();
    let launched_off = emr.run_job_flow(launched_with_protection("off", false)).await;
    let launched_on = emr.run_job_flow(launched_with_protection("on", true)).await;
    // A cluster whose request names no `TerminationProtected` at all, and a transient
    // one, so the write is asserted on each launch form the operation can be called on.
    let defaulted = emr.run_job_flow(kept_alive_request("defaulted")).await;
    let transient = emr.run_job_flow(minimal_request("transient")).await;

    for (label, ids, requested) in [
        ("one cluster, locked", vec![launched_off.clone()], true),
        ("one cluster, unlocked", vec![launched_on.clone()], false),
        (
            "the same cluster, locked again",
            vec![launched_on.clone()],
            false,
        ),
        (
            "two clusters at once, both locked",
            vec![defaulted.clone(), transient.clone()],
            true,
        ),
        (
            "four clusters at once, all unlocked",
            vec![
                launched_off.clone(),
                launched_on.clone(),
                defaulted.clone(),
                transient.clone(),
            ],
            false,
        ),
        (
            "the same four, all locked",
            vec![
                launched_off.clone(),
                launched_on.clone(),
                defaulted.clone(),
                transient.clone(),
            ],
            true,
        ),
    ] {
        emr.ok(
            "SetTerminationProtection",
            json!({ "JobFlowIds": ids, "TerminationProtected": requested }),
        )
        .await;
        for id in &ids {
            let cluster = emr.describe_cluster(id).await;
            assert_eq!(
                cluster["TerminationProtected"].as_bool(),
                Some(requested),
                "case {label:?}: DescribeCluster must report the written value for {id}, \
                 got {cluster}"
            );
        }
    }

    // A cluster the request does not name keeps the value it holds: the last row above
    // locked all four, so locking only one of them leaves the others locked, and
    // unlocking only one leaves the others locked too.
    emr.ok(
        "SetTerminationProtection",
        json!({ "JobFlowIds": [&launched_off], "TerminationProtected": false }),
    )
    .await;
    for (id, expected) in [
        (&launched_off, false),
        (&launched_on, true),
        (&defaulted, true),
        (&transient, true),
    ] {
        let cluster = emr.describe_cluster(id).await;
        assert_eq!(
            cluster["TerminationProtected"].as_bool(),
            Some(expected),
            "case \"unnamed cluster\": {id} must still report {expected}, got {cluster}"
        );
    }
}

/// THE RULE's consequence: a locked cluster survives `TerminateJobFlows`, and "to
/// terminate a cluster that has been locked by setting SetTerminationProtection to true,
/// you must first unlock the job flow by a subsequent call to SetTerminationProtection in
/// which you set the value to false" (`SetTerminationProtection` doc) — after which the
/// same call shuts it down.
#[tokio::test]
async fn termination_protection_blocks_terminate_job_flows_until_it_is_unlocked() {
    let emr = Emr::new();

    // The rows differ in HOW the cluster came to be locked: by its launch request, or by
    // this operation on a cluster that launched unlocked.
    for (label, request, lock_first) in [
        ("locked at launch", launched_with_protection("at-launch", true), false),
        ("locked by the API call", kept_alive_request("by-api"), true),
    ] {
        let cluster_id = emr.run_job_flow(request).await;
        if lock_first {
            emr.ok(
                "SetTerminationProtection",
                json!({ "JobFlowIds": [&cluster_id], "TerminationProtected": true }),
            )
            .await;
        }

        emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [&cluster_id] }))
            .await;
        tokio::time::sleep(PAST_TRANSITION).await;
        let locked = emr.describe_cluster(&cluster_id).await;
        let state = at(&locked, &["Status", "State"]).as_str().unwrap_or_default();
        assert!(
            NOT_SHUTTING_DOWN.contains(&state),
            "case {label:?}: a locked cluster must not be terminated by an API call, \
             got state {state:?} in {locked}"
        );
        assert!(
            at(&locked, &["Status", "StateChangeReason"]).is_null(),
            "case {label:?}: nothing changed a locked cluster's state, got {locked}"
        );
        assert!(
            at(&locked, &["Status", "Timeline", "EndDateTime"]).is_null(),
            "case {label:?}: a locked cluster has not ended, got {locked}"
        );

        // Unlocking it is what lets the same call through.
        emr.ok(
            "SetTerminationProtection",
            json!({ "JobFlowIds": [&cluster_id], "TerminationProtected": false }),
        )
        .await;
        emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [&cluster_id] }))
            .await;
        let shutting_down = emr.describe_cluster(&cluster_id).await;
        assert_eq!(
            at(&shutting_down, &["Status", "State"]).as_str(),
            Some("TERMINATING"),
            "case {label:?}: an unlocked cluster must start shutting down, got {shutting_down}"
        );
        assert_eq!(
            at(&shutting_down, &["Status", "StateChangeReason", "Code"]).as_str(),
            Some("USER_REQUEST"),
            "case {label:?}: the reason must be USER_REQUEST, got {shutting_down}"
        );

        tokio::time::sleep(PAST_TRANSITION).await;
        let gone = emr.describe_cluster(&cluster_id).await;
        assert_eq!(
            at(&gone, &["Status", "State"]).as_str(),
            Some("TERMINATED"),
            "case {label:?}: the unlocked cluster must reach TERMINATED, got {gone}"
        );
        assert!(
            !at(&gone, &["Status", "Timeline", "EndDateTime"]).is_null(),
            "case {label:?}: a terminated cluster carries an EndDateTime, got {gone}"
        );

        // Locking a cluster that is already gone changes neither its state nor the fact
        // that the lock is the cluster's own setting.
        emr.ok(
            "SetTerminationProtection",
            json!({ "JobFlowIds": [&cluster_id], "TerminationProtected": true }),
        )
        .await;
        let after = emr.describe_cluster(&cluster_id).await;
        assert_eq!(
            at(&after, &["Status", "State"]).as_str(),
            Some("TERMINATED"),
            "case {label:?}: locking a terminated cluster must not revive it, got {after}"
        );
        assert_eq!(
            after["TerminationProtected"].as_bool(),
            Some(true),
            "case {label:?}: the written lock must still be reported, got {after}"
        );
    }
}

/// An id no cluster carries refuses the WHOLE batch with the 400 `ValidationException`
/// channel (this operation's model declares no client error) and changes nothing,
/// however many known ids the same request carries.
#[tokio::test]
async fn set_termination_protection_refuses_an_unknown_job_flow_id() {
    let emr = Emr::new();
    let known = emr.run_job_flow(kept_alive_request("known")).await;

    for unknown in UNKNOWN_CLUSTER_IDS {
        let (status, payload) = emr
            .call(
                "SetTerminationProtection",
                json!({
                    "JobFlowIds": [&known, unknown],
                    "TerminationProtected": true,
                }),
            )
            .await;
        assert_refused(
            &format!("job flow id {unknown:?}"),
            status,
            &payload,
            "ValidationException",
        );
        let cluster = emr.describe_cluster(&known).await;
        assert_eq!(
            cluster["TerminationProtected"].as_bool(),
            Some(false),
            "case {unknown:?}: a refused request must change nothing, got {cluster}"
        );
    }
}

/// A batch of job flow ids is capped at the same number `TerminateJobFlows` publishes
/// for its identical member ("The maximum number of clusters allowed is 10"), since this
/// operation's own model publishes none: a batch AT the bound is accepted and one past it
/// is refused before any cluster is touched.
#[tokio::test]
async fn set_termination_protection_caps_the_batch_of_job_flow_ids() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(kept_alive_request("batched")).await;

    let at_the_bound: Vec<&String> = (0..MAX_JOB_FLOW_IDS_PER_REQUEST)
        .map(|_| &cluster_id)
        .collect();
    emr.ok(
        "SetTerminationProtection",
        json!({ "JobFlowIds": at_the_bound, "TerminationProtected": true }),
    )
    .await;
    let cluster = emr.describe_cluster(&cluster_id).await;
    assert_eq!(
        cluster["TerminationProtected"].as_bool(),
        Some(true),
        "case \"at the maximum\": the batch must be applied, got {cluster}"
    );

    let past_the_bound: Vec<&String> = (0..=MAX_JOB_FLOW_IDS_PER_REQUEST)
        .map(|_| &cluster_id)
        .collect();
    let (status, payload) = emr
        .call(
            "SetTerminationProtection",
            json!({ "JobFlowIds": past_the_bound, "TerminationProtected": false }),
        )
        .await;
    assert_refused(
        "one job flow id past the maximum",
        status,
        &payload,
        "ValidationException",
    );
    let cluster = emr.describe_cluster(&cluster_id).await;
    assert_eq!(
        cluster["TerminationProtected"].as_bool(),
        Some(true),
        "case \"past the maximum\": a refused batch must change nothing, got {cluster}"
    );
}

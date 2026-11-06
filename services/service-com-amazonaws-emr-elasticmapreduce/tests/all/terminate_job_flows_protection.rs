// SPDX-License-Identifier: CC0-1.0

//! What `TerminateJobFlows` answers for a cluster that is locked against termination,
//! end to end over the wire.
//!
//! THE RULE this suite asserts: a shutdown request is refused in whole — HTTP 400 on the
//! `ValidationException` channel, carrying one CONSTANT sentence with no variable part —
//! whenever ANY cluster it names still holds termination protection, and it shuts none of
//! the clusters it names down; the lock holds only while the cluster still has instances
//! to lose, so a cluster that is already shutting down or gone keeps answering the
//! idempotent 200, and clearing the lock is what lets the identical call through.
//!
//! The sibling suite `tests/all/termination_protection.rs` spans the WRITE of the lock and
//! the unlock-then-terminate sequence on one id; this one spans the BATCH the refusal is
//! decided over, the cluster states the lock is evaluated in, and the structured record
//! each branch of the shutdown leaves.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so these are the
//! same bytes an SDK client sends. Nothing reaches into the service's state struct: the
//! effect of each call is read back through `DescribeCluster`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::emr_control_plane::MAX_JOB_FLOW_IDS_PER_REQUEST;
use crate::job_flow_lifecycle::{assert_refused, at, kept_alive_request, Emr, REQUEST_TARGET};
use crate::refusal_messages::message;
use crate::security_configurations::audit_capture;
use crate::termination_protection::{launched_with_protection, PAST_TRANSITION};

/// The sentence Amazon EMR refuses a shutdown of a locked cluster with. Recorded
/// behaviour: it is entirely fixed text — plural and id-free, because the whole batch is
/// judged before anything is shut down — so no id, name, count or timestamp of the
/// request or of the state appears in it.
const PROTECTED_SENTENCE: &str =
    "Could not shut down one or more job flows since they are termination protected.";

/// The access key id this suite's audit case presents, which no other suite uses, so the
/// shared audit capture can tell its records from a sibling's.
const AUDIT_AKID: &str = "AKIDEMRTERMINATEJOBFLOWS";

/// One row of the audit table: a label, the `JobFlowIds` batch the request names, and the
/// (resource, outcome) pairs of the records that call must leave, oldest first.
type AuditedBatch<'a> = (&'a str, Vec<&'a String>, Vec<(&'a str, &'a str)>);

/// An id no cluster is held under, for the row that asserts the record a lookup refusal
/// leaves.
const UNKNOWN_ID: &str = "j-NOSUCHCLUSTER";

/// That a refusal is this surface's refusal of a locked shutdown: HTTP 400, the
/// `ValidationException` channel (this operation's model declares no client error) and the
/// sentence EQUAL to the constant, which is what proves nothing was interpolated into it
/// or appended to it. Answers the message so a caller can collect it.
fn assert_protected_refusal(case: &str, status: u16, payload: &Value) -> String {
    assert_refused(case, status, payload, "ValidationException");
    let said = message(case, payload);
    assert_eq!(
        said, PROTECTED_SENTENCE,
        "case {case:?}: expected {PROTECTED_SENTENCE:?}, got {payload}"
    );
    said
}

/// That nothing shut `cluster_id` down: it is in neither shutdown state and carries
/// neither a state-change reason nor an end.
async fn assert_not_shutting_down(emr: &Emr, case: &str, cluster_id: &str) {
    let cluster = emr.describe_cluster(cluster_id).await;
    let state = at(&cluster, &["Status", "State"])
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        state != "TERMINATING" && state != "TERMINATED",
        "case {case:?}: {cluster_id} must not be shut down, got state {state:?} in {cluster}"
    );
    assert!(
        at(&cluster, &["Status", "StateChangeReason"]).is_null(),
        "case {case:?}: nothing changed {cluster_id}'s state, got {cluster}"
    );
    assert!(
        at(&cluster, &["Status", "Timeline", "EndDateTime"]).is_null(),
        "case {case:?}: {cluster_id} has not ended, got {cluster}"
    );
}

/// THE RULE over the BATCH: every shape of `JobFlowIds` carrying a locked cluster is
/// refused with the one constant sentence, and NEITHER the locked cluster nor the
/// unlocked ones beside it in the same batch are shut down.
///
/// Rows differ along what the rule generalises over: how the cluster came to be locked,
/// the size of the batch, where the locked id sits in it, how many locked ids it carries,
/// whether the same one repeats, and a batch at the cap this member takes ("The maximum
/// number of clusters allowed is 10", `TerminateJobFlows` doc).
#[tokio::test]
async fn terminate_job_flows_refuses_every_batch_naming_a_locked_cluster() {
    let emr = Emr::new();
    let locked_at_launch = emr
        .run_job_flow(launched_with_protection("at-launch", true))
        .await;
    let locked_by_api = emr.run_job_flow(kept_alive_request("by-api")).await;
    emr.ok(
        "SetTerminationProtection",
        json!({ "JobFlowIds": [&locked_by_api], "TerminationProtected": true }),
    )
    .await;
    let unlocked = emr
        .run_job_flow(launched_with_protection("unlocked", false))
        .await;
    let also_unlocked = emr.run_job_flow(kept_alive_request("also-unlocked")).await;

    let mut at_the_cap: Vec<&String> = (1..MAX_JOB_FLOW_IDS_PER_REQUEST)
        .map(|_| &unlocked)
        .collect();
    at_the_cap.push(&locked_by_api);

    let batches: Vec<(&str, Vec<&String>)> = vec![
        ("the cluster locked at launch, alone", vec![&locked_at_launch]),
        ("the cluster locked by the API call, alone", vec![&locked_by_api]),
        (
            "the locked cluster after an unlocked one",
            vec![&unlocked, &locked_at_launch],
        ),
        (
            "the locked cluster before an unlocked one",
            vec![&locked_at_launch, &unlocked],
        ),
        (
            "the locked cluster between two unlocked ones",
            vec![&unlocked, &locked_by_api, &also_unlocked],
        ),
        (
            "the same locked cluster twice",
            vec![&locked_at_launch, &locked_at_launch],
        ),
        (
            "two differently locked clusters",
            vec![&locked_at_launch, &locked_by_api],
        ),
        ("a batch at the cap, one cluster locked", at_the_cap),
    ];

    let mut said = BTreeSet::new();
    for (label, ids) in batches {
        let (status, payload) = emr
            .call("TerminateJobFlows", json!({ "JobFlowIds": &ids }))
            .await;
        said.insert(assert_protected_refusal(label, status, &payload));
        for id in [&locked_at_launch, &locked_by_api, &unlocked, &also_unlocked] {
            assert_not_shutting_down(&emr, label, id).await;
        }
    }
    assert_eq!(
        said.len(),
        1,
        "the sentence is constant across every batch, got {said:?}"
    );

    // The lock is evaluated against the cluster's state NOW, so the same batches are
    // refused once the clock has moved every cluster on from `STARTING`.
    tokio::time::sleep(PAST_TRANSITION).await;
    let (status, payload) = emr
        .call(
            "TerminateJobFlows",
            json!({ "JobFlowIds": [&unlocked, &locked_by_api] }),
        )
        .await;
    assert_protected_refusal("a waiting locked cluster", status, &payload);
    for id in [&locked_at_launch, &locked_by_api, &unlocked, &also_unlocked] {
        assert_not_shutting_down(&emr, "a waiting locked cluster", id).await;
    }

    // The control: the same operation on the unlocked clusters alone is served, so every
    // refusal above was the lock and not a blanket refusal of the operation.
    emr.ok(
        "TerminateJobFlows",
        json!({ "JobFlowIds": [&unlocked, &also_unlocked] }),
    )
    .await;
    for id in [&unlocked, &also_unlocked] {
        let cluster = emr.describe_cluster(id).await;
        assert_eq!(
            at(&cluster, &["Status", "State"]).as_str(),
            Some("TERMINATING"),
            "case \"the unlocked clusters\": {id} must start shutting down, got {cluster}"
        );
    }
}

/// THE RULE's other half: the lock only has a shutdown to refuse while the cluster still
/// has instances to lose, so locking a cluster that is already shutting down or gone does
/// NOT turn a repeated `TerminateJobFlows` into a refusal — it stays the idempotent 200
/// no-op, and the cluster's own timeline is unchanged by the repeat.
#[tokio::test]
async fn a_locked_cluster_that_is_already_shutting_down_stays_an_idempotent_success() {
    let emr = Emr::new();

    // Each row locks the cluster AFTER the shutdown started, at a different point of the
    // span the clock owes it: while `TERMINATING`, and once `TERMINATED`.
    for (label, wait_first) in [
        ("locked while TERMINATING", false),
        ("locked once TERMINATED", true),
    ] {
        let cluster_id = emr.run_job_flow(kept_alive_request(label)).await;
        emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [&cluster_id] }))
            .await;
        if wait_first {
            tokio::time::sleep(PAST_TRANSITION).await;
        }
        emr.ok(
            "SetTerminationProtection",
            json!({ "JobFlowIds": [&cluster_id], "TerminationProtected": true }),
        )
        .await;

        let before = emr.describe_cluster(&cluster_id).await;
        let expected_state = if wait_first { "TERMINATED" } else { "TERMINATING" };
        assert_eq!(
            at(&before, &["Status", "State"]).as_str(),
            Some(expected_state),
            "case {label:?}: the cluster must be {expected_state} before the repeat, got {before}"
        );

        // A 200 rather than the locked refusal: `ok` fails the case on any other status.
        emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [&cluster_id] }))
            .await;
        let after = emr.describe_cluster(&cluster_id).await;
        let state = at(&after, &["Status", "State"])
            .as_str()
            .unwrap_or_default()
            .to_string();
        // Still on the shutdown the FIRST call started — the repeat added nothing, and the
        // only move allowed between the two reads is the one the clock already owed.
        assert!(
            state == expected_state || state == "TERMINATED",
            "case {label:?}: the repeat must leave the shutdown as it was, got state \
             {state:?} in {after}"
        );
        assert_eq!(
            at(&after, &["Status", "StateChangeReason"]),
            at(&before, &["Status", "StateChangeReason"]),
            "case {label:?}: the repeat must not restate why the cluster is going away, \
             got {after}"
        );
        assert_eq!(
            after["TerminationProtected"].as_bool(),
            Some(true),
            "case {label:?}: the lock the request wrote must still be reported, got {after}"
        );
        if wait_first {
            assert_eq!(
                after, before,
                "case {label:?}: a repeated shutdown of a gone cluster must change nothing"
            );
        }
    }
}

/// THE RULE's repair path: "to terminate a cluster that has been locked ... you must first
/// unlock the job flow by a subsequent call to SetTerminationProtection in which you set
/// the value to false" (`SetTerminationProtection` doc) — the identical call that was
/// refused is then served, and the cluster reaches `TERMINATED` with the `USER_REQUEST`
/// reason.
#[tokio::test]
async fn clearing_the_lock_lets_the_identical_terminate_through() {
    let emr = Emr::new();

    // The rows differ in the batch the repair is driven over: one cluster, and a batch
    // whose every cluster had to be unlocked.
    for (label, count) in [("one cluster", 1usize), ("three clusters at once", 3)] {
        let ids: Vec<String> = {
            let mut ids = Vec::new();
            for index in 0..count {
                ids.push(
                    emr.run_job_flow(launched_with_protection(
                        &format!("{label}-{index}"),
                        true,
                    ))
                    .await,
                );
            }
            ids
        };

        let (status, payload) = emr
            .call("TerminateJobFlows", json!({ "JobFlowIds": &ids }))
            .await;
        assert_protected_refusal(label, status, &payload);

        emr.ok(
            "SetTerminationProtection",
            json!({ "JobFlowIds": &ids, "TerminationProtected": false }),
        )
        .await;
        emr.ok("TerminateJobFlows", json!({ "JobFlowIds": &ids }))
            .await;
        for id in &ids {
            let shutting_down = emr.describe_cluster(id).await;
            assert_eq!(
                at(&shutting_down, &["Status", "State"]).as_str(),
                Some("TERMINATING"),
                "case {label:?}: {id} must start shutting down once unlocked, got \
                 {shutting_down}"
            );
            assert_eq!(
                at(&shutting_down, &["Status", "StateChangeReason", "Code"]).as_str(),
                Some("USER_REQUEST"),
                "case {label:?}: the reason must be USER_REQUEST, got {shutting_down}"
            );
            assert_eq!(
                at(&shutting_down, &["Status", "StateChangeReason", "Message"]).as_str(),
                Some("Terminated by user request"),
                "case {label:?}: the reason's message must be the user-request one, got \
                 {shutting_down}"
            );
        }

        tokio::time::sleep(PAST_TRANSITION).await;
        for id in &ids {
            let gone = emr.describe_cluster(id).await;
            assert_eq!(
                at(&gone, &["Status", "State"]).as_str(),
                Some("TERMINATED"),
                "case {label:?}: {id} must reach TERMINATED, got {gone}"
            );
            assert!(
                !at(&gone, &["Status", "Timeline", "EndDateTime"]).is_null(),
                "case {label:?}: {id} carries an EndDateTime, got {gone}"
            );
        }
    }
}

/// THE RULE on the record: shutting a cluster down DESTROYS what this service holds for
/// it, so every branch of `TerminateJobFlows` that returns leaves the structured record
/// naming the operation, the cluster the outcome is about, the presented access key id,
/// the request target and the outcome — one per cluster it shut down, one per refusal —
/// while a cluster the call did not move, and a reporting call, leave none.
#[tokio::test]
async fn shutting_a_cluster_down_records_each_cluster_and_each_refusal() {
    let capture = audit_capture();
    let emr = Emr::with_akid(AUDIT_AKID);

    // Harness self-test, so a failure below can never be mistaken for a missing record.
    tracing::info!(target: "emulator::audit", access_key_id = AUDIT_AKID, "probe");
    let probe = capture.drain_audit_records(AUDIT_AKID);
    assert_eq!(
        probe.len(),
        1,
        "the capture harness must see an audit record emitted on this thread, got {probe:?}"
    );

    let locked = emr
        .run_job_flow(launched_with_protection("audited-locked", true))
        .await;
    let first = emr.run_job_flow(kept_alive_request("audited-one")).await;
    let second = emr.run_job_flow(kept_alive_request("audited-two")).await;
    // The launches leave records of their own, which belong to `RunJobFlow`.
    let _ = capture.drain_audit_records(AUDIT_AKID);

    let past_the_cap: Vec<&String> = (0..=MAX_JOB_FLOW_IDS_PER_REQUEST).map(|_| &first).collect();
    let unknown = UNKNOWN_ID.to_string();

    // (label, the batch, the records it must leave, oldest first).
    let audited: Vec<AuditedBatch<'_>> = vec![
        (
            "a batch naming a locked cluster",
            vec![&first, &locked],
            vec![(locked.as_str(), "validation-refused")],
        ),
        (
            "a batch naming an id no cluster carries",
            vec![&unknown],
            vec![(UNKNOWN_ID, "validation-refused")],
        ),
        (
            // The SIZE of the batch is refused before any id in it is looked up, so the
            // record names no cluster.
            "a batch past the maximum",
            past_the_cap,
            vec![("", "validation-refused")],
        ),
        (
            "two clusters shut down at once",
            vec![&first, &second],
            vec![
                (first.as_str(), "cluster-termination-started"),
                (second.as_str(), "cluster-termination-started"),
            ],
        ),
        (
            // Both are already shutting down, so this call moved nothing and has nothing
            // to attribute.
            "the same batch again",
            vec![&first, &second],
            vec![],
        ),
    ];

    for (label, ids, expected) in audited {
        emr.call("TerminateJobFlows", json!({ "JobFlowIds": &ids }))
            .await;
        let records = capture.drain_audit_records(AUDIT_AKID);
        assert_eq!(
            records.len(),
            expected.len(),
            "case {label:?}: expected {} audit record(s), captured {records:?}",
            expected.len()
        );
        for (record, (resource, outcome)) in records.iter().zip(expected) {
            for (field, value) in [
                ("operation", "TerminateJobFlows"),
                ("resource", resource),
                ("outcome", outcome),
                ("access_key_id", AUDIT_AKID),
                ("request_target", REQUEST_TARGET),
            ] {
                assert_eq!(
                    record.field(field),
                    Some(value),
                    "case {label:?}: expected {field}={value:?}, got {record:?}"
                );
            }
        }
    }

    // Reporting calls record nothing: there is no state change to attribute.
    let _ = emr.describe_cluster(&locked).await;
    emr.call("DescribeCluster", json!({ "ClusterId": UNKNOWN_ID }))
        .await;
    let leaked = capture.drain_audit_records(AUDIT_AKID);
    assert!(
        leaked.is_empty(),
        "case \"reporting calls\": a reporting operation must emit no audit record, \
         captured {leaked:?}"
    );
}

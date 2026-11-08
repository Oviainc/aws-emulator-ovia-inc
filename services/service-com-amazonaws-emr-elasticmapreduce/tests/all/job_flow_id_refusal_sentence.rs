// SPDX-License-Identifier: CC0-1.0

//! The sentence Amazon EMR's legacy job-flow surface answers an id it resolves no cluster
//! for, asserted over the whole BATCH the `JobFlowIds` operations take.
//!
//! THE RULE this suite asserts: an operation of that surface refuses an id it resolves no
//! cluster for with HTTP 400 and the surface's sentence, which is CONSTANT — fixed text
//! with no variable part, so no value from the request appears in it and nothing is
//! appended to it — whatever the form of the unresolvable id, however many ids the
//! request batches beside it, wherever in the batch it sits and whatever else the request
//! asks for. The status and the error code are asserted beside the text, because a client
//! reads all three.
//!
//! The sibling suite `tests/all/refusal_wording.rs` spans the OPERATIONS of this surface,
//! one single-id request each; this one spans the BATCH those operations take and the
//! request's other members, which is the other half of the class the rule is stated over
//! — a batch is where a message that interpolates an id has a value to interpolate. The
//! sentence itself is that suite's constant, reused here rather than copied.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so these are the
//! same bytes an SDK client sends.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::emr_control_plane::{MAX_JOB_FLOW_IDS_PER_REQUEST, UNKNOWN_CLUSTER_IDS};
use crate::job_flow_lifecycle::{
    assert_refused, at, kept_alive_request, Emr, PAST_CLUSTER_READY,
};
use crate::refusal_messages::message;
use crate::refusal_wording::JOB_FLOW_SENTENCE;

/// A second id no cluster is held under, so one batch can carry two DIFFERENT
/// unresolvable ids and the sentence still cannot name "the" offending one.
const ANOTHER_UNKNOWN_ID: &str = "j-OTHERUNKNOWN1";

/// `JobFlowIds` batches that cannot all be resolved, built around one id the service
/// holds a cluster under (`known`) and one it does not (`unknown`).
///
/// Rows differ along what the rule generalises over: the size of the batch, the position
/// of the unresolvable id in it, how many unresolvable ids it carries, and whether the
/// same one repeats — up to a batch at the cap the service takes for this member.
fn unresolvable_batches(known: &str, unknown: &str) -> Vec<(&'static str, Vec<String>)> {
    let mut at_the_cap: Vec<String> = (1..MAX_JOB_FLOW_IDS_PER_REQUEST)
        .map(|_| known.to_string())
        .collect();
    at_the_cap.push(unknown.to_string());

    vec![
        ("the unresolvable id alone", vec![unknown.to_string()]),
        (
            "the unresolvable id after a known one",
            vec![known.to_string(), unknown.to_string()],
        ),
        (
            "the unresolvable id before a known one",
            vec![unknown.to_string(), known.to_string()],
        ),
        (
            "the unresolvable id between two known ones",
            vec![known.to_string(), unknown.to_string(), known.to_string()],
        ),
        (
            "the same unresolvable id twice",
            vec![unknown.to_string(), unknown.to_string()],
        ),
        (
            "two different unresolvable ids",
            vec![unknown.to_string(), ANOTHER_UNKNOWN_ID.to_string()],
        ),
        ("a batch at the cap, one id unresolvable", at_the_cap),
    ]
}

/// That a refusal is this surface's: HTTP 400, the `ValidationException` channel (no
/// operation of this surface models a client error) and the sentence EQUAL to the
/// constant — which is what proves no value from the request was interpolated into it and
/// nothing was appended to it — answering the message for the caller to collect.
fn assert_job_flow_sentence(case: &str, status: u16, payload: &Value) -> String {
    assert_refused(case, status, payload, "ValidationException");
    let said = message(case, payload);
    assert_eq!(
        said, JOB_FLOW_SENTENCE,
        "case {case:?}: expected {JOB_FLOW_SENTENCE:?}, got {payload}"
    );
    said
}

/// THE RULE on `SetTerminationProtection`: every batch carrying an id the service
/// resolves no cluster for is refused with the surface's constant sentence, whichever
/// value the request asked to write and however the batch is shaped — and the clusters it
/// did resolve keep the value they held.
#[tokio::test]
async fn set_termination_protection_words_one_constant_sentence_for_every_batch() {
    let emr = Emr::new();
    let known = emr.run_job_flow(kept_alive_request("known")).await;

    let mut said = BTreeSet::new();
    for unknown in UNKNOWN_CLUSTER_IDS {
        for (label, ids) in unresolvable_batches(&known, unknown) {
            // Both values of the member the call writes: the sentence is about the id it
            // could not resolve, so what the request asked to write is no part of it.
            for protected in [true, false] {
                let case =
                    format!("SetTerminationProtection({protected}) with {label} ({unknown:?})");
                let (status, payload) = emr
                    .call(
                        "SetTerminationProtection",
                        json!({ "JobFlowIds": &ids, "TerminationProtected": protected }),
                    )
                    .await;
                said.insert(assert_job_flow_sentence(&case, status, &payload));

                let cluster = emr.describe_cluster(&known).await;
                assert_eq!(
                    cluster["TerminationProtected"].as_bool(),
                    Some(false),
                    "case {case:?}: a refused batch must write nothing, got {cluster}"
                );
            }
        }
    }
    assert_eq!(
        said.len(),
        1,
        "the sentence is constant across every batch, got {said:?}"
    );

    // The control: the same operation on the id it DOES resolve is served, so every
    // refusal above was a lookup and not a blanket refusal of the operation.
    emr.ok(
        "SetTerminationProtection",
        json!({ "JobFlowIds": [&known], "TerminationProtected": true }),
    )
    .await;
    let cluster = emr.describe_cluster(&known).await;
    assert_eq!(
        cluster["TerminationProtected"].as_bool(),
        Some(true),
        "case \"the resolvable id\": the write must be reported back, got {cluster}"
    );
}

/// THE RULE on the surface's other batched operation: `TerminateJobFlows` takes the same
/// `JobFlowIds` member and answers the SAME constant sentence for every shape of batch
/// carrying an unresolvable id — and terminates none of the clusters it did resolve.
#[tokio::test]
async fn terminate_job_flows_words_the_same_constant_sentence_for_every_batch() {
    let emr = Emr::new();
    let known = emr.run_job_flow(kept_alive_request("known")).await;

    let mut said = BTreeSet::new();
    for unknown in UNKNOWN_CLUSTER_IDS {
        for (label, ids) in unresolvable_batches(&known, unknown) {
            let case = format!("TerminateJobFlows with {label} ({unknown:?})");
            let (status, payload) = emr
                .call("TerminateJobFlows", json!({ "JobFlowIds": &ids }))
                .await;
            said.insert(assert_job_flow_sentence(&case, status, &payload));

            let cluster = emr.describe_cluster(&known).await;
            assert!(
                at(&cluster, &["Status", "StateChangeReason"]).is_null()
                    && at(&cluster, &["Status", "Timeline", "EndDateTime"]).is_null(),
                "case {case:?}: a refused batch must terminate nothing, got {cluster}"
            );
        }
    }
    assert_eq!(
        said.len(),
        1,
        "the sentence is constant across every batch, got {said:?}"
    );

    // The cluster the refused batches named is still the one the clock owes `WAITING`: it
    // is kept alive with no steps, so nothing else can have moved it.
    tokio::time::sleep(PAST_CLUSTER_READY).await;
    let cluster = emr.describe_cluster(&known).await;
    assert_eq!(
        at(&cluster, &["Status", "State"]).as_str(),
        Some("WAITING"),
        "the resolvable cluster must still be waiting, got {cluster}"
    );

    // The control: the same operation on the id it DOES resolve shuts that cluster down,
    // so every refusal above was a lookup.
    emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [&known] }))
        .await;
    let cluster = emr.describe_cluster(&known).await;
    assert_eq!(
        at(&cluster, &["Status", "State"]).as_str(),
        Some("TERMINATING"),
        "case \"the resolvable id\": the resolvable cluster must start shutting down, got \
         {cluster}"
    );
}

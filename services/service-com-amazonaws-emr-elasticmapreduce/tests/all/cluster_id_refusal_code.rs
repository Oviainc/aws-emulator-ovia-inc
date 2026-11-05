// SPDX-License-Identifier: CC0-1.0

//! The discriminator an Amazon EMR refusal of an unresolvable cluster id carries, end to
//! end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so
//! these are the same bytes an SDK client sends.
//!
//! THE RULE: Amazon EMR funnels every bad request through `InvalidRequestException`,
//! whose `ErrorCode` member ("the error code associated with the exception",
//! `smithy_query(query="shape", shape="InvalidRequestException")`) is the only
//! non-prose discriminator a client can branch on. A request naming a cluster the
//! service resolves none of is refused with the code of the SURFACE the id was
//! addressed through, beside that surface's sentence: the cluster surface answers
//! `MalformedClusterId`, the legacy job-flow surface — the one that calls a cluster
//! a job flow and words its refusal that way — answers `INVALID_JOB_ID_MALFORMED`, and
//! the session-credentials read — the one that states the cluster-id sentence without the
//! id — answers `InvalidClusterId`.
//! A refusal of any OTHER class on the same operations carries no code at all, which is
//! what gives either discriminator its meaning.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{assert_refused, kept_alive_request, Emr};
use crate::refusal_messages::message;
use crate::refusal_wording::UNKNOWN_CLUSTER_IDS;
use crate::resource_tags::error_code;

/// The discriminator the CLUSTER surface answers an id it resolves no cluster for with.
///
/// `pub(crate)` so a sibling suite asserts it through one constant rather than its own
/// copy.
pub(crate) const MALFORMED_CLUSTER_ID: &str = "MalformedClusterId";

/// The discriminator the LEGACY job-flow surface answers the same class with.
const INVALID_JOB_ID_MALFORMED: &str = "INVALID_JOB_ID_MALFORMED";

/// The discriminator the SESSION-CREDENTIALS read answers the same class with: its own,
/// not the cluster reads'.
///
/// `pub(crate)` so a sibling suite asserts it through one constant rather than its own
/// copy.
pub(crate) const INVALID_CLUSTER_ID: &str = "InvalidClusterId";

/// Assert one refusal: HTTP 400, the modelled client error, and `expected` in
/// `ErrorCode`.
fn assert_code(case: &str, status: u16, payload: &Value, expected: &str) {
    assert_refused(case, status, payload, "InvalidRequestException");
    assert_eq!(
        error_code(payload).as_deref(),
        Some(expected),
        "case {case:?}: expected ErrorCode {expected}, got {:?} in {payload}",
        error_code(payload)
    );
}

/// One request per operation that resolves a cluster id through the modelled client
/// error, naming `cluster_id`, paired with the discriminator its surface answers.
///
/// Rows span the whole family, so the rule is asserted where it holds rather than only
/// where it was observed: the cluster reads, the step reads and the step cancellation,
/// the cluster modification, the session read, the session-credentials read that states
/// the refusal without the id and under its own code, both tag operations — reached by
/// `ResourceId` alone and with the `ClusterId` that scopes them — and the instance-fleet
/// add, which reaches the cluster as a JOB FLOW and so answers the other code.
///
/// The notebook start is NOT a row: it reaches a cluster through its
/// `ExecutionEngine$Id`, and refuses an engine that names no usable cluster through the
/// request-validation envelope, which carries no `ErrorCode` member at all.
fn cluster_id_cases(cluster_id: &str) -> Vec<(&'static str, Value, &'static str)> {
    let mut cases: Vec<(&'static str, Value, &'static str)> = vec![
        (
            "DescribeCluster",
            json!({ "ClusterId": cluster_id }),
            MALFORMED_CLUSTER_ID,
        ),
        (
            "ListInstanceGroups",
            json!({ "ClusterId": cluster_id }),
            MALFORMED_CLUSTER_ID,
        ),
        (
            "ListInstanceFleets",
            json!({ "ClusterId": cluster_id }),
            MALFORMED_CLUSTER_ID,
        ),
        (
            "ListSteps",
            json!({ "ClusterId": cluster_id }),
            MALFORMED_CLUSTER_ID,
        ),
        (
            "DescribeStep",
            json!({ "ClusterId": cluster_id, "StepId": "s-ZZZZZZZZZZZZZ" }),
            MALFORMED_CLUSTER_ID,
        ),
        (
            "CancelSteps",
            json!({ "ClusterId": cluster_id, "StepIds": ["s-ZZZZZZZZZZZZZ"] }),
            MALFORMED_CLUSTER_ID,
        ),
        (
            "ModifyCluster",
            json!({ "ClusterId": cluster_id, "StepConcurrencyLevel": 2 }),
            MALFORMED_CLUSTER_ID,
        ),
        (
            "GetSession",
            json!({ "ClusterId": cluster_id, "SessionId": "is-0000000000000" }),
            MALFORMED_CLUSTER_ID,
        ),
        (
            "GetClusterSessionCredentials",
            json!({
                "ClusterId": cluster_id,
                "ExecutionRoleArn": "arn:aws:iam::000000000000:role/EMRRuntime",
            }),
            INVALID_CLUSTER_ID,
        ),
        (
            "AddTags",
            json!({
                "ResourceId": cluster_id,
                "Tags": [{ "Key": "stack", "Value": "Prod" }],
            }),
            MALFORMED_CLUSTER_ID,
        ),
        (
            "RemoveTags",
            json!({ "ResourceId": cluster_id, "TagKeys": ["stack"] }),
            MALFORMED_CLUSTER_ID,
        ),
        (
            "AddInstanceFleet",
            json!({
                "ClusterId": cluster_id,
                "InstanceFleet": {
                    "InstanceFleetType": "TASK",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                },
            }),
            INVALID_JOB_ID_MALFORMED,
        ),
    ];

    // The tag operations reached by the `ClusterId` that SCOPES them, which resolves the
    // `ResourceId` against that cluster alone. `ClusterId` is `len 0..256` and an empty
    // one scopes nothing — the `ResourceId` then says which resource the call is about —
    // so these two rows take every id but that one.
    if !cluster_id.is_empty() {
        cases.push((
            "AddTags",
            json!({
                "ClusterId": cluster_id,
                "ResourceId": "s-session",
                "Tags": [{ "Key": "stack", "Value": "Prod" }],
            }),
            MALFORMED_CLUSTER_ID,
        ));
        cases.push((
            "RemoveTags",
            json!({
                "ClusterId": cluster_id,
                "ResourceId": "s-session",
                "TagKeys": ["stack"],
            }),
            MALFORMED_CLUSTER_ID,
        ));
    }
    cases
}

/// THE RULE: every operation that resolves a cluster id answers its surface's
/// discriminator in `ErrorCode` beside its own message — for every form of id no cluster
/// carries, and while another cluster exists, so each refusal is a lookup rather than a
/// blanket refusal of the surface.
#[tokio::test]
async fn every_unresolvable_cluster_id_is_refused_with_its_surfaces_code() {
    let emr = Emr::new();
    let live = emr.run_job_flow(kept_alive_request("live")).await;

    for unknown in UNKNOWN_CLUSTER_IDS {
        for (operation, body, expected) in cluster_id_cases(unknown) {
            let scoped = if body.get("ClusterId").is_some() && body.get("ResourceId").is_some() {
                " scoping a tag operation"
            } else {
                ""
            };
            let case = format!("{operation} with cluster id {unknown:?}{scoped}");
            let (status, payload) = emr.call(operation, body).await;
            assert_code(&case, status, &payload, expected);
            // The surface's own sentence stands beside the code, so the discriminator
            // and the prose cannot come apart.
            let said = message(&case, &payload);
            assert!(
                !said.is_empty(),
                "case {case:?}: the refusal carries a message beside the code, got {payload}"
            );
        }
    }

    // The live cluster is untouched and still resolves, so every refusal above was a
    // lookup of persisted state.
    let cluster = emr.describe_cluster(&live).await;
    assert_eq!(
        cluster["Id"].as_str(),
        Some(live.as_str()),
        "the live cluster must still read back, got {cluster}"
    );
}

/// THE OTHER HALF of the rule: on the SAME operations, a refusal that is not about an
/// unresolvable cluster id, and whose class Amazon EMR publishes no code for, carries no
/// discriminator at all. Rows span those classes — a step id the cluster carries none of,
/// a member outside its documented range, a session the cluster has none of, a tag the
/// service will not hold, and a fleet add to a cluster built from instance groups — so
/// `MalformedClusterId` and `INVALID_JOB_ID_MALFORMED` each mean exactly one thing.
#[tokio::test]
async fn a_refusal_of_another_class_on_the_same_operations_carries_no_code() {
    let emr = Emr::new();
    let live = emr.run_job_flow(kept_alive_request("live")).await;

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a step id the cluster carries none of",
            "DescribeStep",
            json!({ "ClusterId": &live, "StepId": "s-ZZZZZZZZZZZZZ" }),
        ),
        (
            "a step concurrency level outside its documented range",
            "ModifyCluster",
            json!({ "ClusterId": &live, "StepConcurrencyLevel": 0 }),
        ),
        (
            "a session the cluster has none of",
            "GetSession",
            json!({ "ClusterId": &live, "SessionId": "is-ZZZZZZZZZZZZZ" }),
        ),
        (
            "a tag key outside the class a tag is drawn from",
            "AddTags",
            json!({
                "ResourceId": &live,
                "Tags": [{ "Key": "stack\u{1f600}", "Value": "Prod" }],
            }),
        ),
        (
            "a fleet add to a cluster built from instance groups",
            "AddInstanceFleet",
            json!({
                "ClusterId": &live,
                "InstanceFleet": {
                    "InstanceFleetType": "TASK",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                },
            }),
        ),
    ];

    for (case, operation, body) in cases {
        let (status, payload) = emr.call(operation, body).await;
        assert_refused(case, status, &payload, "InvalidRequestException");
        assert_eq!(
            error_code(&payload),
            None,
            "case {case:?}: {operation} must carry no ErrorCode, got {:?} in {payload}",
            error_code(&payload)
        );
    }
}

/// THE RULE under the resource's own lifecycle: a cluster that EXISTS is served, and the
/// same id answers the cluster surface's discriminator once the cluster is gone — so the
/// code is computed from persisted state rather than from the shape of the id.
#[tokio::test]
async fn an_id_carries_the_code_only_once_its_cluster_is_gone() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("doomed")).await;

    // While it exists, the reads are served: no refusal and no discriminator.
    for operation in ["DescribeCluster", "ListSteps", "ListInstanceGroups"] {
        let payload = emr.ok(operation, json!({ "ClusterId": &cluster })).await;
        assert_eq!(
            error_code(&payload),
            None,
            "case {operation:?}: a served read carries no ErrorCode, got {payload}"
        );
    }

    // A terminated cluster is still HELD and still reported, so terminating it is not
    // what makes the id unresolvable: the refusal is for an id the store has no record
    // under at all.
    emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [&cluster] }))
        .await;
    let described = emr.describe_cluster(&cluster).await;
    assert_eq!(
        described["Id"].as_str(),
        Some(cluster.as_str()),
        "a terminated cluster is still reported, got {described}"
    );

    // One character off that very id names no cluster, and is refused with the code.
    let mutated = format!("{cluster}X");
    let case = format!("DescribeCluster with the near-miss id {mutated:?}");
    let (status, payload) = emr
        .call("DescribeCluster", json!({ "ClusterId": &mutated }))
        .await;
    assert_code(&case, status, &payload, MALFORMED_CLUSTER_ID);
    assert_eq!(
        message(&case, &payload),
        format!("Cluster id '{mutated}' is not valid."),
        "case {case:?}: the surface's own sentence stands beside its code, got {payload}"
    );
}

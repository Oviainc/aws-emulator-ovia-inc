// SPDX-License-Identifier: CC0-1.0

//! Termination protection on a cluster with more than one primary node, end to end over
//! the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, and reads the lock back through `DescribeCluster`, so nothing here is
//! evidence of anything but the persisted state.
//!
//! THE RULE: a cluster with more than one primary node is a highly available cluster, and
//! Amazon EMR locks one against a shutdown it did not mean. `RunJobFlow` turns termination
//! protection ON for such a cluster whatever `Instances.TerminationProtected` asked for,
//! so `DescribeCluster` reports `TerminationProtected` true (`Cluster$TerminationProtected`)
//! and `TerminateJobFlows` on it is refused — until `SetTerminationProtection` with `false`
//! clears the lock, which is the one way to clear it. A cluster with a single primary node
//! keeps exactly the protection the launch asked for.
//!
//! The rows span what the rule generalises over: how the launch STATES its primary nodes
//! (the uniform `MasterInstanceType`/`InstanceCount` form, an explicit `MASTER` instance
//! group, a `MASTER` instance fleet), how many primaries that comes to (one, two, three,
//! and three split across On-Demand and Spot capacity), and what the request asked for
//! (`false`, absent, `true`).

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{
    assert_refused, at, Emr, INSTANCE_PROFILE, SERVICE_ROLE,
};
use crate::refusal_messages::message;
use crate::terminate_job_flows_protection::PROTECTED_SENTENCE;

/// One row: a launch, the number of primary nodes it provisions, and whether the cluster
/// it creates is locked.
struct Case {
    label: &'static str,
    /// The `Instances` member of the launch, without its `TerminationProtected`.
    instances: Value,
    /// What the launch asks for, or `None` to leave the member out altogether.
    requested: Option<bool>,
    /// Whether `DescribeCluster` must report the cluster as protected.
    protected: bool,
}

fn uniform(count: i64) -> Value {
    json!({
        "InstanceCount": count,
        "MasterInstanceType": "m5.xlarge",
        "SlaveInstanceType": "m5.xlarge",
        "KeepJobFlowAliveWhenNoSteps": true,
    })
}

fn groups(primaries: i64) -> Value {
    json!({
        "KeepJobFlowAliveWhenNoSteps": true,
        "InstanceGroups": [
            {
                "Name": "primary",
                "InstanceRole": "MASTER",
                "InstanceType": "m5.xlarge",
                "InstanceCount": primaries,
            },
            {
                "Name": "core",
                "InstanceRole": "CORE",
                "InstanceType": "m5.xlarge",
                "InstanceCount": 2,
            },
        ],
    })
}

fn fleets(on_demand: i64, spot: i64) -> Value {
    json!({
        "KeepJobFlowAliveWhenNoSteps": true,
        "InstanceFleets": [{
            "Name": "primary",
            "InstanceFleetType": "MASTER",
            "TargetOnDemandCapacity": on_demand,
            "TargetSpotCapacity": spot,
            "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
        }],
    })
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "the uniform form: three instances, one of them the primary",
            instances: uniform(3),
            requested: Some(false),
            protected: false,
        },
        Case {
            label: "one primary in its own instance group",
            instances: groups(1),
            requested: Some(false),
            protected: false,
        },
        Case {
            label: "three primaries in an instance group, protection declined",
            instances: groups(3),
            requested: Some(false),
            protected: true,
        },
        Case {
            label: "two primaries in an instance group, protection not mentioned",
            instances: groups(2),
            requested: None,
            protected: true,
        },
        Case {
            label: "three primaries in an On-Demand instance fleet",
            instances: fleets(3, 0),
            requested: Some(false),
            protected: true,
        },
        Case {
            label: "three primaries split across On-Demand and Spot capacity",
            instances: fleets(1, 2),
            requested: Some(false),
            protected: true,
        },
        Case {
            label: "one primary in an instance fleet, protection asked for",
            instances: fleets(1, 0),
            requested: Some(true),
            protected: true,
        },
        Case {
            label: "one primary in an instance fleet, protection declined",
            instances: fleets(1, 0),
            requested: Some(false),
            protected: false,
        },
    ]
}

/// The launch of one row.
fn launch(case: &Case) -> Value {
    let mut instances = case.instances.clone();
    if let Some(requested) = case.requested {
        instances["TerminationProtected"] = json!(requested);
    }
    json!({
        "Name": case.label,
        "JobFlowRole": INSTANCE_PROFILE,
        "ServiceRole": SERVICE_ROLE,
        "Instances": instances,
    })
}

/// Whether `DescribeCluster` reports the cluster as termination protected.
async fn protection_of(emr: &Emr, cluster_id: &str) -> Option<bool> {
    at(&emr.describe_cluster(cluster_id).await, &["TerminationProtected"]).as_bool()
}

#[tokio::test]
async fn a_multi_primary_cluster_is_protected_and_refuses_a_bare_terminate() {
    let emr = Emr::new();

    for case in cases() {
        let label = case.label;
        let cluster_id = emr.run_job_flow(launch(&case)).await;

        assert_eq!(
            protection_of(&emr, &cluster_id).await,
            Some(case.protected),
            "case {label:?}: expected TerminationProtected {}, got the cluster {:?}",
            case.protected,
            emr.describe_cluster(&cluster_id).await
        );

        let (status, payload) = emr
            .call("TerminateJobFlows", json!({ "JobFlowIds": [&cluster_id] }))
            .await;
        if case.protected {
            // The lock refuses the shutdown, in Amazon EMR's own fixed sentence, and the
            // cluster is still there.
            assert_refused(label, status, &payload, "ValidationException");
            assert_eq!(
                message(label, &payload),
                PROTECTED_SENTENCE,
                "case {label:?}: expected the protected sentence, got {payload}"
            );
            let state = at(&emr.describe_cluster(&cluster_id).await, &["Status", "State"])
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                state != "TERMINATING" && state != "TERMINATED",
                "case {label:?}: a refused shutdown leaves the cluster running, got {state:?}"
            );

            // Clearing the lock is what lets the shutdown through, and nothing else.
            emr.ok(
                "SetTerminationProtection",
                json!({ "JobFlowIds": [&cluster_id], "TerminationProtected": false }),
            )
            .await;
            assert_eq!(
                protection_of(&emr, &cluster_id).await,
                Some(false),
                "case {label:?}: SetTerminationProtection(false) clears the lock"
            );
            emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [&cluster_id] }))
                .await;
        } else {
            assert_eq!(
                status, 200,
                "case {label:?}: an unprotected cluster shuts down, got {status} with {payload}"
            );
        }
        assert_eq!(
            at(&emr.describe_cluster(&cluster_id).await, &["Status", "State"]).as_str(),
            Some("TERMINATING"),
            "case {label:?}: the cluster shuts down once nothing locks it"
        );
    }
}

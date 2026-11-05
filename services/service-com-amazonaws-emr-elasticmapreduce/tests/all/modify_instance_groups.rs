// SPDX-License-Identifier: CC0-1.0

//! `ModifyInstanceGroups` over the wire, end to end.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so these are the
//! same bytes an SDK client sends. Nothing reaches into the service's state struct: a
//! modification is only ever read back through `ListInstanceGroups`, which is what makes
//! these tests evidence that the groups are computed from persisted state.
//!
//! THE RULE: `ModifyInstanceGroups` applies each submitted modification to the persisted
//! record of the group it names — the new target instance count, the configuration
//! specification (merged into the one the group holds or replacing it, at a new version)
//! and the shrink policy — and `ListInstanceGroups` reports exactly that, with the
//! instances the group already runs reported until the clock makes the change due. The
//! call succeeds or fails atomically: a cluster the service holds no record for, one that
//! is shutting down or gone, a group that cluster does not carry, a negative target, a
//! resize of the group that runs the primary node, a specification past what a group may
//! hold, a list naming more modifications than a cluster has groups, and an Amazon EC2
//! instance id that names no instance are refused with HTTP 400 and the store untouched.
//! A managed scaling policy's compute limits bound what the CLUSTER scales itself to, not
//! what a caller asks a group for, so a target outside them is applied like any other.

use std::time::Duration;

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{assert_refused, at, Emr};

/// Longer than the service's clock-owed transition span (500 ms), so a group that was
/// `PROVISIONING`, `RESIZING` or `RECONFIGURING` has reached its next state by the time
/// we look.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// The bounds this suite takes against the group store (`crate::state`'s
/// `MAX_INSTANCE_GROUPS_PER_CLUSTER` and `MAX_INSTANCE_GROUP_CONFIGURATIONS`).
const MAX_INSTANCE_GROUPS_PER_CLUSTER: usize = 50;
const MAX_INSTANCE_GROUP_CONFIGURATIONS: usize = 100;

/// The groups every cluster in this suite launches with: the primary node, a core group
/// of two and a task group of three, so a case can scale up, scale down and leave a
/// neighbour alone. The core group carries a configuration, which is what a `MERGE`
/// reconfiguration merges into.
fn launch_request(name: &str) -> Value {
    json!({
        "Name": name,
        "ReleaseLabel": "emr-6.15.0",
        "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
        "Instances": {
            "KeepJobFlowAliveWhenNoSteps": true,
            "InstanceGroups": [
                { "Name": "primary", "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 1 },
                {
                    "Name": "core",
                    "InstanceRole": "CORE",
                    "InstanceType": "r5.xlarge",
                    "InstanceCount": 2,
                    "Configurations": [{
                        "Classification": "spark-defaults",
                        "Properties": { "spark.executor.memory": "2g" },
                    }],
                },
                { "Name": "workers", "InstanceRole": "TASK", "InstanceType": "c5.xlarge", "InstanceCount": 3 },
            ],
        },
    })
}

/// A cluster launched from [`launch_request`], with its groups already up: the cluster
/// id and the ids of its MASTER, CORE and TASK groups, in creation order.
async fn launched(emr: &Emr, name: &str) -> (String, Vec<String>) {
    let cluster_id = emr.run_job_flow(launch_request(name)).await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let ids = groups_of(emr, &cluster_id)
        .await
        .iter()
        .map(|group| group["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    (cluster_id, ids)
}

/// Every group `cluster_id` carries, as `ListInstanceGroups` reports them.
async fn groups_of(emr: &Emr, cluster_id: &str) -> Vec<Value> {
    let out = emr
        .ok("ListInstanceGroups", json!({ "ClusterId": cluster_id }))
        .await;
    out["InstanceGroups"].as_array().cloned().unwrap_or_default()
}

/// The reported group `id`, out of a listing.
fn reported<'a>(case: &str, groups: &'a [Value], id: &str) -> &'a Value {
    groups
        .iter()
        .find(|group| group["Id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("case {case:?}: no group {id:?} in {groups:?}"))
}

/// The classifications of a reported `Configurations`-shaped member, in order.
fn classifications(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["Classification"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

/// One `Configurations` entry, with one property so a replaced entry can be told from
/// the one it replaced.
fn configuration(classification: &str, value: &str) -> Value {
    json!({ "Classification": classification, "Properties": { "setting": value } })
}

// ---------------------------------------------------------------------------
// The resize
// ---------------------------------------------------------------------------

/// THE RULE: the target instance count a modification submits is what the group requests
/// from then on, and `ListInstanceGroups` reports it. The table spans the directions a
/// resize runs (up, down, to none, to the count it already holds, far up), the roles it
/// may scale (CORE and TASK), and both forms of the request — with the `ClusterId` the
/// group belongs to and without it, since the group id already names that cluster.
#[tokio::test]
async fn a_submitted_target_instance_count_is_what_the_group_requests() {
    struct Case {
        label: &'static str,
        /// Which launched group to resize: 1 is the core group, 2 the task group.
        group: usize,
        target: i64,
        /// Whether the request names the cluster the group belongs to.
        name_cluster: bool,
    }
    // The launched counts are MASTER 1, CORE 2, TASK 3.
    const LAUNCHED: [i64; 3] = [1, 2, 3];
    let cases = [
        Case {
            label: "scale the core group up",
            group: 1,
            target: 6,
            name_cluster: true,
        },
        Case {
            label: "scale the task group down",
            group: 2,
            target: 1,
            name_cluster: true,
        },
        Case {
            label: "scale the task group to no instances",
            group: 2,
            target: 0,
            name_cluster: true,
        },
        Case {
            label: "the count it already holds",
            group: 1,
            target: LAUNCHED[1],
            name_cluster: true,
        },
        Case {
            label: "far above what it launched with",
            group: 2,
            target: 500,
            name_cluster: true,
        },
        Case {
            label: "no ClusterId: the group id names its cluster",
            group: 1,
            target: 4,
            name_cluster: false,
        },
    ];

    let emr = Emr::new();
    for case in &cases {
        let (cluster_id, ids) = launched(&emr, case.label).await;
        let target_id = ids[case.group].clone();

        let mut request = json!({
            "InstanceGroups": [{ "InstanceGroupId": &target_id, "InstanceCount": case.target }],
        });
        if case.name_cluster {
            request["ClusterId"] = json!(&cluster_id);
        }
        emr.ok("ModifyInstanceGroups", request).await;

        tokio::time::sleep(PAST_TRANSITION).await;
        let groups = groups_of(&emr, &cluster_id).await;
        for (at_index, id) in ids.iter().enumerate() {
            let expected = if at_index == case.group {
                case.target
            } else {
                LAUNCHED[at_index]
            };
            let group = reported(case.label, &groups, id);
            assert_eq!(
                group["RequestedInstanceCount"].as_i64(),
                Some(expected),
                "case {:?}: group {id} must request {expected} instances, got {group}",
                case.label
            );
            // "The number of instances currently running in this instance group": the
            // resize has come due, so the group runs the target it now requests.
            assert_eq!(
                group["RunningInstanceCount"].as_i64(),
                Some(expected),
                "case {:?}: group {id} must run {expected} instances, got {group}",
                case.label
            );
            assert_eq!(
                at(group, &["Status", "State"]).as_str(),
                Some("RUNNING"),
                "case {:?}: group {id} must be RUNNING again, got {group}",
                case.label
            );
        }
    }
}

/// THE RULE: a resize is not instant — the group is `RESIZING` and still runs the
/// instances it already had until the clock makes the new target due, and only then is it
/// `RUNNING` at that target. Both directions, because a shrink keeps its instances
/// running exactly as a growth does.
#[tokio::test]
async fn a_resize_runs_the_old_count_until_the_clock_makes_the_new_one_due() {
    for (label, target, launched_count) in [("growth", 5_i64, 2_i64), ("shrink", 1, 2)] {
        let emr = Emr::new();
        let (cluster_id, ids) = launched(&emr, label).await;
        let core = ids[1].clone();

        emr.ok(
            "ModifyInstanceGroups",
            json!({
                "ClusterId": &cluster_id,
                "InstanceGroups": [{ "InstanceGroupId": &core, "InstanceCount": target }],
            }),
        )
        .await;

        let groups = groups_of(&emr, &cluster_id).await;
        let group = reported(label, &groups, &core);
        assert_eq!(
            at(group, &["Status", "State"]).as_str(),
            Some("RESIZING"),
            "case {label:?}: the group must be RESIZING straight after the call, got {group}"
        );
        assert_eq!(
            group["RequestedInstanceCount"].as_i64(),
            Some(target),
            "case {label:?}: the new target must be requested at once, got {group}"
        );
        assert_eq!(
            group["RunningInstanceCount"].as_i64(),
            Some(launched_count),
            "case {label:?}: the instances already up must still be running, got {group}"
        );

        tokio::time::sleep(PAST_TRANSITION).await;
        let groups = groups_of(&emr, &cluster_id).await;
        let group = reported(label, &groups, &core);
        assert_eq!(
            at(group, &["Status", "State"]).as_str(),
            Some("RUNNING"),
            "case {label:?}: the group must be RUNNING once the resize is due, got {group}"
        );
        assert_eq!(
            group["RunningInstanceCount"].as_i64(),
            Some(target),
            "case {label:?}: the target must be running once the resize is due, got {group}"
        );
    }
}

/// THE RULE: one call changes every group it names, and nothing it does not.
#[tokio::test]
async fn one_call_applies_every_modification_it_names() {
    let emr = Emr::new();
    let (cluster_id, ids) = launched(&emr, "two groups at once").await;

    emr.ok(
        "ModifyInstanceGroups",
        json!({
            "ClusterId": &cluster_id,
            "InstanceGroups": [
                { "InstanceGroupId": &ids[1], "InstanceCount": 7 },
                { "InstanceGroupId": &ids[2], "InstanceCount": 0 },
            ],
        }),
    )
    .await;

    tokio::time::sleep(PAST_TRANSITION).await;
    let groups = groups_of(&emr, &cluster_id).await;
    for (id, expected) in [(&ids[0], 1_i64), (&ids[1], 7), (&ids[2], 0)] {
        let group = reported("two groups at once", &groups, id);
        assert_eq!(
            group["RequestedInstanceCount"].as_i64(),
            Some(expected),
            "case \"two groups at once\": group {id} must request {expected} instances, got {group}"
        );
    }
}

// ---------------------------------------------------------------------------
// The reconfiguration
// ---------------------------------------------------------------------------

/// THE RULE: a submitted configuration specification becomes what the group requests, at
/// a version of its own, and becomes its last SUCCESSFULLY APPLIED specification once the
/// clock makes the reconfiguration applied. `MERGE` keeps every classification the group
/// holds that the request does not name again and replaces the ones it does; `OVERWRITE`,
/// which is also what a request naming no type asks for, replaces the specification
/// whole. The table spans both types, the absent type, a merge that replaces a held
/// classification, and a merge into a group holding none.
#[tokio::test]
async fn a_reconfiguration_versions_the_specification_it_leaves_the_group_requesting() {
    struct Case {
        label: &'static str,
        /// Which launched group: 1 holds `spark-defaults`, 2 holds nothing.
        group: usize,
        reconfiguration_type: Option<&'static str>,
        submitted: Vec<Value>,
        /// The classifications the group must request afterwards, in order.
        expected: Vec<&'static str>,
    }
    let cases = vec![
        Case {
            label: "OVERWRITE replaces the held specification",
            group: 1,
            reconfiguration_type: Some("OVERWRITE"),
            submitted: vec![configuration("yarn-site", "after")],
            expected: vec!["yarn-site"],
        },
        Case {
            label: "no type given replaces it too",
            group: 1,
            reconfiguration_type: None,
            submitted: vec![configuration("yarn-site", "after")],
            expected: vec!["yarn-site"],
        },
        Case {
            label: "MERGE keeps what it does not name",
            group: 1,
            reconfiguration_type: Some("MERGE"),
            submitted: vec![configuration("yarn-site", "after")],
            expected: vec!["spark-defaults", "yarn-site"],
        },
        Case {
            label: "MERGE replaces the classification it names again",
            group: 1,
            reconfiguration_type: Some("MERGE"),
            submitted: vec![configuration("spark-defaults", "after")],
            expected: vec!["spark-defaults"],
        },
        Case {
            label: "MERGE into a group holding none",
            group: 2,
            reconfiguration_type: Some("MERGE"),
            submitted: vec![
                configuration("hive-site", "after"),
                configuration("core-site", "after"),
            ],
            expected: vec!["hive-site", "core-site"],
        },
        Case {
            label: "OVERWRITE with an empty list clears the specification",
            group: 1,
            reconfiguration_type: Some("OVERWRITE"),
            submitted: Vec::new(),
            expected: Vec::new(),
        },
    ];

    let emr = Emr::new();
    for case in &cases {
        let (cluster_id, ids) = launched(&emr, case.label).await;
        let target_id = ids[case.group].clone();

        let mut modification = json!({
            "InstanceGroupId": &target_id,
            "Configurations": case.submitted.clone(),
        });
        if let Some(reconfiguration_type) = case.reconfiguration_type {
            modification["ReconfigurationType"] = json!(reconfiguration_type);
        }
        emr.ok(
            "ModifyInstanceGroups",
            json!({ "ClusterId": &cluster_id, "InstanceGroups": [modification] }),
        )
        .await;

        // Straight after the call the group is applying the specification: the version it
        // requests has moved on, and nothing has been applied yet.
        let groups = groups_of(&emr, &cluster_id).await;
        let group = reported(case.label, &groups, &target_id);
        assert_eq!(
            at(group, &["Status", "State"]).as_str(),
            Some("RECONFIGURING"),
            "case {:?}: the group must be RECONFIGURING straight after the call, got {group}",
            case.label
        );
        assert_eq!(
            group["ConfigurationsVersion"].as_i64(),
            Some(1),
            "case {:?}: the first reconfiguration is version 1, got {group}",
            case.label
        );
        assert!(
            group["LastSuccessfullyAppliedConfigurationsVersion"].is_null(),
            "case {:?}: nothing is applied while the reconfiguration is in flight, got {group}",
            case.label
        );

        tokio::time::sleep(PAST_TRANSITION).await;
        let groups = groups_of(&emr, &cluster_id).await;
        let group = reported(case.label, &groups, &target_id);
        assert_eq!(
            at(group, &["Status", "State"]).as_str(),
            Some("RUNNING"),
            "case {:?}: the group must be RUNNING once the reconfiguration is due, got {group}",
            case.label
        );
        assert_eq!(
            classifications(&group["Configurations"]),
            case.expected,
            "case {:?}: wrong requested specification, got {group}",
            case.label
        );
        assert_eq!(
            classifications(&group["LastSuccessfullyAppliedConfigurations"]),
            case.expected,
            "case {:?}: the applied specification must be the requested one, got {group}",
            case.label
        );
        assert_eq!(
            group["LastSuccessfullyAppliedConfigurationsVersion"].as_i64(),
            Some(1),
            "case {:?}: the applied version must be the requested one, got {group}",
            case.label
        );
        // A merge that names a classification again carries the submitted properties, so
        // the replacement is the request's entry and not the one it replaced.
        for entry in case.submitted.iter() {
            let classification = entry["Classification"].as_str().unwrap_or_default();
            let Some(reported_entry) = group["Configurations"]
                .as_array()
                .and_then(|entries| {
                    entries
                        .iter()
                        .find(|held| held["Classification"].as_str() == Some(classification))
                })
            else {
                panic!(
                    "case {:?}: {classification} must be reported, got {group}",
                    case.label
                );
            };
            assert_eq!(
                at(reported_entry, &["Properties", "setting"]).as_str(),
                Some("after"),
                "case {:?}: {classification} must carry the submitted property, got {group}",
                case.label
            );
        }
    }
}

/// THE RULE: each reconfiguration moves the requested version on by one, and the applied
/// version follows it as the clock makes each one applied.
#[tokio::test]
async fn every_reconfiguration_moves_the_version_on() {
    let emr = Emr::new();
    let (cluster_id, ids) = launched(&emr, "repeated reconfiguration").await;
    let core = ids[1].clone();

    for version in 1..=3_i64 {
        emr.ok(
            "ModifyInstanceGroups",
            json!({
                "ClusterId": &cluster_id,
                "InstanceGroups": [{
                    "InstanceGroupId": &core,
                    "ReconfigurationType": "OVERWRITE",
                    "Configurations": [configuration("yarn-site", "after")],
                }],
            }),
        )
        .await;
        tokio::time::sleep(PAST_TRANSITION).await;

        let groups = groups_of(&emr, &cluster_id).await;
        let group = reported("repeated reconfiguration", &groups, &core);
        assert_eq!(
            group["ConfigurationsVersion"].as_i64(),
            Some(version),
            "case \"repeated reconfiguration\": reconfiguration {version} must be version \
             {version}, got {group}"
        );
        assert_eq!(
            group["LastSuccessfullyAppliedConfigurationsVersion"].as_i64(),
            Some(version),
            "case \"repeated reconfiguration\": the applied version must have caught up, \
             got {group}"
        );
    }
}

/// THE RULE: "Policy for customizing shrink operations" — the submitted shrink policy is
/// what the group carries from then on, and a later read reports it.
#[tokio::test]
async fn a_submitted_shrink_policy_is_what_the_group_carries() {
    let emr = Emr::new();
    let (cluster_id, ids) = launched(&emr, "shrink policy").await;
    let task = ids[2].clone();

    emr.ok(
        "ModifyInstanceGroups",
        json!({
            "ClusterId": &cluster_id,
            "InstanceGroups": [{
                "InstanceGroupId": &task,
                "InstanceCount": 1,
                "ShrinkPolicy": {
                    "DecommissionTimeout": 600,
                    "InstanceResizePolicy": { "InstanceTerminationTimeout": 120 },
                },
            }],
        }),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    let groups = groups_of(&emr, &cluster_id).await;
    let group = reported("shrink policy", &groups, &task);
    assert_eq!(
        at(group, &["ShrinkPolicy", "DecommissionTimeout"]).as_i64(),
        Some(600),
        "case \"shrink policy\": the submitted DecommissionTimeout must be reported, got {group}"
    );
    assert_eq!(
        at(
            group,
            &[
                "ShrinkPolicy",
                "InstanceResizePolicy",
                "InstanceTerminationTimeout"
            ]
        )
        .as_i64(),
        Some(120),
        "case \"shrink policy\": the submitted InstanceResizePolicy must be reported, got {group}"
    );
}

// ---------------------------------------------------------------------------
// The managed scaling policy
// ---------------------------------------------------------------------------

/// THE RULE: a managed scaling policy's compute limits bound the scaling ACTIVITY of the
/// cluster, not what a caller asks an instance group for, so a target above the maximum
/// or below the minimum is applied like any other and the policy is left as it was. The
/// table spans a target above the maximum, one below the minimum, and both unit types a
/// policy measures an instance group's capacity in.
#[tokio::test]
async fn a_managed_scaling_policy_does_not_bound_a_submitted_target() {
    struct Case {
        label: &'static str,
        unit_type: &'static str,
        minimum: i64,
        maximum: i64,
        target: i64,
    }
    let cases = [
        Case {
            label: "above MaximumCapacityUnits, counted in instances",
            unit_type: "Instances",
            minimum: 1,
            maximum: 2,
            target: 5,
        },
        Case {
            label: "below MinimumCapacityUnits, counted in instances",
            unit_type: "Instances",
            minimum: 4,
            maximum: 8,
            target: 0,
        },
        Case {
            label: "above MaximumCapacityUnits, counted in vCPU cores",
            unit_type: "VCPU",
            minimum: 8,
            maximum: 16,
            target: 40,
        },
    ];

    let emr = Emr::new();
    for case in &cases {
        let (cluster_id, ids) = launched(&emr, case.label).await;
        let core = ids[1].clone();
        let policy = json!({
            "ComputeLimits": {
                "UnitType": case.unit_type,
                "MinimumCapacityUnits": case.minimum,
                "MaximumCapacityUnits": case.maximum,
            },
        });
        emr.ok(
            "PutManagedScalingPolicy",
            json!({ "ClusterId": &cluster_id, "ManagedScalingPolicy": policy }),
        )
        .await;

        emr.ok(
            "ModifyInstanceGroups",
            json!({
                "ClusterId": &cluster_id,
                "InstanceGroups": [{ "InstanceGroupId": &core, "InstanceCount": case.target }],
            }),
        )
        .await;
        tokio::time::sleep(PAST_TRANSITION).await;

        let groups = groups_of(&emr, &cluster_id).await;
        let group = reported(case.label, &groups, &core);
        assert_eq!(
            group["RequestedInstanceCount"].as_i64(),
            Some(case.target),
            "case {:?}: the submitted target must be applied, got {group}",
            case.label
        );

        // The policy is a cluster's own, and a modification is not a change to it.
        let reported_policy = emr
            .ok(
                "GetManagedScalingPolicy",
                json!({ "ClusterId": &cluster_id }),
            )
            .await;
        assert_eq!(
            at(
                &reported_policy,
                &["ManagedScalingPolicy", "ComputeLimits", "MaximumCapacityUnits"]
            )
            .as_i64(),
            Some(case.maximum),
            "case {:?}: the policy must be left as it was, got {reported_policy}",
            case.label
        );
        assert_eq!(
            at(
                &reported_policy,
                &["ManagedScalingPolicy", "ComputeLimits", "MinimumCapacityUnits"]
            )
            .as_i64(),
            Some(case.minimum),
            "case {:?}: the policy must be left as it was, got {reported_policy}",
            case.label
        );
    }
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// THE RULE: "The call will either succeed or fail atomically" — a modification the
/// service cannot carry out is refused with HTTP 400 `ValidationException`, and the
/// cluster's groups are left exactly as they were, down to the group named in the same
/// request that WAS valid. The table spans every refusal the operation owes: an unknown
/// cluster, an unknown group with and without a `ClusterId`, a resize of the group that
/// runs the primary node, a negative target, an Amazon EC2 instance id that names no
/// instance, more modifications than a cluster has groups, a specification past what a
/// group may hold, and a valid modification bundled with an invalid one.
#[tokio::test]
async fn a_refused_modification_changes_nothing() {
    let emr = Emr::new();
    let (cluster_id, ids) = launched(&emr, "refusals").await;
    let before = groups_of(&emr, &cluster_id).await;

    // One entry past `MAX_INSTANCE_GROUPS_PER_CLUSTER`, which is more modifications than
    // a cluster can have distinct groups for.
    let too_many: Vec<Value> = (0..=MAX_INSTANCE_GROUPS_PER_CLUSTER)
        .map(|_| json!({ "InstanceGroupId": &ids[1], "InstanceCount": 2 }))
        .collect();
    // One entry past `MAX_INSTANCE_GROUP_CONFIGURATIONS`.
    let too_many_configurations: Vec<Value> = (0..=MAX_INSTANCE_GROUP_CONFIGURATIONS)
        .map(|at_index| configuration(&format!("classification-{at_index}"), "after"))
        .collect();

    let cases: Vec<(&str, Value)> = vec![
        (
            "a cluster the service holds no record for",
            json!({
                "ClusterId": "j-ZZZZZZZZZZZZZ",
                "InstanceGroups": [{ "InstanceGroupId": &ids[1], "InstanceCount": 4 }],
            }),
        ),
        (
            "a group that cluster does not carry",
            json!({
                "ClusterId": &cluster_id,
                "InstanceGroups": [{ "InstanceGroupId": "ig-ZZZZZZZZZZZZZ", "InstanceCount": 4 }],
            }),
        ),
        (
            "a group no cluster carries, with no ClusterId to resolve",
            json!({
                "InstanceGroups": [{ "InstanceGroupId": "ig-ZZZZZZZZZZZZZ", "InstanceCount": 4 }],
            }),
        ),
        (
            "resizing the group that runs the primary node",
            json!({
                "ClusterId": &cluster_id,
                "InstanceGroups": [{ "InstanceGroupId": &ids[0], "InstanceCount": 2 }],
            }),
        ),
        (
            "a negative target",
            json!({
                "ClusterId": &cluster_id,
                "InstanceGroups": [{ "InstanceGroupId": &ids[1], "InstanceCount": -1 }],
            }),
        ),
        (
            "an Amazon EC2 instance id that names no instance",
            json!({
                "ClusterId": &cluster_id,
                "InstanceGroups": [{
                    "InstanceGroupId": &ids[1],
                    "EC2InstanceIdsToTerminate": ["i-0123456789abcdef0"],
                }],
            }),
        ),
        (
            "more modifications than a cluster has groups",
            json!({ "ClusterId": &cluster_id, "InstanceGroups": too_many }),
        ),
        (
            "a specification past what a group may hold",
            json!({
                "ClusterId": &cluster_id,
                "InstanceGroups": [{
                    "InstanceGroupId": &ids[1],
                    "Configurations": too_many_configurations,
                }],
            }),
        ),
        (
            "a valid modification bundled with an unknown group",
            json!({
                "ClusterId": &cluster_id,
                "InstanceGroups": [
                    { "InstanceGroupId": &ids[2], "InstanceCount": 9 },
                    { "InstanceGroupId": "ig-ZZZZZZZZZZZZZ", "InstanceCount": 4 },
                ],
            }),
        ),
    ];

    for (label, request) in &cases {
        let (status, payload) = emr.call("ModifyInstanceGroups", request.clone()).await;
        assert_refused(label, status, &payload, "ValidationException");

        let after = groups_of(&emr, &cluster_id).await;
        assert_eq!(
            after, before,
            "case {label:?}: the refused call must leave every group as it was"
        );
    }
}

/// THE RULE: a cluster that is shutting down or gone may not be modified — its groups are
/// headed for `ENDED` whatever the request asks for.
#[tokio::test]
async fn a_cluster_on_its_way_out_may_not_be_modified() {
    let emr = Emr::new();
    let (cluster_id, ids) = launched(&emr, "terminated cluster").await;

    emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [&cluster_id] }))
        .await;

    // TERMINATING straight after the call, TERMINATED once the clock makes it due: both
    // are refused.
    for label in ["shutting down", "terminated"] {
        let (status, payload) = emr
            .call(
                "ModifyInstanceGroups",
                json!({
                    "ClusterId": &cluster_id,
                    "InstanceGroups": [{ "InstanceGroupId": &ids[1], "InstanceCount": 4 }],
                }),
            )
            .await;
        assert_refused(label, status, &payload, "ValidationException");

        let groups = groups_of(&emr, &cluster_id).await;
        let group = reported(label, &groups, &ids[1]);
        assert_eq!(
            group["RequestedInstanceCount"].as_i64(),
            Some(2),
            "case {label:?}: the group must still request what it launched with, got {group}"
        );
        tokio::time::sleep(PAST_TRANSITION).await;
    }
}

/// THE RULE: a request that names no modification changes nothing, and is answered with
/// the empty success the operation models.
#[tokio::test]
async fn a_request_naming_no_modification_changes_nothing() {
    let emr = Emr::new();
    let (cluster_id, _) = launched(&emr, "no modifications").await;
    let before = groups_of(&emr, &cluster_id).await;

    for request in [
        json!({ "ClusterId": &cluster_id, "InstanceGroups": [] }),
        json!({ "ClusterId": &cluster_id }),
    ] {
        let label = format!("no modifications: {request}");
        emr.ok("ModifyInstanceGroups", request.clone()).await;
        let after = groups_of(&emr, &cluster_id).await;
        assert_eq!(
            after, before,
            "case {label:?}: an empty modification list must leave every group as it was"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `UpdateComputeEnvironment`, end to end over the wire.
//!
//! Every case drives real HTTP (restJson1) requests through the router the emulator
//! mounts, and reads the result back through `DescribeComputeEnvironments`, so what is
//! asserted is what a client sees.
//!
//! The rules under test, each over a SET of inputs:
//!
//! * an update PATCHES: every member the request carries is written to the stored
//!   environment and every member it leaves out keeps the value it had;
//! * `computeResources` is merged member-wise into the stored compute resources for the
//!   same reason;
//! * the environment is resolved by name OR by ARN, and an unknown reference is the
//!   modelled `ClientException` (HTTP 400);
//! * a member the model documents as belonging to one kind of environment is refused on
//!   the other (`computeResources` only `MANAGED`, `unmanagedvCpus` only unmanaged,
//!   `eksConfiguration` only an EKS environment), with the same 400;
//! * `desiredvCpus` "must be between the `minvCpus` and `maxvCpus` values": both bounds
//!   are accepted and a value past either is refused;
//! * an empty `subnets`/`securityGroupIds` list is "no change" for a Fargate compute type
//!   and a removal for every other one, as those members document;
//! * the environment is reported `UPDATING` until the clock owes it `VALID` again, and a
//!   read never moves it.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::Batch;

/// Every `CRType` the model declares, with whether it runs on Fargate. The names are the
/// crate's own `C_R_TYPE_VALUES` (the model's `com.amazonaws.batch#CRType` enum), and the
/// row count is asserted against it below, so a newly modelled compute type cannot escape
/// the empty-list rule's coverage.
const COMPUTE_TYPES: &[(&str, bool)] = &[
    ("EC2", false),
    ("SPOT", false),
    ("FARGATE", true),
    ("FARGATE_SPOT", true),
    ("ECS_MANAGED_INSTANCES", false),
];

/// An IAM role ARN, for the members that carry one.
fn role_arn(name: &str) -> String {
    format!(
        "arn:aws:iam::{}:role/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The compute resources a `MANAGED` environment is created with here: the two required
/// members of `ComputeResource` (`type`, `maxvCpus`) plus the ones the rows patch.
fn managed_compute_resources(compute_type: &str) -> Value {
    json!({
        "type": compute_type,
        "minvCpus": 2,
        "maxvCpus": 16,
        "desiredvCpus": 4,
        "instanceTypes": ["optimal"],
        "subnets": ["subnet-1111"],
        "securityGroupIds": ["sg-1111"],
    })
}

/// Create the compute environment `name` from `members` (everything but the name);
/// answers its ARN.
async fn create_environment(batch: &Batch, name: &str, members: &Value) -> String {
    let mut request = members.clone();
    request["computeEnvironmentName"] = json!(name);
    let body = batch.ok("/v1/createcomputeenvironment", request).await;
    body["computeEnvironmentArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `UpdateComputeEnvironment` with `members` against `reference`; answers status and body.
async fn update(batch: &Batch, reference: &str, members: &Value) -> (u16, Value) {
    let mut request = members.clone();
    request["computeEnvironment"] = json!(reference);
    batch.call("/v1/updatecomputeenvironment", request).await
}

/// The one `ComputeEnvironmentDetail` for `reference`.
async fn describe(batch: &Batch, reference: &str) -> Value {
    let body = batch
        .ok(
            "/v1/describecomputeenvironments",
            json!({"computeEnvironments": [reference]}),
        )
        .await;
    let environments = body["computeEnvironments"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        environments.len(),
        1,
        "case describe {reference}: expected exactly one ComputeEnvironmentDetail, got {body}"
    );
    environments[0].clone()
}

/// How long to wait for a clock-owed compute-environment transition to fall due: one
/// [`state::JOB_PHASE_MILLIS`] span, with margin for a slow machine.
async fn wait_for_due() {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span * 3)).await;
}

/// One row of the patch table: an environment created from `create`, updated with
/// `members`, then read back and checked at each `expected` JSON pointer.
struct Patch {
    case: &'static str,
    create: Value,
    members: Value,
    expected: Vec<(&'static str, Value)>,
}

#[tokio::test]
async fn an_update_writes_the_members_it_carries_and_keeps_the_rest() {
    let unmanaged = json!({"type": "UNMANAGED", "state": "ENABLED", "unmanagedvCpus": 4});
    let managed =
        json!({"type": "MANAGED", "state": "ENABLED", "computeResources": managed_compute_resources("EC2")});
    let eks = json!({
        "type": "MANAGED",
        "state": "ENABLED",
        "computeResources": managed_compute_resources("EC2"),
        "eksConfiguration": {
            "eksClusterArn": format!("arn:aws:eks:us-east-1:{}:cluster/batch", engine::EMULATOR_ACCOUNT_ID),
            "kubernetesNamespace": "batch-jobs",
        },
    });

    let rows = vec![
        Patch {
            case: "state only, on an unmanaged environment",
            create: unmanaged.clone(),
            members: json!({"state": "DISABLED"}),
            expected: vec![
                ("/state", json!("DISABLED")),
                // Everything the request did not name is untouched.
                ("/type", json!("UNMANAGED")),
                ("/unmanagedvCpus", json!(4)),
            ],
        },
        Patch {
            case: "unmanagedvCpus only, on an unmanaged environment",
            create: unmanaged.clone(),
            members: json!({"unmanagedvCpus": 16}),
            expected: vec![
                ("/unmanagedvCpus", json!(16)),
                ("/state", json!("ENABLED")),
            ],
        },
        Patch {
            case: "serviceRole only",
            create: unmanaged.clone(),
            members: json!({"serviceRole": role_arn("AWSBatchServiceRole")}),
            expected: vec![
                ("/serviceRole", json!(role_arn("AWSBatchServiceRole"))),
                ("/state", json!("ENABLED")),
                ("/unmanagedvCpus", json!(4)),
            ],
        },
        Patch {
            case: "context and ecsSettings together",
            create: unmanaged.clone(),
            members: json!({
                "context": "reserved",
                "ecsSettings": {"containerInsights": "ENHANCED"},
            }),
            expected: vec![
                ("/context", json!("reserved")),
                ("/ecsSettings/containerInsights", json!("ENHANCED")),
                ("/state", json!("ENABLED")),
            ],
        },
        Patch {
            case: "updatePolicy only",
            create: unmanaged.clone(),
            members: json!({
                "updatePolicy": {"terminateJobsOnUpdate": true, "jobExecutionTimeoutMinutes": 30},
            }),
            expected: vec![
                ("/updatePolicy/terminateJobsOnUpdate", json!(true)),
                ("/updatePolicy/jobExecutionTimeoutMinutes", json!(30)),
            ],
        },
        Patch {
            case: "nothing but the required member",
            create: unmanaged.clone(),
            members: json!({}),
            expected: vec![
                ("/state", json!("ENABLED")),
                ("/unmanagedvCpus", json!(4)),
                ("/type", json!("UNMANAGED")),
            ],
        },
        Patch {
            case: "computeResources maxvCpus only, merged into the stored resources",
            create: managed.clone(),
            members: json!({"computeResources": {"maxvCpus": 64}}),
            expected: vec![
                ("/computeResources/maxvCpus", json!(64)),
                // Merged, not replaced: every other member survives.
                ("/computeResources/minvCpus", json!(2)),
                ("/computeResources/desiredvCpus", json!(4)),
                ("/computeResources/type", json!("EC2")),
                ("/computeResources/instanceTypes/0", json!("optimal")),
                ("/computeResources/subnets/0", json!("subnet-1111")),
            ],
        },
        Patch {
            case: "computeResources type, subnets and security groups",
            create: managed.clone(),
            members: json!({
                "computeResources": {
                    "type": "SPOT",
                    "subnets": ["subnet-2222", "subnet-3333"],
                    "securityGroupIds": ["sg-2222"],
                    "bidPercentage": 60,
                },
            }),
            expected: vec![
                ("/computeResources/type", json!("SPOT")),
                ("/computeResources/subnets", json!(["subnet-2222", "subnet-3333"])),
                ("/computeResources/securityGroupIds", json!(["sg-2222"])),
                ("/computeResources/bidPercentage", json!(60)),
                ("/computeResources/maxvCpus", json!(16)),
            ],
        },
        Patch {
            case: "computeResources desiredvCpus inside the stored bounds",
            create: managed.clone(),
            members: json!({"computeResources": {"desiredvCpus": 8}}),
            expected: vec![
                ("/computeResources/desiredvCpus", json!(8)),
                ("/computeResources/minvCpus", json!(2)),
                ("/computeResources/maxvCpus", json!(16)),
            ],
        },
        Patch {
            case: "eksConfiguration accessEntry, on an EKS environment",
            create: eks,
            members: json!({"eksConfiguration": {"accessEntry": {"desiredState": "ENABLED"}}}),
            expected: vec![
                ("/eksConfiguration/accessEntry/desiredState", json!("ENABLED")),
                ("/containerOrchestrationType", json!("EKS")),
                ("/eksConfiguration/kubernetesNamespace", json!("batch-jobs")),
            ],
        },
    ];

    for row in rows {
        // One emulator per row: what a row asserts is this environment's own state.
        let batch = Batch::new();
        let arn = create_environment(&batch, "ce-patch", &row.create).await;
        let case = row.case;

        let (status, body) = update(&batch, "ce-patch", &row.members).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["computeEnvironmentName"].as_str(),
            Some("ce-patch"),
            "case {case}: expected the environment's name in the response, got {body}"
        );
        assert_eq!(
            body["computeEnvironmentArn"].as_str(),
            Some(arn.as_str()),
            "case {case}: expected the environment's ARN in the response, got {body}"
        );

        let detail = describe(&batch, "ce-patch").await;
        for (path, expected) in &row.expected {
            let actual = detail.pointer(path).cloned().unwrap_or(Value::Null);
            assert_eq!(
                &actual, expected,
                "case {case}: expected {path} to be {expected}, got {actual:?} in {detail}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_resolves_the_environment_by_name_or_by_arn() {
    for by_arn in [false, true] {
        let batch = Batch::new();
        let arn = create_environment(
            &batch,
            "ce-reference",
            &json!({"type": "UNMANAGED", "state": "ENABLED"}),
        )
        .await;
        let reference = if by_arn { arn.clone() } else { "ce-reference".to_string() };

        let (status, body) = update(&batch, &reference, &json!({"state": "DISABLED"})).await;
        assert_eq!(
            status, 200,
            "case update by {reference}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["computeEnvironmentArn"].as_str(),
            Some(arn.as_str()),
            "case update by {reference}: expected the environment's ARN, got {body}"
        );
        let detail = describe(&batch, "ce-reference").await;
        assert_eq!(
            detail["state"].as_str(),
            Some("DISABLED"),
            "case update by {reference}: expected the update to be persisted, got {detail}"
        );
    }
}

/// One row of the refusal table.
struct Refusal {
    case: &'static str,
    create: Value,
    /// The environment the update names; `None` means the created one.
    reference: Option<&'static str>,
    members: Value,
    /// A fragment the refusal must name, so a caller learns WHICH member is wrong.
    names: &'static str,
}

#[tokio::test]
async fn a_member_outside_its_documented_kind_or_range_is_refused() {
    let unmanaged = json!({"type": "UNMANAGED", "state": "ENABLED"});
    let managed =
        json!({"type": "MANAGED", "state": "ENABLED", "computeResources": managed_compute_resources("EC2")});

    let rows = vec![
        Refusal {
            case: "an environment nothing created",
            create: unmanaged.clone(),
            reference: Some("ce-nothing-created-this"),
            members: json!({"state": "DISABLED"}),
            names: "does not exist",
        },
        Refusal {
            case: "computeResources on an UNMANAGED environment",
            create: unmanaged.clone(),
            reference: None,
            members: json!({"computeResources": {"minvCpus": 1, "maxvCpus": 4}}),
            names: "computeResources",
        },
        Refusal {
            case: "unmanagedvCpus on a MANAGED environment",
            create: managed.clone(),
            reference: None,
            members: json!({"unmanagedvCpus": 8}),
            names: "unmanagedvCpus",
        },
        Refusal {
            case: "eksConfiguration on an ECS environment",
            create: managed.clone(),
            reference: None,
            members: json!({"eksConfiguration": {"accessEntry": {"desiredState": "ENABLED"}}}),
            names: "eksConfiguration",
        },
        Refusal {
            case: "desiredvCpus one past the stored maxvCpus",
            create: managed.clone(),
            reference: None,
            members: json!({"computeResources": {"desiredvCpus": 17}}),
            names: "desiredvCpus",
        },
        Refusal {
            case: "desiredvCpus one below the stored minvCpus",
            create: managed.clone(),
            reference: None,
            members: json!({"computeResources": {"desiredvCpus": 1}}),
            names: "desiredvCpus",
        },
        Refusal {
            case: "desiredvCpus outside the bounds the SAME request sets",
            create: managed.clone(),
            reference: None,
            members: json!({
                "computeResources": {"minvCpus": 4, "maxvCpus": 8, "desiredvCpus": 9},
            }),
            names: "desiredvCpus",
        },
        Refusal {
            case: "minvCpus above maxvCpus",
            create: managed.clone(),
            reference: None,
            members: json!({"computeResources": {"minvCpus": 32}}),
            names: "minvCpus",
        },
        Refusal {
            case: "managedInstancesProvider on resources created without one",
            create: managed.clone(),
            reference: None,
            members: json!({
                "computeResources": {
                    "managedInstancesProvider": {
                        "infrastructureRoleArn": role_arn("BatchInfrastructureRole"),
                    },
                },
            }),
            names: "managedInstancesProvider",
        },
    ];

    for row in rows {
        let batch = Batch::new();
        create_environment(&batch, "ce-refused", &row.create).await;
        let before = describe(&batch, "ce-refused").await;
        let reference = row.reference.unwrap_or("ce-refused");
        let case = row.case;

        let (status, body) = update(&batch, reference, &row.members).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled ClientException 400, got {status} {body}"
        );
        assert!(
            body.to_string().contains(row.names),
            "case {case}: the refusal must name {:?}, got {body}",
            row.names
        );
        // A refusal changes nothing a later caller sees.
        let after = describe(&batch, "ce-refused").await;
        assert_eq!(
            after, before,
            "case {case}: a refused update must leave the environment as it was, got {after}"
        );
    }
}

#[tokio::test]
async fn the_documented_vcpu_bounds_are_themselves_accepted() {
    // Each published bound of the `desiredvCpus` range ("the value must be between the
    // minvCpus and maxvCpus values"), with the stored resources' own bounds as the range.
    for desired in [2, 16] {
        let batch = Batch::new();
        create_environment(
            &batch,
            "ce-bounds",
            &json!({
                "type": "MANAGED",
                "state": "ENABLED",
                "computeResources": managed_compute_resources("EC2"),
            }),
        )
        .await;

        let (status, body) = update(
            &batch,
            "ce-bounds",
            &json!({"computeResources": {"desiredvCpus": desired}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case desiredvCpus at the bound {desired}: expected 200, got {status} {body}"
        );
        let detail = describe(&batch, "ce-bounds").await;
        assert_eq!(
            detail.pointer("/computeResources/desiredvCpus"),
            Some(&json!(desired)),
            "case desiredvCpus at the bound {desired}: expected it persisted, got {detail}"
        );
    }
}

#[tokio::test]
async fn an_empty_subnet_or_security_group_list_follows_the_compute_type() {
    assert_eq!(
        COMPUTE_TYPES.len(),
        service_com_amazonaws_batch_awsbatchv20160810::ops::types::C_R_TYPE_VALUES.len(),
        "case the CRType table: every modelled compute type needs a row, got {COMPUTE_TYPES:?}"
    );
    for (compute_type, fargate) in COMPUTE_TYPES {
        let batch = Batch::new();
        create_environment(
            &batch,
            "ce-empty-list",
            &json!({
                "type": "MANAGED",
                "state": "ENABLED",
                "computeResources": managed_compute_resources(compute_type),
            }),
        )
        .await;

        let (status, body) = update(
            &batch,
            "ce-empty-list",
            &json!({"computeResources": {"subnets": [], "securityGroupIds": []}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {compute_type} with empty lists: expected 200, got {status} {body}"
        );

        let detail = describe(&batch, "ce-empty-list").await;
        // Fargate: "handled as if this parameter wasn't specified and no change is made".
        // Every other compute type: the empty list removes the values.
        let (expected_subnets, expected_groups) = if *fargate {
            (json!(["subnet-1111"]), json!(["sg-1111"]))
        } else {
            (json!([]), json!([]))
        };
        let subnets = detail
            .pointer("/computeResources/subnets")
            .cloned()
            .unwrap_or(Value::Null);
        let groups = detail
            .pointer("/computeResources/securityGroupIds")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            subnets, expected_subnets,
            "case {compute_type} with an empty subnets list: expected {expected_subnets}, got {subnets}"
        );
        assert_eq!(
            groups, expected_groups,
            "case {compute_type} with an empty securityGroupIds list: expected {expected_groups}, got {groups}"
        );
    }
}

#[tokio::test]
async fn the_move_out_of_updating_is_owed_by_the_clock_and_never_by_a_read() {
    let batch = Batch::new();
    create_environment(
        &batch,
        "ce-status",
        &json!({"type": "UNMANAGED", "state": "ENABLED"}),
    )
    .await;
    // Past the CREATING -> VALID move, so what is observed below is the update's own.
    wait_for_due().await;
    let settled = describe(&batch, "ce-status").await;
    assert_eq!(
        settled["status"].as_str(),
        Some(state::STATUS_VALID),
        "case before the update: expected {}, got {settled}",
        state::STATUS_VALID
    );

    batch
        .ok(
            "/v1/updatecomputeenvironment",
            json!({"computeEnvironment": "ce-status", "state": "DISABLED"}),
        )
        .await;

    // The span is never zero, so the caller observes the transient status...
    let updating = describe(&batch, "ce-status").await;
    assert_eq!(
        updating["status"].as_str(),
        Some(state::STATUS_UPDATING),
        "case straight after the update: expected {}, got {updating}",
        state::STATUS_UPDATING
    );
    assert_eq!(
        updating["state"].as_str(),
        Some("DISABLED"),
        "case straight after the update: the update is already persisted, got {updating}"
    );
    // ...and reading it again does not move it on: two reads at one instant agree.
    let read_again = describe(&batch, "ce-status").await;
    assert_eq!(
        read_again, updating,
        "case a second read at the same instant: a read must not change what a later caller sees, got {read_again}"
    );

    // The clock does move it on, back to VALID, with the update still in place.
    wait_for_due().await;
    let valid = describe(&batch, "ce-status").await;
    assert_eq!(
        valid["status"].as_str(),
        Some(state::STATUS_VALID),
        "case after the owed span: expected {}, got {valid}",
        state::STATUS_VALID
    );
    assert_eq!(
        valid["state"].as_str(),
        Some("DISABLED"),
        "case after the owed span: the update must still be in place, got {valid}"
    );
}

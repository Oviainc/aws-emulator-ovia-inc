// SPDX-License-Identifier: CC0-1.0

//! Which launch types a registered revision reports as validated against, driven end to
//! end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test: `TaskDefinition$compatibilities` ("Amazon ECS validates the task
//! definition parameters with those supported by the launch type") reports a launch type
//! only when the stored revision's parameters meet that launch type's documented
//! requirements AND Amazon ECS validated the revision against that launch type at all —
//! the launch types a placement names outright in its `launchType`, plus whatever the
//! registration asked for in `requiresCompatibilities` ("The task launch type that Amazon
//! ECS validates the task definition against"). Managed-instances capacity is not one a
//! placement names outright (`CreateService$launchType`: "If you want to use Amazon ECS
//! Managed Instances, you must use the capacityProviderStrategy request parameter and omit
//! the launchType request parameter"), so a revision reports it only when its registration
//! asked to be validated against it — the network mode alone never puts it in the list.
//!
//! The same derived list is the gate every placement's `launchType` is checked against, so
//! the second suite drives `RunTask` through it.
//!
//! Every case goes through the HTTP front door, so a list that is not derived from the
//! stored revision cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, cluster_with_capacity, list, ok, refusal, service, string,
};
use crate::task_lifecycle::container;

/// The image every container here starts from; irrelevant to the rule, so one value.
const IMAGE: &str = "public.ecr.aws/docker/library/nginx:latest";

/// A legal Fargate task size (`RegisterTaskDefinition$memory`'s documented list of
/// cpu/memory pairs), so a row that declares a task size declares one Fargate accepts.
const TASK_CPU: &str = "512";
const TASK_MEMORY: &str = "1024";

/// One row of the derivation table.
struct Row {
    case: &'static str,
    /// The `networkMode` the registration declares, or `None` for the documented default
    /// (`bridge`).
    network_mode: Option<&'static str>,
    /// Whether the registration declares task-level `cpu` and `memory`.
    task_size: bool,
    /// What the registration asks to be validated against, `requiresCompatibilities`.
    requires: &'static [&'static str],
    /// The launch types the revision must report, in the order ECS reports them.
    expected: &'static [&'static str],
}

/// Register one row's revision and return the body `RegisterTaskDefinition` answered.
async fn register(router: &MultiServiceRouter, family: &str, row: &Row, case: &str) -> Value {
    let mut request = json!({
        "family": family,
        "containerDefinitions": [container("app", IMAGE)],
    });
    if let Some(mode) = row.network_mode {
        request["networkMode"] = json!(mode);
    }
    if row.task_size {
        request["cpu"] = json!(TASK_CPU);
        request["memory"] = json!(TASK_MEMORY);
    }
    if !row.requires.is_empty() {
        request["requiresCompatibilities"] = json!(row.requires);
    }
    ok(router, "RegisterTaskDefinition", request, case).await
}

/// The `compatibilities` of one projection, as a list of strings.
fn compatibilities(definition: &Value, case: &str) -> Vec<String> {
    list(definition, "compatibilities", case)
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a string launch type, got {value}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn compatibilities_report_only_the_launch_types_the_revision_was_validated_against() {
    let router = service();

    // The rows differ along everything the rule generalises over: the network mode
    // (`NetworkMode`: bridge, host, awsvpc, none, plus the documented default), whether
    // task-level cpu and memory are declared, and what the registration asked to be
    // validated against — including asking for a launch type the parameters rule out.
    let rows: [Row; 10] = [
        Row {
            case: "default network mode, asking for nothing",
            network_mode: None,
            task_size: false,
            requires: &[],
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "bridge asking for EC2",
            network_mode: Some("bridge"),
            task_size: false,
            requires: &["EC2"],
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "bridge asking for managed instances, which needs awsvpc",
            network_mode: Some("bridge"),
            task_size: false,
            requires: &["EC2", "MANAGED_INSTANCES"],
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "host with a task size",
            network_mode: Some("host"),
            task_size: true,
            requires: &[],
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "none",
            network_mode: Some("none"),
            task_size: false,
            requires: &[],
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "awsvpc without a task size, asking for nothing",
            network_mode: Some("awsvpc"),
            task_size: false,
            requires: &[],
            expected: &["EC2"],
        },
        Row {
            case: "awsvpc with a task size, asking for nothing",
            network_mode: Some("awsvpc"),
            task_size: true,
            requires: &[],
            expected: &["EC2", "FARGATE"],
        },
        Row {
            case: "awsvpc with a task size, asking for Fargate",
            network_mode: Some("awsvpc"),
            task_size: true,
            requires: &["FARGATE"],
            expected: &["EC2", "FARGATE"],
        },
        Row {
            case: "awsvpc without a task size, asking for managed instances",
            network_mode: Some("awsvpc"),
            task_size: false,
            requires: &["MANAGED_INSTANCES"],
            expected: &["EC2", "MANAGED_INSTANCES"],
        },
        Row {
            case: "awsvpc with a task size, asking for every launch type awsvpc allows",
            network_mode: Some("awsvpc"),
            task_size: true,
            requires: &["EC2", "FARGATE", "MANAGED_INSTANCES"],
            expected: &["EC2", "MANAGED_INSTANCES", "FARGATE"],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let family = format!("compatfam{index}");
        let registered = register(&router, &family, row, case).await;
        let described = ok(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": family }),
            case,
        )
        .await;

        // Both projections of the SAME stored revision, so a list rendered off the
        // request rather than off the record shows up as a difference between them.
        for (source, definition) in [
            ("RegisterTaskDefinition", &registered["taskDefinition"]),
            ("DescribeTaskDefinition", &described["taskDefinition"]),
        ] {
            let actual = compatibilities(definition, case);
            assert_eq!(
                actual, row.expected,
                "case {case:?}: {source} expected compatibilities {:?}, got {actual:?} in \
                 {definition}",
                row.expected
            );
            // What the REQUEST asked for is reported separately and unchanged: the
            // derived list is not written back onto it, nor it onto the derived list.
            let requested: Vec<String> = list(definition, "requiresCompatibilities", case)
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                requested, row.requires,
                "case {case:?}: {source} expected requiresCompatibilities {:?}, got \
                 {requested:?} in {definition}",
                row.requires
            );
        }
    }
}

#[tokio::test]
async fn a_placement_may_name_only_a_launch_type_the_revision_reports() {
    let router = service();
    let case = "setup";
    cluster_with_capacity(&router, "gate", case).await;

    // Two revisions of the SAME shape — `awsvpc` at a legal Fargate task size — that
    // differ only in whether the registration asked to be validated against
    // managed-instances capacity, which is what the derived list and so the gate follow.
    let plain = Row {
        case: "asking for nothing",
        network_mode: Some("awsvpc"),
        task_size: true,
        requires: &[],
        expected: &["EC2", "FARGATE"],
    };
    let validated = Row {
        case: "asking for managed instances",
        network_mode: Some("awsvpc"),
        task_size: true,
        requires: &["MANAGED_INSTANCES"],
        expected: &["EC2", "MANAGED_INSTANCES", "FARGATE"],
    };
    register(&router, "gateplain", &plain, case).await;
    register(&router, "gatevalidated", &validated, case).await;

    // (case, the family, the launch type asked for, whether the placement is accepted).
    let rows: [(&str, &str, &str, bool); 6] = [
        ("plain on EC2", "gateplain", "EC2", true),
        ("plain on FARGATE", "gateplain", "FARGATE", true),
        (
            "plain on MANAGED_INSTANCES",
            "gateplain",
            "MANAGED_INSTANCES",
            false,
        ),
        ("plain on EXTERNAL", "gateplain", "EXTERNAL", false),
        (
            "validated on MANAGED_INSTANCES",
            "gatevalidated",
            "MANAGED_INSTANCES",
            true,
        ),
        ("validated on FARGATE", "gatevalidated", "FARGATE", true),
    ];

    for (case, family, launch_type, accepted) in rows {
        let request = json!({
            "cluster": "gate",
            "taskDefinition": family,
            "launchType": launch_type,
            "networkConfiguration": awsvpc_network_configuration(),
        });
        if accepted {
            let run = ok(&router, "RunTask", request, case).await;
            let tasks = list(&run, "tasks", case);
            assert_eq!(
                tasks.len(),
                1,
                "case {case:?}: a launch type the revision reports places the task, got \
                 {run}"
            );
            let placed = string(&tasks[0], "launchType", case);
            assert_eq!(
                placed, launch_type,
                "case {case:?}: expected the task to record launchType {launch_type}, got \
                 {placed:?}"
            );
            continue;
        }
        let (code, message) = refusal(&router, "RunTask", request, case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the modelled refusal, got {code:?} with {message:?}"
        );
        assert_eq!(
            message,
            format!("Task definition does not support launch_type {launch_type}."),
            "case {case:?}: the refusal must name the launch type asked for, got {message:?}"
        );
    }
}

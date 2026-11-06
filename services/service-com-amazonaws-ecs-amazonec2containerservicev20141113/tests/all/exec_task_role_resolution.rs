// SPDX-License-Identifier: CC0-1.0

//! Where a launched task's IAM ROLE comes from, and what the ECS Exec gate makes of it.
//!
//! The rule under test: a placement's `overrides.taskRoleArn` is the role the task it
//! launches assumes, standing in place of the one the stored revision declared, so every
//! rule keyed on the task's role — here the one `enableExecuteCommand` is gated by — is
//! decided on the role that RESOLVES, and only a placement for which none resolves at all
//! is refused.
//!
//! The table spans where the role comes from (the revision, the placement's overrides,
//! both, neither), what a member that is PRESENT but names no role does (a blank string
//! overrides nothing), and the two operations that carry a `TaskOverride` — `RunTask` and
//! `StartTask` — against revisions in both network modes a placement of each supports.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, list, ok, refusal, register_instance, string, ACCOUNT,
};

/// The cluster every row places into.
const CLUSTER: &str = "execroles";

/// The whole of AWS's sentence for an exec-enabled launch no task role resolves for. It
/// has no variable part, and its remediation clause names where the role belongs when the
/// launch itself supplied none.
const NO_TASK_ROLE_REFUSAL: &str =
    "The task couldn't be launched because a valid taskRoleArn is not being used. Specify \
     a valid task role in your task definition and try again.";

/// A role ARN of this account, as `taskRoleArn` takes it.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// What a row supplies for a `taskRoleArn` member: nothing at all, a member naming no
/// role, or a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Absent,
    Blank,
    Named(&'static str),
}

impl Role {
    /// The member's value, or `None` when the row does not carry the member.
    fn member(self) -> Option<Value> {
        match self {
            Role::Absent => None,
            Role::Blank => Some(json!("   ")),
            Role::Named(name) => Some(json!(role_arn(name))),
        }
    }

    /// Whether this member names a role — the only thing the gate is about.
    fn names_a_role(self) -> bool {
        matches!(self, Role::Named(_))
    }
}

/// One row: what the revision declares, what the placement overrides, and so whether the
/// exec-enabled launch is accepted.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The `taskRoleArn` the REVISION is registered with.
    definition: Role,
    /// The `taskRoleArn` the placement's `overrides` carry.
    overrides: Role,
}

impl Row {
    /// The role the task ends up assuming: the override's when it names one, else the
    /// revision's.
    fn resolved(&self) -> Role {
        if self.overrides.names_a_role() {
            self.overrides
        } else {
            self.definition
        }
    }
}

/// Register one revision of `family` in `network_mode`, with `task_role` for its
/// `taskRoleArn`, and return the family name.
async fn register(
    router: &MultiServiceRouter,
    family: &str,
    network_mode: &str,
    task_role: Role,
    case: &str,
) {
    let mut request = json!({
        "family": family,
        "networkMode": network_mode,
        "containerDefinitions": [{
            "name": "web",
            "image": "public.ecr.aws/nginx/nginx:latest",
            "cpu": 0,
            "memory": 128,
        }],
    });
    if let Some(member) = task_role.member() {
        request["taskRoleArn"] = member;
    }
    ok(router, "RegisterTaskDefinition", request, case).await;
}

/// The `overrides` a row carries, or `None` when it overrides nothing.
fn overrides(role: Role) -> Option<Value> {
    role.member().map(|member| json!({ "taskRoleArn": member }))
}

/// Every row of the table, read by both placements.
const ROWS: [Row; 7] = [
    Row {
        case: "neither the revision nor the overrides name a role",
        definition: Role::Absent,
        overrides: Role::Absent,
    },
    Row {
        case: "only the revision names a role",
        definition: Role::Named("fromDefinition"),
        overrides: Role::Absent,
    },
    Row {
        case: "only the overrides name a role",
        definition: Role::Absent,
        overrides: Role::Named("fromOverrides"),
    },
    Row {
        case: "both name a role, so the overrides decide",
        definition: Role::Named("fromDefinition"),
        overrides: Role::Named("fromOverrides"),
    },
    Row {
        case: "the overrides carry a blank member over a revision that names a role",
        definition: Role::Named("fromDefinition"),
        overrides: Role::Blank,
    },
    Row {
        case: "the overrides carry a blank member and the revision names none",
        definition: Role::Absent,
        overrides: Role::Blank,
    },
    Row {
        case: "the overrides name a role over a revision whose member is blank",
        definition: Role::Blank,
        overrides: Role::Named("fromOverrides"),
    },
];

#[tokio::test]
async fn run_task_resolves_the_task_role_from_the_overrides_before_the_revision() {
    let router = service_with_capacity().await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let family = format!("runrole{index}");
        register(&router, &family, "bridge", row.definition, case).await;

        let mut request = json!({
            "cluster": CLUSTER,
            "taskDefinition": family,
            "enableExecuteCommand": true,
        });
        if let Some(overrides) = overrides(row.overrides) {
            request["overrides"] = overrides;
        }

        if row.resolved().names_a_role() {
            let body = ok(&router, "RunTask", request, case).await;
            assert!(
                list(&body, "failures", case).is_empty(),
                "case {case:?}: an accepted launch reports no failure, got {body}"
            );
            let tasks = list(&body, "tasks", case);
            assert_eq!(
                tasks.len(),
                1,
                "case {case:?}: expected one launched task, got {body}"
            );
            // Read the task back through the describe: the role the launch resolved is
            // what the STORED task reports, so a later caller sees the same override.
            let arn = string(&tasks[0], "taskArn", case);
            let stored = ok(
                &router,
                "DescribeTasks",
                json!({ "cluster": CLUSTER, "tasks": [arn] }),
                case,
            )
            .await;
            let stored = list(&stored, "tasks", case);
            let reported = stored[0]["overrides"]["taskRoleArn"].as_str();
            assert_eq!(
                reported,
                row.overrides.member().and_then(|member| member
                    .as_str()
                    .map(str::to_string))
                    .as_deref(),
                "case {case:?}: the stored task reports the overrides as given, got \
                 {stored:?}"
            );
            continue;
        }

        let (code, message) = refusal(&router, "RunTask", request, case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", NO_TASK_ROLE_REFUSAL),
            "case {case:?}: expected the no-task-role refusal, got {code:?} {message:?}"
        );
    }
}

#[tokio::test]
async fn start_task_resolves_the_task_role_the_same_way() {
    let router = service_with_capacity().await;
    // A second registered instance, so each placement names a host of its own.
    let instance = register_instance(&router, CLUSTER, "i-0123456789abcdef0", "setup").await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let family = format!("startrole{index}");
        register(&router, &family, "bridge", row.definition, case).await;

        let mut request = json!({
            "cluster": CLUSTER,
            "taskDefinition": family,
            "containerInstances": [instance.clone()],
            "enableExecuteCommand": true,
        });
        if let Some(overrides) = overrides(row.overrides) {
            request["overrides"] = overrides;
        }

        if row.resolved().names_a_role() {
            let body = ok(&router, "StartTask", request, case).await;
            assert_eq!(
                list(&body, "tasks", case).len(),
                1,
                "case {case:?}: expected one placed task, got {body}"
            );
            continue;
        }

        let (code, message) = refusal(&router, "StartTask", request, case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", NO_TASK_ROLE_REFUSAL),
            "case {case:?}: expected the no-task-role refusal, got {code:?} {message:?}"
        );
    }
}

#[tokio::test]
async fn an_awsvpc_launch_resolves_its_task_role_from_the_overrides_too() {
    // The rule is about the ROLE, so it holds whatever else the launch has to carry: an
    // `awsvpc` revision gets its own elastic network interface and so a network
    // configuration, which is checked before the role gate.
    let router = service_with_capacity().await;
    let case = "awsvpc, the role supplied only by the overrides";
    register(&router, "awsvpcrole", "awsvpc", Role::Absent, case).await;

    let body = ok(
        &router,
        "RunTask",
        json!({
            "cluster": CLUSTER,
            "taskDefinition": "awsvpcrole",
            "enableExecuteCommand": true,
            "overrides": { "taskRoleArn": role_arn("fromOverrides") },
            "networkConfiguration": crate::ecs_wire::awsvpc_network_configuration(),
        }),
        case,
    )
    .await;
    assert_eq!(
        list(&body, "tasks", case).len(),
        1,
        "case {case:?}: expected one launched task, got {body}"
    );
}

/// A router with this service mounted and ONE container instance registered into the
/// cluster every row places into, so a placement onto the cluster's EC2 capacity has
/// somewhere to run.
async fn service_with_capacity() -> MultiServiceRouter {
    let router = crate::ecs_wire::service();
    cluster_with_capacity(&router, CLUSTER, "setup").await;
    router
}

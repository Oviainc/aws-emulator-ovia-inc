// SPDX-License-Identifier: CC0-1.0

//! What a `DeleteService` LEAVES BEHIND, over a table of services rather than one case.
//!
//! The rule under test: a delete that still has cleanup to do answers with the resource in
//! the TRANSIENT status that cleanup runs in, together with the dependents it is taking
//! away with it, and only the CLOCK moves it to the terminal status. For a service that is
//! `DeleteService`'s own sentence — "the service status moves from ACTIVE to DRAINING ...
//! After all tasks have transitioned to either STOPPING or STOPPED status, the service
//! status moves from DRAINING to INACTIVE" — so the call reports `DRAINING` with the task
//! sets it is draining away (`TaskSet$status` `DRAINING`: "The tasks in the task set are
//! being stopped, and their corresponding targets are being deregistered from their target
//! group"), and the same doc's "the service is no longer visible in the console or in the
//! ListServices API operation" is true from that instant on.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::json;

use crate::ecs_wire::{
    count, create_cluster, list, ok, register_capacity, register_definition, service, string,
    ACCOUNT, SIGNED_REGION,
};
use crate::task_sets::create_external_service;

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 500 ms) to fall
/// due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// Which deployment controller places the service's tasks, and so whether the service
/// carries task sets at all (`Service$taskSets`: "a set of Amazon ECS tasks in either an
/// CodeDeploy or an `EXTERNAL` deployment").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Controller {
    /// The ECS scheduler places the tasks itself; the service holds no task set.
    Ecs,
    /// An `EXTERNAL` deployment: every task comes from a task set.
    External,
}

/// One row of the delete table.
#[derive(Debug)]
struct Row {
    case: &'static str,
    name: &'static str,
    controller: Controller,
    /// What the service wanted when it was deleted.
    desired: i64,
    /// The `scale.value` of each task set the service holds, in creation order.
    task_set_scales: &'static [f64],
    /// Whether the delete has to be forced: "You can delete a service if you have no
    /// running tasks in it and the desired task count is zero", and `force` "allows you to
    /// delete a service even if it wasn't scaled down to zero tasks".
    force: bool,
    /// Whether the request addresses the service by its ARN rather than by name.
    by_arn: bool,
    /// How many tasks the service held when it was deleted, and so how many the drain has
    /// to stop.
    placed: usize,
}

#[tokio::test]
async fn a_deleted_service_drains_with_its_task_sets_and_settles_on_the_clock() {
    let router = service();
    let cluster = "drainingsvc";
    create_cluster(&router, cluster, "setup").await;
    // The EC2 launch type runs tasks on an instance registered to the cluster, so the
    // cluster needs capacity before a scheduler or a task set can place anything.
    register_capacity(&router, cluster, "setup").await;
    register_definition(&router, "drainingfamily", "setup").await;

    // The rows differ along what the delete has to clean up: the controller that placed
    // the tasks, how many tasks the service held, how many task sets it held and at what
    // share of the service, and how the request names the service.
    let rows: [Row; 6] = [
        Row {
            case: "an ECS service scaled to zero",
            name: "idle",
            controller: Controller::Ecs,
            desired: 0,
            task_set_scales: &[],
            force: false,
            by_arn: false,
            placed: 0,
        },
        Row {
            case: "an ECS service still running tasks, forced",
            name: "busy",
            controller: Controller::Ecs,
            desired: 2,
            task_set_scales: &[],
            force: true,
            by_arn: false,
            placed: 2,
        },
        Row {
            case: "an ECS service addressed by its ARN",
            name: "byarn",
            controller: Controller::Ecs,
            desired: 0,
            task_set_scales: &[],
            force: false,
            by_arn: true,
            placed: 0,
        },
        Row {
            case: "an EXTERNAL service with one task set",
            name: "single",
            controller: Controller::External,
            desired: 1,
            task_set_scales: &[100.0],
            force: true,
            by_arn: false,
            placed: 1,
        },
        Row {
            case: "an EXTERNAL service with two task sets",
            name: "bluegreen",
            controller: Controller::External,
            desired: 2,
            task_set_scales: &[50.0, 50.0],
            force: true,
            by_arn: true,
            placed: 2,
        },
        Row {
            case: "an EXTERNAL service whose task set is scaled to zero",
            name: "parked",
            controller: Controller::External,
            desired: 0,
            task_set_scales: &[0.0],
            force: false,
            by_arn: false,
            placed: 0,
        },
    ];

    for row in &rows {
        let case = row.case;
        let service_arn =
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{cluster}/{}", row.name);

        // One service per row, so no row's drain can be read through another's record.
        match row.controller {
            Controller::Ecs => {
                ok(
                    &router,
                    "CreateService",
                    json!({
                        "cluster": cluster,
                        "serviceName": row.name,
                        "taskDefinition": "drainingfamily",
                        "desiredCount": row.desired,
                        "launchType": "EC2",
                    }),
                    case,
                )
                .await;
            }
            Controller::External => {
                create_external_service(&router, cluster, row.name, row.desired, case).await;
            }
        }
        let mut created_task_sets = Vec::new();
        for scale in row.task_set_scales {
            let created = ok(
                &router,
                "CreateTaskSet",
                json!({
                    "cluster": cluster,
                    "service": row.name,
                    "taskDefinition": "drainingfamily",
                    "launchType": "EC2",
                    "scale": { "value": scale, "unit": "PERCENT" },
                }),
                case,
            )
            .await;
            created_task_sets.push(string(&created["taskSet"], "id", case));
        }

        let mut request = if row.by_arn {
            json!({ "service": service_arn })
        } else {
            json!({ "cluster": cluster, "service": row.name })
        };
        if row.force {
            request["force"] = json!(true);
        }
        let deleted = ok(&router, "DeleteService", request, case).await;
        let deleted = &deleted["service"];

        assert_eq!(
            string(deleted, "serviceArn", case),
            service_arn,
            "case {case:?}: the delete must answer with the service it was given, got \
             {deleted}"
        );
        assert_eq!(
            string(deleted, "status", case),
            "DRAINING",
            "case {case:?}: expected the delete to leave the service draining, got \
             {deleted}"
        );
        assert_eq!(
            (
                count(deleted, "desiredCount", case),
                count(deleted, "runningCount", case),
                count(deleted, "pendingCount", case),
            ),
            (0, 0, 0),
            "case {case:?}: a draining service wants, runs and pends nothing, got \
             {deleted}"
        );

        // The task sets the drain is taking away with the service are REPORTED, each in
        // the status that drain leaves it and with the count it now computes to.
        let reported = match row.controller {
            Controller::Ecs => {
                assert!(
                    deleted["taskSets"].as_array().is_none_or(Vec::is_empty),
                    "case {case:?}: a service whose own scheduler places its tasks holds \
                     no task set, got {deleted}"
                );
                Vec::new()
            }
            Controller::External => list(deleted, "taskSets", case),
        };
        assert_eq!(
            reported.len(),
            created_task_sets.len(),
            "case {case:?}: expected every task set of the service to be reported, got \
             {deleted}"
        );
        for (task_set, id) in reported.iter().zip(&created_task_sets) {
            assert_eq!(
                string(task_set, "id", case),
                *id,
                "case {case:?}: expected the stored task sets in creation order, got \
                 {deleted}"
            );
            assert_eq!(
                string(task_set, "taskSetArn", case),
                format!(
                    "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-set/{cluster}/{}/{id}",
                    row.name
                ),
                "case {case:?}: unexpected taskSetArn for {id:?}, got {task_set}"
            );
            assert_eq!(
                string(task_set, "serviceArn", case),
                format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{}", row.name),
                "case {case:?}: the task set reports the ARN of the service it exists in, \
                 got {task_set}"
            );
            assert_eq!(
                string(task_set, "status", case),
                "DRAINING",
                "case {case:?}: expected the task set of a deleted service to be draining, \
                 got {task_set}"
            );
            assert_eq!(
                string(task_set, "stabilityStatus", case),
                "STABILIZING",
                "case {case:?}: a task set whose tasks are being stopped has not reached a \
                 steady state, got {task_set}"
            );
            assert_eq!(
                count(task_set, "computedDesiredCount", case),
                0,
                "case {case:?}: the count is derived from the service's desiredCount, which \
                 the delete took to zero, got {task_set}"
            );
        }

        // "The service status moves from ACTIVE to DRAINING, and the service is no longer
        // visible in the console or in the ListServices API operation."
        let listed = ok(&router, "ListServices", json!({ "cluster": cluster }), case).await;
        assert!(
            !list(&listed, "serviceArns", case)
                .iter()
                .any(|arn| arn.as_str() == Some(service_arn.as_str())),
            "case {case:?}: a draining service is no longer listed, got {listed}"
        );
    }

    // The DRAINING -> INACTIVE move is owed by the CLOCK, not by whoever reads next.
    tokio::time::sleep(ONE_SPAN).await;

    for row in &rows {
        let case = row.case;
        let read = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": cluster, "services": [row.name] }),
            case,
        )
        .await;
        assert!(
            list(&read, "failures", case).is_empty(),
            "case {case:?}: \"Services in the DRAINING or INACTIVE status can still be \
             viewed with the DescribeServices API operation\", got {read}"
        );
        let stored = &list(&read, "services", case)[0];
        assert_eq!(
            string(stored, "status", case),
            "INACTIVE",
            "case {case:?}: expected the owed move to INACTIVE to have fallen due, got \
             {read}"
        );
        assert!(
            stored["taskSets"].as_array().is_none_or(Vec::is_empty),
            "case {case:?}: a service that has finished draining holds no task set, got \
             {read}"
        );

        // Every task the service held was asked to stop by the drain, and the clock has
        // taken them to STOPPED.
        let stopped = ok(
            &router,
            "ListTasks",
            json!({
                "cluster": cluster,
                "serviceName": row.name,
                "desiredStatus": "STOPPED",
            }),
            case,
        )
        .await;
        assert_eq!(
            list(&stopped, "taskArns", case).len(),
            row.placed,
            "case {case:?}: expected the {} task(s) of the service to have stopped, got \
             {stopped}",
            row.placed
        );

        // `@idempotent`: deleting it again reports the same tombstone and drains nothing
        // a second time.
        let again = ok(
            &router,
            "DeleteService",
            json!({ "cluster": cluster, "service": row.name }),
            case,
        )
        .await;
        assert_eq!(
            string(&again["service"], "status", case),
            "INACTIVE",
            "case {case:?}: a repeated delete reports the drained record, got {again}"
        );
    }
}

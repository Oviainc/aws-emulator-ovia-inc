// SPDX-License-Identifier: CC0-1.0

//! What a SERVICE and its DEPLOYMENT report about the work the scheduler has done, over
//! tables that span each rule rather than over one recorded request.
//!
//! The rules under test:
//!
//! 1. every count a deployment reports is read off the tasks that exist — the ones it
//!    holds that count as healthy, the ones it lost while it still wanted them, the ones
//!    of those that never launched, and the ones of an earlier deployment it replaced;
//! 2. the share of the service's desired count a deployment is scaled to is the whole of
//!    it once that rollout has taken the service over, and none of it before (a rollout
//!    still under way) or after (a rollout a newer deployment replaced);
//! 3. a deployment has reached a steady state only once the scheduler's confirmation is
//!    in AND it holds the tasks the service wants, so every write that touches the
//!    service puts it back to `STABILIZING` until that confirmation falls due;
//! 4. the service's run status reports what the scheduler still owes it: nothing
//!    (`SteadyState`), a task or task set it still has to place or let go of
//!    (`MoreWork`), or the tombstone a finished deletion leaves (`TOMBSTONE`) — and a
//!    service the scheduler has not run at all carries no run status;
//! 5. the version of the configuration the service runs is `0` as created and advances
//!    with every new service revision it rolls out, and no other write;
//! 6. a tombstoned service keeps no event stream, and every event id is UUID-shaped;
//! 7. `createdBy` names the authenticated caller, the same principal for every service
//!    that caller creates.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so a member that is not computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    caller_principal, cluster_with_capacity, list, ok, present_count, register_definition, service,
    string,
};

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 500 ms) to fall
/// due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// Describe one service and return it, asserting it is reported rather than failed.
async fn describe(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": [service_name] }),
        case,
    )
    .await;
    assert!(
        list(&body, "failures", case).is_empty(),
        "case {case:?}: expected the service to be described, got {body}"
    );
    list(&body, "services", case)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected one described service, got {body}"))
}

/// The service's current (`PRIMARY`) deployment, which is the first one reported.
fn primary(service_view: &Value, case: &str) -> Value {
    list(service_view, "deployments", case)
        .first()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a deployment, got {service_view}")
        })
}

/// `Scale$value` of a deployment, which the projection must REPORT: a deployment that
/// carries no scale at all reads as `None`, so an assertion of a zero share cannot pass
/// against a projection that wrote no scale.
fn scale_value(deployment: &Value, case: &str) -> Option<f64> {
    match &deployment["scale"]["value"] {
        Value::Null => deployment["scale"]["unit"].as_str().map(|_| 0.0),
        value => Some(value.as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a numeric scale.value, got {value}")
        })),
    }
}

/// The four counts a deployment reports beside its running/pending split, in the order
/// `(healthyTaskCount, replacedTaskCount, failedLaunchTaskCount, totalFailedTasks)`. Each
/// must reach the wire: a count the response left out reads as `None`, not as zero.
fn deployment_counts(
    deployment: &Value,
    case: &str,
) -> (Option<i64>, Option<i64>, Option<i64>, Option<i64>) {
    (
        present_count(deployment, "healthyTaskCount", case),
        present_count(deployment, "replacedTaskCount", case),
        present_count(deployment, "failedLaunchTaskCount", case),
        present_count(deployment, "totalFailedTasks", case),
    )
}

/// Whether `id` is spelled as a UUID: five hyphen-separated groups of 8, 4, 4, 4 and 12
/// hexadecimal digits, the shape Amazon ECS spells a service event's id in.
fn is_uuid_shaped(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width && group.chars().all(|c| c.is_ascii_hexdigit())
            })
}

/// Create one EC2-launch-type service of `desired` tasks and return the reported service.
async fn create_service(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    definition_arn: &str,
    desired: i64,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": service_name,
            "taskDefinition": definition_arn,
            "desiredCount": desired,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    body["service"].clone()
}

#[tokio::test]
async fn a_created_service_reports_a_rollout_that_has_taken_nothing_over_yet() {
    let router = service();
    let cluster = "starting";
    cluster_with_capacity(&router, cluster, "setup").await;
    let definition_arn = register_definition(&router, "startingfamily", "setup").await;

    // The rule generalises over the number of tasks the rollout has to place, zero
    // included: a deployment holds none of the service's desired count until its rollout
    // has taken over, and the counts it reports are the ones its tasks support.
    let mut principals = Vec::new();
    for desired in [0_i64, 1, 3] {
        let case = &format!("a service of {desired} tasks");
        let service_name = &format!("web{desired}");
        let created = create_service(
            &router,
            cluster,
            service_name,
            &definition_arn,
            desired,
            case,
        )
        .await;

        assert_eq!(
            present_count(&created, "version", case),
            Some(0),
            "case {case:?}: the configuration a create wrote is version 0, got {created}"
        );
        assert_eq!(
            present_count(&created, "extendDeploymentTimePeriod", case),
            Some(0),
            "case {case:?}: nothing extends a deployment's time period, got {created}"
        );
        assert!(
            created["runStatus"].is_null(),
            "case {case:?}: a service the scheduler has not run yet reports no run \
             status, got {created}"
        );
        principals.push(string(&created, "createdBy", case));

        let deployment = primary(&created, case);
        assert_eq!(
            string(&deployment, "stabilityStatus", case),
            "STABILIZING",
            "case {case:?}: a rollout that just started has not been confirmed stable, \
             got {created}"
        );
        assert_eq!(
            (
                scale_value(&deployment, case),
                deployment["scale"]["unit"].as_str()
            ),
            (Some(0.0), Some("PERCENT")),
            "case {case:?}: a rollout that has taken nothing over is scaled to none of \
             the service, got {created}"
        );
        assert_eq!(
            deployment_counts(&deployment, case),
            (Some(0), Some(0), Some(0), Some(0)),
            "case {case:?}: a rollout with no task of its own running, none lost and \
             none replaced counts zeroes, got {created}"
        );

        // The CLOCK settles the rollout. Reading before the span does not.
        let early = describe(&router, cluster, service_name, case).await;
        assert_eq!(
            string(&primary(&early, case), "stabilityStatus", case),
            "STABILIZING",
            "case {case:?}: reading a service does not confirm its deployment stable, \
             got {early}"
        );

        tokio::time::sleep(ONE_SPAN).await;
        let settled = describe(&router, cluster, service_name, case).await;
        let deployment = primary(&settled, case);
        assert_eq!(
            string(&deployment, "stabilityStatus", case),
            "STEADY_STATE",
            "case {case:?}: a confirmed rollout holding what the service wants has \
             reached a steady state, got {settled}"
        );
        assert_eq!(
            (
                scale_value(&deployment, case),
                deployment["scale"]["unit"].as_str()
            ),
            (Some(100.0), Some("PERCENT")),
            "case {case:?}: a rollout that took the service over holds all of its \
             desired count, got {settled}"
        );
        // The healthy count is READ off the tasks: "Tasks for services that don't use a
        // load balancer are considered healthy if they're in the RUNNING state."
        assert_eq!(
            deployment_counts(&deployment, case),
            (Some(desired), Some(0), Some(0), Some(0)),
            "case {case:?}: expected {desired} healthy task(s) and nothing lost or \
             replaced, got {settled}"
        );
        assert_eq!(
            string(&settled, "runStatus", case),
            "SteadyState",
            "case {case:?}: a service holding what it wants owes the scheduler nothing, \
             got {settled}"
        );
        assert_eq!(
            present_count(&settled, "version", case),
            Some(0),
            "case {case:?}: reaching a steady state is no new service revision, got \
             {settled}"
        );

        // Every event the rollout wrote carries an id spelled as a UUID, and no two
        // events share one.
        let events = list(&settled, "events", case);
        assert_eq!(
            events.len(),
            2,
            "case {case:?}: reaching a steady state writes two events, got {settled}"
        );
        for event in &events {
            let id = string(event, "id", case);
            assert!(
                is_uuid_shaped(&id),
                "case {case:?}: expected a UUID-shaped event id, got {id:?}"
            );
        }
        assert_ne!(
            string(&events[0], "id", case),
            string(&events[1], "id", case),
            "case {case:?}: every event has its own id, got {events:?}"
        );
    }

    // "The principal that created the service": the authenticated caller, and the SAME
    // principal for every service that caller created.
    assert!(
        principals.iter().all(|principal| *principal == caller_principal()),
        "expected every createdBy to name the authenticated caller {:?}, got {principals:?}",
        caller_principal()
    );
}

#[tokio::test]
async fn a_write_restabilizes_the_deployment_and_only_a_new_revision_advances_the_version() {
    let router = service();
    let cluster = "rolling";
    cluster_with_capacity(&router, cluster, "setup").await;
    let definition_arn = register_definition(&router, "rollingfamily", "setup").await;
    let setup = "setup";
    create_service(&router, cluster, "rolled", &definition_arn, 1, setup).await;
    tokio::time::sleep(ONE_SPAN).await;

    // The writes that do NOT roll the service onto a new service revision: each one puts
    // the deployment back to STABILIZING without advancing the version or taking the
    // service back off the rollout that holds it. The rows differ along WHICH member the
    // update carries — a member whose doc says it "doesn't trigger a new service
    // deployment", and the same desired count the service already has.
    let unrolled: [(&str, Value); 3] = [
        (
            "re-stating the desired count the service holds",
            json!({ "desiredCount": 1 }),
        ),
        (
            "a propagateTags change",
            json!({ "propagateTags": "SERVICE" }),
        ),
        (
            "a deployment configuration change",
            json!({
                "deploymentConfiguration": {
                    "maximumPercent": 200,
                    "minimumHealthyPercent": 50,
                },
            }),
        ),
    ];
    for (case, members) in unrolled {
        let mut request = json!({ "cluster": cluster, "service": "rolled" });
        for (member, value) in members.as_object().expect("the row is an object") {
            request[member] = value.clone();
        }
        let updated = ok(&router, "UpdateService", request, case).await;
        let updated = updated["service"].clone();
        let deployment = primary(&updated, case);
        assert_eq!(
            present_count(&updated, "version", case),
            Some(0),
            "case {case:?}: an update that starts no new service revision keeps the \
             version, got {updated}"
        );
        assert_eq!(
            string(&deployment, "rolloutState", case),
            "COMPLETED",
            "case {case:?}: the rollout the service runs is the one that completed, got \
             {updated}"
        );
        assert_eq!(
            scale_value(&deployment, case),
            Some(100.0),
            "case {case:?}: the deployment still holds the whole service, got {updated}"
        );
        assert_eq!(
            string(&deployment, "stabilityStatus", case),
            "STABILIZING",
            "case {case:?}: a write puts the deployment back to stabilizing, got {updated}"
        );
        assert_eq!(
            string(&updated, "runStatus", case),
            "SteadyState",
            "case {case:?}: the service still holds the tasks it wants, got {updated}"
        );

        tokio::time::sleep(ONE_SPAN).await;
        let settled = describe(&router, cluster, "rolled", case).await;
        assert_eq!(
            string(&primary(&settled, case), "stabilityStatus", case),
            "STEADY_STATE",
            "case {case:?}: the confirmation falls due on the clock, got {settled}"
        );
    }

    // A forced new deployment rolls out a NEW service revision: the version advances, the
    // new rollout has taken nothing over yet, and the deployment it replaces is reported
    // beside it holding none of the service either.
    let case = "a forced new deployment";
    let updated = ok(
        &router,
        "UpdateService",
        json!({ "cluster": cluster, "service": "rolled", "forceNewDeployment": true }),
        case,
    )
    .await;
    let updated = updated["service"].clone();
    assert_eq!(
        present_count(&updated, "version", case),
        Some(1),
        "case {case:?}: a new service revision advances the version, got {updated}"
    );
    let deployments = list(&updated, "deployments", case);
    assert_eq!(
        deployments.len(),
        2,
        "case {case:?}: the superseded deployment is reported while it is replaced, got \
         {updated}"
    );
    assert_eq!(
        (
            string(&deployments[0], "rolloutState", case),
            scale_value(&deployments[0], case),
            string(&deployments[0], "stabilityStatus", case),
        ),
        (
            "IN_PROGRESS".to_string(),
            Some(0.0),
            "STABILIZING".to_string()
        ),
        "case {case:?}: the new rollout has taken nothing over yet, got {updated}"
    );
    assert_eq!(
        (
            string(&deployments[1], "status", case),
            scale_value(&deployments[1], case),
            string(&deployments[1], "stabilityStatus", case),
        ),
        ("ACTIVE".to_string(), Some(0.0), "STABILIZING".to_string()),
        "case {case:?}: a replaced deployment holds none of the service, got {updated}"
    );

    // Once the replacement is through, the new deployment holds the service and COUNTS
    // the task of the old one it replaced.
    tokio::time::sleep(ONE_SPAN).await;
    let settled = describe(&router, cluster, "rolled", case).await;
    let deployment = primary(&settled, case);
    assert_eq!(
        present_count(&settled, "version", case),
        Some(1),
        "case {case:?}: settling is no further service revision, got {settled}"
    );
    assert_eq!(
        (
            scale_value(&deployment, case),
            string(&deployment, "stabilityStatus", case),
        ),
        (Some(100.0), "STEADY_STATE".to_string()),
        "case {case:?}: the new rollout has taken the service over, got {settled}"
    );
    assert_eq!(
        deployment_counts(&deployment, case),
        (Some(1), Some(1), Some(0), Some(0)),
        "case {case:?}: expected one healthy task and one replaced task, got {settled}"
    );

    // Each further revision advances the version by one, and nothing else does.
    let case = "a second forced new deployment";
    let again = ok(
        &router,
        "UpdateService",
        json!({ "cluster": cluster, "service": "rolled", "forceNewDeployment": true }),
        case,
    )
    .await;
    assert_eq!(
        present_count(&again["service"], "version", case),
        Some(2),
        "case {case:?}: every new service revision advances the version, got {again}"
    );

    // A scale-out leaves the scheduler work to do, which the run status reports until the
    // tasks it placed are running.
    let case = "a scale-out";
    tokio::time::sleep(ONE_SPAN).await;
    let scaled = ok(
        &router,
        "UpdateService",
        json!({ "cluster": cluster, "service": "rolled", "desiredCount": 3 }),
        case,
    )
    .await;
    let scaled = scaled["service"].clone();
    assert_eq!(
        string(&scaled, "runStatus", case),
        "MoreWork",
        "case {case:?}: tasks still coming up are work the scheduler owes, got {scaled}"
    );
    tokio::time::sleep(ONE_SPAN).await;
    let settled = describe(&router, cluster, "rolled", case).await;
    assert_eq!(
        string(&settled, "runStatus", case),
        "SteadyState",
        "case {case:?}: the service holds the tasks it wants again, got {settled}"
    );
    assert_eq!(
        present_count(&primary(&settled, case), "healthyTaskCount", case),
        Some(3),
        "case {case:?}: every running task of the deployment counts as healthy, got \
         {settled}"
    );
}

/// One row of the deleted-service table: what the service held when it was deleted, and
/// the run status the delete must report for it.
#[derive(Debug, Clone, Copy)]
struct Deleted {
    case: &'static str,
    service_name: &'static str,
    /// The service's `desiredCount`.
    desired: i64,
    /// The deployment controller, or `None` for the ECS scheduler's own.
    controller: Option<&'static str>,
    /// `Scale$value` of the task set an external-controller row creates.
    scale: i64,
    /// Whether the row creates a task set in the service at all.
    task_set: bool,
    run_status: &'static str,
}

#[tokio::test]
async fn a_deleted_service_reports_a_tombstone_and_keeps_no_event_stream() {
    let router = service();
    let cluster = "deleted";
    cluster_with_capacity(&router, cluster, "setup").await;
    let definition_arn = register_definition(&router, "deletedfamily", "setup").await;

    // The rows differ along what the service still holds when it is deleted: nothing at
    // all, tasks the delete has to stop, a task set that holds no task of its own, and one
    // whose tasks the delete has to stop. The TASKS the delete still has to let go of are
    // what the run status reports as outstanding; the tombstone it settles into reports
    // none.
    let rows = [
        Deleted {
            case: "a service holding nothing",
            service_name: "idle",
            desired: 0,
            controller: None,
            scale: 0,
            task_set: false,
            run_status: "SteadyState",
        },
        Deleted {
            case: "a service still running a task",
            service_name: "busy",
            desired: 1,
            controller: None,
            scale: 0,
            task_set: false,
            run_status: "MoreWork",
        },
        Deleted {
            case: "a service running three tasks",
            service_name: "crowded",
            desired: 3,
            controller: None,
            scale: 0,
            task_set: false,
            run_status: "MoreWork",
        },
        Deleted {
            case: "an external-controller service holding an empty task set",
            service_name: "external",
            desired: 0,
            controller: Some("EXTERNAL"),
            scale: 0,
            task_set: true,
            run_status: "SteadyState",
        },
        Deleted {
            case: "an external-controller service whose task set holds a task",
            service_name: "working",
            desired: 1,
            controller: Some("EXTERNAL"),
            scale: 100,
            task_set: true,
            run_status: "MoreWork",
        },
        // A service nothing ever created a task set in has no deployment of its own to
        // lose, so it owes no cleanup for the want of one.
        Deleted {
            case: "an external-controller service that never held a task set",
            service_name: "bare",
            desired: 0,
            controller: Some("EXTERNAL"),
            scale: 0,
            task_set: false,
            run_status: "SteadyState",
        },
    ];

    for row in rows {
        let Deleted {
            case,
            service_name,
            desired,
            controller,
            scale,
            task_set,
            run_status: expected_run_status,
        } = row;
        let mut request = json!({
            "cluster": cluster,
            "serviceName": service_name,
            "desiredCount": desired,
            "launchType": "EC2",
        });
        match controller {
            // An EXTERNAL-controller service's tasks come from a task set, not from the
            // service, so it names no task definition of its own.
            Some(controller) => request["deploymentController"] = json!({ "type": controller }),
            None => request["taskDefinition"] = json!(definition_arn),
        }
        ok(&router, "CreateService", request, case).await;
        if task_set {
            ok(
                &router,
                "CreateTaskSet",
                json!({
                    "cluster": cluster,
                    "service": service_name,
                    "taskDefinition": definition_arn,
                    "launchType": "EC2",
                    "scale": { "value": scale, "unit": "PERCENT" },
                }),
                case,
            )
            .await;
        }
        tokio::time::sleep(ONE_SPAN).await;

        let mut delete = json!({ "cluster": cluster, "service": service_name });
        if desired > 0 {
            // "If the service is actively maintaining tasks, you can't delete it, and you
            // must update the service to a desired task count of zero" — unless the
            // delete is forced.
            delete["force"] = json!(true);
        }
        let deleted = ok(&router, "DeleteService", delete, case).await;
        let deleted = deleted["service"].clone();
        assert_eq!(
            string(&deleted, "runStatus", case),
            expected_run_status,
            "case {case:?}: unexpected run status for a service being deleted, got \
             {deleted}"
        );
        assert_eq!(
            present_count(&deleted, "version", case),
            Some(0),
            "case {case:?}: deleting a service rolls out no new revision, got {deleted}"
        );
        assert_eq!(
            present_count(&deleted, "extendDeploymentTimePeriod", case),
            Some(0),
            "case {case:?}: nothing extends a deployment's time period, got {deleted}"
        );

        // The clock finishes the deletion: what it leaves is a tombstone that runs
        // nothing and keeps no event stream.
        tokio::time::sleep(ONE_SPAN).await;
        let read = describe(&router, cluster, service_name, case).await;
        assert_eq!(
            string(&read, "runStatus", case),
            "TOMBSTONE",
            "case {case:?}: a deleted service reports its tombstone, got {read}"
        );
        assert!(
            list(&read, "events", case).is_empty(),
            "case {case:?}: a tombstoned service keeps no event stream, got {read}"
        );
        assert!(
            list(&read, "taskSets", case).is_empty(),
            "case {case:?}: a tombstoned service holds no task set, got {read}"
        );
        if controller.is_none() {
            let deployment = primary(&read, case);
            assert_eq!(
                string(&deployment, "stabilityStatus", case),
                "STEADY_STATE",
                "case {case:?}: a tombstone has nothing left to stabilize, got {read}"
            );
            assert_eq!(
                present_count(&deployment, "healthyTaskCount", case),
                Some(0),
                "case {case:?}: a tombstoned service holds no healthy task, got {read}"
            );
        }
    }

    // A service in another cluster that was never deleted still reports what it runs, so
    // the tombstone above is the deleted service's own state and not a global one.
    let case = "a live service beside the deleted ones";
    create_cluster_with_live_service(&router, &definition_arn, case).await;
}

/// Create a second cluster with one live service and assert it reports a steady state:
/// the tombstone a deletion leaves belongs to the service that was deleted.
async fn create_cluster_with_live_service(
    router: &MultiServiceRouter,
    definition_arn: &str,
    case: &str,
) {
    let cluster = "untouched";
    cluster_with_capacity(router, cluster, case).await;
    create_service(router, cluster, "live", definition_arn, 1, case).await;
    tokio::time::sleep(ONE_SPAN).await;
    let read = describe(router, cluster, "live", case).await;
    assert_eq!(
        (
            string(&read, "runStatus", case),
            string(&read, "status", case),
        ),
        ("SteadyState".to_string(), "ACTIVE".to_string()),
        "case {case:?}: a service nobody deleted keeps running, got {read}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! TASK-SET DELETION, and what every later read of the service says about it.
//!
//! One rule, asserted over a table of inputs rather than one recorded case: a
//! `DeleteTaskSet` TAKES THE TASK SET AWAY. The call leaves it `DRAINING` ("the tasks in
//! the task set are being stopped, and their corresponding targets are being deregistered
//! from their target group") at a scale of zero, stops every task it placed, and the CLOCK
//! finishes that drain — after which the task set is one the service no longer holds: a
//! `DescribeTaskSets` that names it answers 200 with it reported through a `MISSING`
//! failure rather than as a task set, the service's own projection carries it no more, and
//! no write modifies it again. The task set that is serving production traffic (`PRIMARY`)
//! is the one taken away only when the request says `force`: the recorded calls of this
//! operation delete a task set that is still scaled above zero without it, so the share of
//! the service a task set holds is not what gates the delete and its status is.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    count, create_cluster, list, ok, refusal, register_capacity, register_definition, service,
    string, ACCOUNT, SIGNED_REGION,
};
use crate::task_sets::{create_external_service, describe_task_sets};

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 500 ms) to fall
/// due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// How the request names the task set to delete. The `taskSet` doc: "The task set ID or
/// full Amazon Resource Name (ARN) of the task set to delete."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reference {
    Id,
    Arn,
}

/// One row of the deletion table.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The service's `desiredCount`, which the task set's scale is a percentage OF.
    desired: i64,
    /// `Scale$value` the task set is created with.
    scale: f64,
    /// Whether the task set is promoted to `PRIMARY` before the delete.
    primary: bool,
    /// How the delete names it.
    reference: Reference,
    /// How many task sets the service holds besides the deleted one.
    siblings: usize,
}

/// Create one task set of `service_name` and return it.
async fn create_task_set(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    external_id: &str,
    scale: f64,
    case: &str,
) -> Value {
    let created = ok(
        router,
        "CreateTaskSet",
        json!({
            "cluster": cluster,
            "service": service_name,
            "taskDefinition": "deletedfamily",
            "launchType": "EC2",
            "externalId": external_id,
            "scale": { "value": scale, "unit": "PERCENT" },
        }),
        case,
    )
    .await;
    created["taskSet"].clone()
}

/// `DeleteTaskSet`, which must answer 200.
async fn delete_task_set(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    reference: &str,
    force: bool,
    case: &str,
) -> Value {
    let mut request = json!({
        "cluster": cluster,
        "service": service_name,
        "taskSet": reference,
    });
    if force {
        request["force"] = json!(true);
    }
    let deleted = ok(router, "DeleteTaskSet", request, case).await;
    deleted["taskSet"].clone()
}

/// The ids of the task sets `DescribeTaskSets` reports for one service, oldest first.
async fn reported_ids(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Vec<String> {
    let read = describe_task_sets(router, cluster, service_name, None, false, case).await;
    list(&read, "taskSets", case)
        .iter()
        .map(|task_set| string(task_set, "id", case))
        .collect()
}

#[tokio::test]
async fn a_deleted_task_set_drains_away_and_is_then_reported_missing() {
    let router = service();
    let cluster = "deletedtasksets";
    create_cluster(&router, cluster, "setup").await;
    register_capacity(&router, cluster, "setup").await;
    register_definition(&router, "deletedfamily", "setup").await;

    // The rows differ along what the rule generalises over: the share of the service the
    // task set holds (so whether it placed tasks the delete has to stop), whether it is
    // the one serving production traffic, which reference form the delete names it by, and
    // how many task sets the service holds beside it.
    let rows = [
        Row {
            case: "a task set holding nothing, named by its id",
            desired: 0,
            scale: 0.0,
            primary: false,
            reference: Reference::Id,
            siblings: 0,
        },
        Row {
            case: "a task set holding every task of the service, named by its ARN",
            desired: 2,
            scale: 100.0,
            primary: false,
            reference: Reference::Arn,
            siblings: 0,
        },
        Row {
            case: "a task set holding half of an odd count",
            desired: 3,
            scale: 50.0,
            primary: false,
            reference: Reference::Id,
            siblings: 0,
        },
        Row {
            case: "the task set serving production traffic",
            desired: 1,
            scale: 100.0,
            primary: true,
            reference: Reference::Arn,
            siblings: 0,
        },
        Row {
            case: "one task set of three",
            desired: 2,
            scale: 50.0,
            primary: false,
            reference: Reference::Id,
            siblings: 2,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let service_name = format!("svc{index}");
        create_external_service(&router, cluster, &service_name, row.desired, case).await;
        let deleted =
            create_task_set(&router, cluster, &service_name, "deleted", row.scale, case).await;
        let id = string(&deleted, "id", case);
        let arn = string(&deleted, "taskSetArn", case);
        let mut siblings = Vec::new();
        for sibling in 0..row.siblings {
            let created = create_task_set(
                &router,
                cluster,
                &service_name,
                &format!("kept{sibling}"),
                row.scale,
                case,
            )
            .await;
            siblings.push(string(&created, "id", case));
        }
        if row.primary {
            ok(
                &router,
                "UpdateServicePrimaryTaskSet",
                json!({
                    "cluster": cluster,
                    "service": service_name,
                    "primaryTaskSet": id,
                }),
                case,
            )
            .await;
        }

        let reference = match row.reference {
            Reference::Id => id.clone(),
            Reference::Arn => arn.clone(),
        };

        // "If true, you can delete a task set even if it hasn't been scaled down to zero":
        // the task set serving production traffic is the one that condition holds for, so
        // it is taken away only when the request insists on it, and the refusal leaves it
        // exactly where it was. One that is not serving production traffic is deleted at
        // whatever share of the service it holds, which the rows above span.
        if row.primary {
            let (code, message) = refusal(
                &router,
                "DeleteTaskSet",
                json!({
                    "cluster": cluster,
                    "service": service_name,
                    "taskSet": reference,
                }),
                case,
            )
            .await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (
                    "InvalidParameterException",
                    "Primary TaskSet cannot be deleted"
                ),
                "case {case:?}: expected the primary task set to be refused, got {code:?} \
                 with {message:?}"
            );
            let still_there = reported_ids(&router, cluster, &service_name, case).await;
            assert!(
                still_there.contains(&id),
                "case {case:?}: a refused delete must take nothing away, got \
                 {still_there:?}"
            );
        }

        let answered = delete_task_set(
            &router,
            cluster,
            &service_name,
            &reference,
            row.primary,
            case,
        )
        .await;
        assert_eq!(
            string(&answered, "id", case),
            id,
            "case {case:?}: the delete must answer with the task set it was given, got \
             {answered}"
        );
        assert_eq!(
            string(&answered, "status", case),
            "DRAINING",
            "case {case:?}: the delete leaves the task set draining away, got {answered}"
        );
        assert_eq!(
            answered["scale"]["value"].as_f64().unwrap_or_default(),
            0.0,
            "case {case:?}: a task set being drained away wants no task, got {answered}"
        );
        assert_eq!(
            string(&answered, "stabilityStatus", case),
            "STABILIZING",
            "case {case:?}: a task set whose tasks are being stopped has not reached a \
             steady state, got {answered}"
        );
        assert_eq!(
            string(&answered, "serviceArn", case),
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{service_name}"),
            "case {case:?}: the task set reports the ARN of the service it exists in, got \
             {answered}"
        );
        assert_eq!(
            count(&answered, "computedDesiredCount", case),
            0,
            "case {case:?}: a task set at a scale of zero computes no desired count, got \
             {answered}"
        );

        // The drain is UNDER WAY, not done: the task set is still one the service holds,
        // and a write to it is refused on the status that drain put it in.
        let draining = describe_task_sets(
            &router,
            cluster,
            &service_name,
            Some(vec![reference.clone()]),
            false,
            case,
        )
        .await;
        assert_eq!(
            list(&draining, "taskSets", case).len(),
            1,
            "case {case:?}: a task set whose drain is under way is still reported, got \
             {draining}"
        );
        assert!(
            list(&draining, "failures", case).is_empty(),
            "case {case:?}: a task set that is still there is no failure, got {draining}"
        );
        let (code, message) = refusal(
            &router,
            "UpdateTaskSet",
            json!({
                "cluster": cluster,
                "service": service_name,
                "taskSet": reference,
                "scale": { "value": 100, "unit": "PERCENT" },
            }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                "Unable to update taskset with status = DRAINING"
            ),
            "case {case:?}: a task set that is still there but draining is refused on its \
             status, not reported absent, got {code:?} with {message:?}"
        );

        // The CLOCK finishes the drain, and from there the task set is gone: the request
        // that names it is answered through a MISSING failure rather than with a task set.
        tokio::time::sleep(ONE_SPAN).await;
        let gone = describe_task_sets(
            &router,
            cluster,
            &service_name,
            Some(vec![reference.clone()]),
            false,
            case,
        )
        .await;
        assert!(
            list(&gone, "taskSets", case).is_empty(),
            "case {case:?}: a drained-away task set is reported no more, got {gone}"
        );
        assert_eq!(
            list(&gone, "failures", case),
            vec![json!({
                "arn": format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-set/{id}"),
                "reason": "MISSING",
            })],
            "case {case:?}: expected one MISSING failure naming the task set the request \
             named by its id alone, got {gone}"
        );

        // The task sets the service does still hold are reported, in creation order, and
        // the deleted one is not among them — on the service's own projection too.
        let left = reported_ids(&router, cluster, &service_name, case).await;
        assert_eq!(
            left, siblings,
            "case {case:?}: expected only the task sets nothing deleted, got {left:?}"
        );
        let read = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": cluster, "services": [service_name.clone()] }),
            case,
        )
        .await;
        let described = list(&read, "services", case);
        let projected: Vec<String> = list(&described[0], "taskSets", case)
            .iter()
            .map(|task_set| string(task_set, "id", case))
            .collect();
        assert_eq!(
            projected, siblings,
            "case {case:?}: the service must project the task sets it holds, got {read}"
        );
    }
}

#[tokio::test]
async fn a_delete_that_names_no_task_set_of_the_service_is_refused() {
    let router = service();
    let cluster = "refuseddeletes";
    let service_name = "web";
    let case = "setup";
    create_cluster(&router, cluster, case).await;
    register_capacity(&router, cluster, case).await;
    register_definition(&router, "deletedfamily", case).await;
    create_external_service(&router, cluster, service_name, 1, case).await;
    let held = create_task_set(&router, cluster, service_name, "held", 50.0, case).await;
    let held_id = string(&held, "id", case);

    // A SECOND external service of the same cluster with a task set of its own: "Task sets
    // are specific to each cluster, service and Region", so that task set is not one this
    // service can delete.
    create_external_service(&router, cluster, "other", 1, case).await;
    let elsewhere = create_task_set(&router, cluster, "other", "elsewhere", 50.0, case).await;
    let elsewhere_id = string(&elsewhere, "id", case);

    // The rows differ along WHICH part of the address names nothing: the cluster, the
    // service, the task set, and a task set that exists but in another service.
    let rows: [(&str, &str, &str, &str, &str, String); 4] = [
        (
            "a cluster that does not exist",
            "nosuchcluster",
            service_name,
            &held_id,
            "ClusterNotFoundException",
            "Cluster not found.".to_string(),
        ),
        (
            "a service the cluster does not hold",
            cluster,
            "nosuchservice",
            &held_id,
            "ServiceNotFoundException",
            "Service not found.".to_string(),
        ),
        (
            "a task set id nothing was created under",
            cluster,
            service_name,
            "ecs-svc/0000000000000000099",
            "TaskSetNotFoundException",
            format!(
                "Unable to find task set with id ecs-svc/0000000000000000099 on service \
                 {service_name}."
            ),
        ),
        (
            "a task set of another service of the same cluster",
            cluster,
            service_name,
            &elsewhere_id,
            "TaskSetNotFoundException",
            format!("Unable to find task set with id {elsewhere_id} on service {service_name}."),
        ),
    ];

    for (case, cluster_name, named_service, reference, expected_code, expected_message) in rows {
        let (code, message) = refusal(
            &router,
            "DeleteTaskSet",
            json!({
                "cluster": cluster_name,
                "service": named_service,
                "taskSet": reference,
            }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (expected_code, expected_message.as_str()),
            "case {case:?}: expected a {expected_code} saying {expected_message:?}, got \
             {code:?} with {message:?}"
        );
    }

    // None of them deleted anything: both task sets are still there.
    let left = reported_ids(&router, cluster, service_name, "after the refusals").await;
    assert_eq!(
        left,
        vec![held_id.clone()],
        "a refused delete must take nothing away, got {left:?}"
    );
    let other = reported_ids(&router, cluster, "other", "after the refusals").await;
    assert_eq!(
        other,
        vec![elsewhere_id],
        "a refused delete must leave the other service's task set alone, got {other:?}"
    );
}

#[tokio::test]
async fn an_update_holds_the_task_set_at_the_scale_it_asks_for() {
    let router = service();
    let cluster = "scaledtasksets";
    let service_name = "web";
    let case = "setup";
    create_cluster(&router, cluster, case).await;
    register_capacity(&router, cluster, case).await;
    register_definition(&router, "deletedfamily", case).await;
    create_external_service(&router, cluster, service_name, 4, case).await;
    let created = create_task_set(&router, cluster, service_name, "scaled", 25.0, case).await;
    let id = string(&created, "id", case);

    // "A floating-point percentage of the desired number of tasks to place and keep
    // running in the task set", and `Scale$value`: "Accepted values are numbers between 0
    // and 100". The rows walk both published bounds, a value inside the range, and the
    // count the service's own desiredCount and the percentage come to —
    // `computedDesiredCount` is "calculated by multiplying the service's desiredCount by
    // the task set's scale percentage. The result is always rounded up".
    let rows: [(&str, f64, i64); 5] = [
        ("the lowest accepted percentage", 0.0, 0),
        ("a percentage that rounds up", 10.0, 1),
        ("half of the service", 50.0, 2),
        ("the highest accepted percentage", 100.0, 4),
        ("back down to nothing", 0.0, 0),
    ];

    for (case, scale, expected) in rows {
        let updated = ok(
            &router,
            "UpdateTaskSet",
            json!({
                "cluster": cluster,
                "service": service_name,
                "taskSet": id,
                "scale": { "value": scale, "unit": "PERCENT" },
            }),
            case,
        )
        .await;
        let updated = &updated["taskSet"];
        assert_eq!(
            updated["scale"]["value"].as_f64().unwrap_or_default(),
            scale,
            "case {case:?}: expected the percentage the update asked for, got {updated}"
        );
        assert_eq!(
            count(updated, "computedDesiredCount", case),
            expected,
            "case {case:?}: expected computedDesiredCount {expected}, got {updated}"
        );

        // And it is STORED: the read-back reports the same percentage and count.
        let read = describe_task_sets(
            &router,
            cluster,
            service_name,
            Some(vec![id.clone()]),
            false,
            case,
        )
        .await;
        let stored = &list(&read, "taskSets", case)[0];
        assert_eq!(
            stored["scale"]["value"].as_f64().unwrap_or_default(),
            scale,
            "case {case:?}: expected the stored percentage back, got {read}"
        );
        assert_eq!(
            count(stored, "computedDesiredCount", case),
            expected,
            "case {case:?}: expected the stored count back, got {read}"
        );
    }

    // A percentage outside the range holds no task set, so it is refused and the stored
    // one is left at the percentage it was already held at — one past each bound.
    for (case, scale) in [("just above the top bound", 100.1), ("below zero", -0.1)] {
        let (code, _) = refusal(
            &router,
            "UpdateTaskSet",
            json!({
                "cluster": cluster,
                "service": service_name,
                "taskSet": id,
                "scale": { "value": scale, "unit": "PERCENT" },
            }),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected a percentage outside the range to be refused, got \
             {code:?}"
        );
        let read = describe_task_sets(
            &router,
            cluster,
            service_name,
            Some(vec![id.clone()]),
            false,
            case,
        )
        .await;
        let stored = &list(&read, "taskSets", case)[0];
        assert_eq!(
            stored["scale"]["value"].as_f64().unwrap_or_default(),
            0.0,
            "case {case:?}: a refused update must change nothing, got {read}"
        );
    }
}

#[tokio::test]
async fn a_deleted_task_set_is_gone_from_the_service_teardown_that_follows_it() {
    let router = service();
    let cluster = "teardowntasksets";
    create_cluster(&router, cluster, "setup").await;
    register_capacity(&router, cluster, "setup").await;
    register_definition(&router, "deletedfamily", "setup").await;

    // The rows differ along WHEN the service is deleted relative to the drain the
    // task-set delete started: while it is still under way, and after the clock has
    // finished it. A service that still holds the draining task set projects it; one whose
    // task set has drained away projects none, and its run status reports the cleanup that
    // deployment left it with.
    let rows: [(&str, bool, &str); 2] = [
        ("while the drain is under way", false, "SteadyState"),
        ("after the clock finished the drain", true, "MoreWork"),
    ];

    for (index, (case, settled, expected_run_status)) in rows.into_iter().enumerate() {
        let service_name = format!("svc{index}");
        create_external_service(&router, cluster, &service_name, 0, case).await;
        let created = create_task_set(&router, cluster, &service_name, "torndown", 0.0, case).await;
        let id = string(&created, "id", case);
        delete_task_set(&router, cluster, &service_name, &id, false, case).await;
        if settled {
            tokio::time::sleep(ONE_SPAN).await;
        }

        let deleted = ok(
            &router,
            "DeleteService",
            json!({ "cluster": cluster, "service": service_name.clone() }),
            case,
        )
        .await;
        let deleted = &deleted["service"];
        let projected: Vec<String> = list(deleted, "taskSets", case)
            .iter()
            .map(|task_set| string(task_set, "id", case))
            .collect();
        let expected_projection: Vec<String> = if settled { Vec::new() } else { vec![id.clone()] };
        assert_eq!(
            projected, expected_projection,
            "case {case:?}: the delete must project the task sets the service still \
             holds, got {deleted}"
        );
        assert_eq!(
            string(deleted, "runStatus", case),
            expected_run_status,
            "case {case:?}: unexpected run status for the deleted service, got {deleted}"
        );

        // And once the service's OWN drain is over, what is left is the tombstone, which
        // runs nothing and holds no task set whichever way the delete found it.
        tokio::time::sleep(ONE_SPAN).await;
        let read = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": cluster, "services": [service_name.clone()] }),
            case,
        )
        .await;
        let described = list(&read, "services", case);
        assert_eq!(
            (
                string(&described[0], "status", case),
                string(&described[0], "runStatus", case),
            ),
            ("INACTIVE".to_string(), "TOMBSTONE".to_string()),
            "case {case:?}: expected the tombstone a finished deletion leaves, got {read}"
        );
        assert!(
            list(&described[0], "taskSets", case).is_empty(),
            "case {case:?}: a tombstoned service holds no task set, got {read}"
        );
    }
}

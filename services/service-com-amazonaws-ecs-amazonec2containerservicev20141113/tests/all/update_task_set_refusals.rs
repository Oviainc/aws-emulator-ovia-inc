// SPDX-License-Identifier: CC0-1.0

//! How `UpdateTaskSet` REFUSES: which error class a request earns, and the sentence it
//! carries.
//!
//! Two rules, each asserted over a table of inputs rather than one case:
//!
//! * A task set the store holds is never an absent one. The task set is resolved whatever
//!   status it stands in, and one whose status says it is being taken away (`DRAINING`:
//!   "the tasks in the task set are being stopped") cannot be held at any percentage of
//!   the service, so the scale is refused with `InvalidParameterException` naming the
//!   status READ FROM STATE — a condition the caller waits out, told apart from a resource
//!   that is gone. Only a task set this service does not hold is a
//!   `TaskSetNotFoundException`.
//! * That not-found refusal names both identifiers the request addressed the task set by:
//!   the task-set id it asked for and the service it was looked for in.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    create_cluster, list, ok, refusal, register_capacity, register_definition, service, string,
    ACCOUNT, SIGNED_REGION,
};
use crate::task_sets::{create_external_service, describe_task_sets};

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 500 ms) to fall
/// due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// The family every task set of this suite places.
const FAMILY: &str = "updatedtaskset";

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
            "taskDefinition": FAMILY,
            "launchType": "EC2",
            "externalId": external_id,
            "scale": { "value": scale, "unit": "PERCENT" },
        }),
        case,
    )
    .await;
    created["taskSet"].clone()
}

/// `UpdateTaskSet` at `scale`, answering the HTTP status and body through the harness.
fn update_request(cluster: &str, service_name: &str, reference: &str, scale: f64) -> Value {
    json!({
        "cluster": cluster,
        "service": service_name,
        "taskSet": reference,
        "scale": { "value": scale, "unit": "PERCENT" },
    })
}

/// The ARN a task set of `service_name` in `cluster` carries for id `id` — the other form
/// the `taskSet` member takes ("The short name or full Amazon Resource Name (ARN) of the
/// task set to update").
fn task_set_arn(cluster: &str, service_name: &str, id: &str) -> String {
    format!(
        "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-set/{cluster}/{service_name}/{id}"
    )
}

/// The `scale.value` a task set reports, with the modelled default of `0` for a member the
/// wire omits because it equals that default.
fn scale_value(task_set: &Value, case: &str) -> f64 {
    match &task_set["scale"]["value"] {
        Value::Null => 0.0,
        value => value
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a numeric scale.value, got {value}")),
    }
}

/// The ONE task set `DescribeTaskSets` reports for `reference`, or `None` when the service
/// holds it no more.
async fn stored_task_set(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    reference: &str,
    case: &str,
) -> Option<Value> {
    let read = describe_task_sets(
        router,
        cluster,
        service_name,
        Some(vec![reference.to_string()]),
        false,
        case,
    )
    .await;
    list(&read, "taskSets", case).into_iter().next()
}

#[tokio::test]
async fn update_task_set_refuses_a_task_set_the_service_does_not_hold() {
    let router = service();
    let case = "setup";
    let cluster = "updaterefusals";
    let service_name = "web";
    create_cluster(&router, cluster, case).await;
    register_capacity(&router, cluster, case).await;
    register_definition(&router, FAMILY, case).await;
    create_external_service(&router, cluster, service_name, 4, case).await;
    let held = create_task_set(&router, cluster, service_name, "held", 25.0, case).await;
    let held_id = string(&held, "id", case);

    // A SECOND external service of the same cluster, holding a task set of its own: "Task
    // sets are specific to each cluster, service and Region", so that task set is not one
    // this service holds either.
    create_external_service(&router, cluster, "other", 4, case).await;
    let elsewhere = create_task_set(&router, cluster, "other", "elsewhere", 25.0, case).await;
    let elsewhere_id = string(&elsewhere, "id", case);

    // The rows differ along WHY the service holds no such task set and along the FORM the
    // request names it in: an id nothing was created under, another one, the ARN form of
    // an absent id, and a task set that exists but in another service of the same cluster,
    // by id and by ARN. The expected message is built from the id the request named and
    // the service it named, which is what the sentence interpolates.
    let rows: [(&str, String, String); 5] = [
        (
            "an id nothing was created under",
            "ecs-svc/0000000000000000099".to_string(),
            "ecs-svc/0000000000000000099".to_string(),
        ),
        (
            "another id nothing was created under",
            "ecs-svc/9999999999999999999".to_string(),
            "ecs-svc/9999999999999999999".to_string(),
        ),
        (
            "the ARN form of an absent id",
            task_set_arn(cluster, service_name, "ecs-svc/0000000000000000077"),
            "ecs-svc/0000000000000000077".to_string(),
        ),
        (
            "a task set of another service, by id",
            elsewhere_id.clone(),
            elsewhere_id.clone(),
        ),
        (
            "a task set of another service, by ARN",
            task_set_arn(cluster, "other", &elsewhere_id),
            elsewhere_id.clone(),
        ),
    ];

    for (case, reference, named_id) in rows {
        let (code, message) = refusal(
            &router,
            "UpdateTaskSet",
            update_request(cluster, service_name, &reference, 100.0),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "TaskSetNotFoundException",
                format!("TaskSet: {named_id} does not exist in Service: {service_name}.").as_str()
            ),
            "case {case:?}: expected the not-found refusal naming the task set and the \
             service, got {code:?} with {message:?}"
        );
    }

    // None of the refusals wrote anything: the task set the service does hold is still at
    // the percentage it was created with, and so is the other service's.
    for (service_name, reference) in [(service_name, &held_id), ("other", &elsewhere_id)] {
        let case = "after the refusals";
        let stored = stored_task_set(&router, cluster, service_name, reference, case)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected {reference} to still be there"));
        assert_eq!(
            scale_value(&stored, case),
            25.0,
            "case {case:?}: a refused update must hold the task set where it was, got \
             {stored}"
        );
    }
}

#[tokio::test]
async fn update_task_set_refuses_a_task_set_whose_status_does_not_take_a_scale() {
    let router = service();
    let case = "setup";
    let cluster = "updatestatuses";
    let service_name = "web";
    create_cluster(&router, cluster, case).await;
    register_capacity(&router, cluster, case).await;
    register_definition(&router, FAMILY, case).await;
    create_external_service(&router, cluster, service_name, 4, case).await;

    // The rows span the statuses a task set stands in — `TaskSet$status` "PRIMARY: The task
    // set is serving production traffic. ACTIVE: The task set isn't serving production
    // traffic. DRAINING: The tasks in the task set are being stopped" — and, for the one
    // that is refused, both forms the request may name it in.
    #[derive(Debug, Clone, Copy)]
    enum Named {
        Id,
        Arn,
    }
    let rows: [(&str, bool, bool, Named); 4] = [
        ("a task set serving no production traffic", false, false, Named::Id),
        ("the task set serving production traffic", true, false, Named::Id),
        ("a task set being drained away, by id", false, true, Named::Id),
        ("a task set being drained away, by ARN", false, true, Named::Arn),
    ];

    for (index, (case, primary, drained, named)) in rows.into_iter().enumerate() {
        let created = create_task_set(
            &router,
            cluster,
            service_name,
            &format!("row{index}"),
            25.0,
            case,
        )
        .await;
        let id = string(&created, "id", case);
        let arn = string(&created, "taskSetArn", case);
        let reference = match named {
            Named::Id => id.clone(),
            Named::Arn => arn.clone(),
        };
        if primary {
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
        if drained {
            ok(
                &router,
                "DeleteTaskSet",
                json!({
                    "cluster": cluster,
                    "service": service_name,
                    "taskSet": id,
                    "force": true,
                }),
                case,
            )
            .await;
        }

        // Whatever the row did to it, the task set is one the service STILL holds: the
        // status the refusal is about is read off the stored record.
        let stored = stored_task_set(&router, cluster, service_name, &reference, case)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the task set to be reported"));
        let status = string(&stored, "status", case);

        if drained {
            let (code, message) = refusal(
                &router,
                "UpdateTaskSet",
                update_request(cluster, service_name, &reference, 100.0),
                case,
            )
            .await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (
                    "InvalidParameterException",
                    format!("Unable to update taskset with status = {status}").as_str()
                ),
                "case {case:?}: a task set that is present but not updatable is refused on \
                 its stored status, got {code:?} with {message:?}"
            );
            // And nothing was written: the task set is still there, at the percentage its
            // own delete left it at.
            let after = stored_task_set(&router, cluster, service_name, &reference, case)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: expected the task set to be reported"));
            assert_eq!(
                (scale_value(&after, case), string(&after, "status", case)),
                (scale_value(&stored, case), status.clone()),
                "case {case:?}: a refused update must change nothing, got {after}"
            );

            // Once the CLOCK finishes the drain the task set really is gone, and the same
            // call is answered as the absent resource it now names.
            tokio::time::sleep(ONE_SPAN).await;
            let (code, message) = refusal(
                &router,
                "UpdateTaskSet",
                update_request(cluster, service_name, &reference, 100.0),
                case,
            )
            .await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (
                    "TaskSetNotFoundException",
                    format!("TaskSet: {id} does not exist in Service: {service_name}.").as_str()
                ),
                "case {case:?}: a drained-away task set is absent, got {code:?} with \
                 {message:?}"
            );
        } else {
            let updated = ok(
                &router,
                "UpdateTaskSet",
                update_request(cluster, service_name, &reference, 50.0),
                case,
            )
            .await;
            let updated = &updated["taskSet"];
            assert_eq!(
                (scale_value(updated, case), string(updated, "status", case)),
                (50.0, status.clone()),
                "case {case:?}: a task set in a status that takes a scale is held at the \
                 percentage asked for, keeping its status, got {updated}"
            );
            // And it is STORED: the read-back reports the same percentage.
            let after = stored_task_set(&router, cluster, service_name, &reference, case)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: expected the task set to be reported"));
            assert_eq!(
                scale_value(&after, case),
                50.0,
                "case {case:?}: expected the stored percentage back, got {after}"
            );
        }
    }
}

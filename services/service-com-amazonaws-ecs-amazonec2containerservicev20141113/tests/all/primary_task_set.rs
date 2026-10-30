// SPDX-License-Identifier: CC0-1.0

//! WHICH TASK SET OF A SERVICE SERVES PRODUCTION TRAFFIC.
//!
//! One rule, asserted over a table of promotions rather than one recorded case: at most
//! one task set of a service is the primary one (`TaskSet$status`: "PRIMARY: The task set
//! is serving production traffic. ACTIVE: The task set isn't serving production traffic"),
//! so promoting a task set of a service makes that one primary, returns whichever was
//! primary before to `ACTIVE`, leaves every other task set where it was, and hands the
//! service the parameters the new primary task set deploys with ("Any parameters that are
//! updated on the primary task set in a service will transition to the service"). A task
//! set the service does not hold is refused, and nothing is promoted.
//!
//! The rows differ along the task set promoted (its position in the service, its task
//! definition and the capacity it runs on), the form the request names it by (id or ARN),
//! and whether it was already the primary one — the recorded case is one row.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{awsvpc_network_configuration, list, ok, refusal, string};
use crate::task_sets::{create_external_service, describe_task_sets, register_fargate_definition};

/// How the request names the task set to promote. The `primaryTaskSet` doc: "The short
/// name or full Amazon Resource Name (ARN) of the task set to set as the primary task set
/// in the deployment."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reference {
    Id,
    Arn,
}

/// One task set the suite creates, and the parameters promoting it must hand the service.
#[derive(Debug, Clone)]
struct Created {
    external_id: &'static str,
    id: String,
    arn: String,
    task_definition: String,
    launch_type: &'static str,
}

/// Describe one service and return it.
async fn describe_service(
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
    list(&body, "services", case)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected the service to be described, got {body}"))
}

/// The `(externalId, status)` of every task set the service holds, oldest first.
async fn statuses(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Vec<(String, String)> {
    let read = describe_task_sets(router, cluster, service_name, None, false, case).await;
    list(&read, "taskSets", case)
        .iter()
        .map(|task_set| {
            (
                string(task_set, "externalId", case),
                string(task_set, "status", case),
            )
        })
        .collect()
}

/// Create the three task sets the promotion table switches between: two on the EC2
/// capacity registered to the cluster and one on Fargate, each with its own revision, so
/// a promotion changes the parameters the service reports.
async fn create_task_sets(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Vec<Created> {
    let rows: [(&'static str, &'static str, &'static str, f64); 3] = [
        ("blue", "bluefamily", "EC2", 100.0),
        ("green", "greenfamily", "EC2", 50.0),
        ("canary", "fargatefamily", "FARGATE", 50.0),
    ];
    let mut created = Vec::new();
    for (external_id, family, launch_type, scale) in rows {
        let mut request = json!({
            "cluster": cluster,
            "service": service_name,
            "taskDefinition": family,
            "launchType": launch_type,
            "externalId": external_id,
            "scale": { "value": scale, "unit": "PERCENT" },
        });
        if launch_type == "FARGATE" {
            // An `awsvpc` revision gets its own elastic network interface, so the task
            // set has to say which subnets and security groups it takes.
            request["networkConfiguration"] = awsvpc_network_configuration();
        }
        let body = ok(router, "CreateTaskSet", request, case).await;
        let task_set = &body["taskSet"];
        assert_eq!(
            string(task_set, "status", case),
            "ACTIVE",
            "case {case:?}: a created task set is not serving production traffic yet, got \
             {task_set}"
        );
        created.push(Created {
            external_id,
            id: string(task_set, "id", case),
            arn: string(task_set, "taskSetArn", case),
            task_definition: string(task_set, "taskDefinition", case),
            launch_type,
        });
    }
    created
}

#[tokio::test]
async fn promoting_a_task_set_leaves_exactly_one_primary_and_the_service_follows_it() {
    let router = crate::ecs_wire::service();
    let cluster = "primarytaskset";
    let service_name = "web";
    let case = "setup";
    crate::ecs_wire::create_cluster(&router, cluster, case).await;
    crate::ecs_wire::register_capacity(&router, cluster, case).await;
    crate::ecs_wire::register_definition(&router, "bluefamily", case).await;
    crate::ecs_wire::register_definition(&router, "greenfamily", case).await;
    register_fargate_definition(&router, "fargatefamily", case).await;
    create_external_service(&router, cluster, service_name, 2, case).await;
    let created = create_task_sets(&router, cluster, service_name, case).await;

    // The rows walk a service through its task sets: a first promotion with no primary
    // task set to displace, a switch named by ARN, that same promotion repeated (so the
    // task set that is already primary stays the only one), and a switch back.
    let promotions: [(&str, usize, Reference); 4] = [
        ("first promotion, by id", 0, Reference::Id),
        ("switch to another task set, by ARN", 2, Reference::Arn),
        ("the same task set again, by id", 2, Reference::Id),
        ("switch back, by id", 1, Reference::Id),
    ];

    for (case, index, reference) in promotions {
        let target = created[index].clone();
        let named = match reference {
            Reference::Id => target.id.clone(),
            Reference::Arn => target.arn.clone(),
        };
        let body = ok(
            &router,
            "UpdateServicePrimaryTaskSet",
            json!({
                "cluster": cluster,
                "service": service_name,
                "primaryTaskSet": named,
            }),
            case,
        )
        .await;
        let promoted = &body["taskSet"];
        assert_eq!(
            string(promoted, "id", case),
            target.id,
            "case {case:?}: expected the promotion to answer with the task set it was \
             given ({named}), got {promoted}"
        );
        assert_eq!(
            string(promoted, "status", case),
            "PRIMARY",
            "case {case:?}: expected the promoted task set to serve production traffic, \
             got {promoted}"
        );

        // Read it back: exactly one task set of the service is primary, it is the one just
        // promoted, and every other one is ACTIVE.
        let read = statuses(&router, cluster, service_name, case).await;
        let expected: Vec<(String, String)> = created
            .iter()
            .map(|task_set| {
                (
                    task_set.external_id.to_string(),
                    if task_set.id == target.id {
                        "PRIMARY"
                    } else {
                        "ACTIVE"
                    }
                    .to_string(),
                )
            })
            .collect();
        assert_eq!(
            read, expected,
            "case {case:?}: expected {expected:?} after promoting {:?}, got {read:?}",
            target.external_id
        );

        // "Any parameters that are updated on the primary task set in a service will
        // transition to the service": the service deploys through its primary task set, so
        // the revision and the capacity it reports are that task set's.
        let service_view = describe_service(&router, cluster, service_name, case).await;
        assert_eq!(
            (
                string(&service_view, "taskDefinition", case),
                string(&service_view, "launchType", case),
            ),
            (target.task_definition.clone(), target.launch_type.to_string()),
            "case {case:?}: expected the service to deploy with the promoted task set's \
             revision and capacity, got {service_view}"
        );
    }
}

/// One row of the refusal table: what a request that names no task set of the service is
/// refused with.
#[derive(Debug, Clone)]
struct Refusal {
    case: &'static str,
    cluster: &'static str,
    /// The service named, or `None` for the service the suite created.
    service: Option<&'static str>,
    /// The `primaryTaskSet` named, or `None` for the task set the suite created in another
    /// service of the same cluster.
    task_set: Option<&'static str>,
    code: &'static str,
    message: String,
}

/// The sentence a `TaskSetNotFoundException` carries, which NAMES the task set the
/// request asked for and the service it was looked for in
/// (`crate::state::task_set_not_found_message`).
fn task_set_not_found(reference: &str, service_name: &str) -> String {
    format!("Unable to find task set with id {reference} on service {service_name}.")
}

#[tokio::test]
async fn a_task_set_the_service_does_not_hold_is_refused_and_promotes_nothing() {
    let router = crate::ecs_wire::service();
    let cluster = "refusedprimary";
    let service_name = "web";
    let case = "setup";
    crate::ecs_wire::create_cluster(&router, cluster, case).await;
    crate::ecs_wire::register_capacity(&router, cluster, case).await;
    crate::ecs_wire::register_definition(&router, "refusedfamily", case).await;
    create_external_service(&router, cluster, service_name, 1, case).await;
    let held = ok(
        &router,
        "CreateTaskSet",
        json!({
            "cluster": cluster,
            "service": service_name,
            "taskDefinition": "refusedfamily",
            "launchType": "EC2",
            "externalId": "held",
            "scale": { "value": 100, "unit": "PERCENT" },
        }),
        case,
    )
    .await;
    let held = string(&held["taskSet"], "id", case);

    // A SECOND external service of the same cluster, with a task set of its own: "Task
    // sets are specific to each cluster, service and Region", so that task set is not one
    // this service can promote.
    create_external_service(&router, cluster, "other", 1, case).await;
    let elsewhere = ok(
        &router,
        "CreateTaskSet",
        json!({
            "cluster": cluster,
            "service": "other",
            "taskDefinition": "refusedfamily",
            "launchType": "EC2",
            "externalId": "elsewhere",
            "scale": { "value": 100, "unit": "PERCENT" },
        }),
        case,
    )
    .await;
    let elsewhere = string(&elsewhere["taskSet"], "id", case);

    // A service whose own scheduler places its tasks holds no task set at all.
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": "scheduled",
            "desiredCount": 0,
            "taskDefinition": "refusedfamily",
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    // And a service the caller has deleted is not one to update at all.
    create_external_service(&router, cluster, "deleted", 0, case).await;
    ok(
        &router,
        "DeleteService",
        json!({ "cluster": cluster, "service": "deleted" }),
        case,
    )
    .await;

    let rows = [
        Refusal {
            case: "a cluster that does not exist",
            cluster: "nosuchcluster",
            service: None,
            task_set: None,
            code: "ClusterNotFoundException",
            message: "Cluster not found.".to_string(),
        },
        Refusal {
            case: "a service the cluster does not hold",
            cluster,
            service: Some("nosuchservice"),
            task_set: None,
            code: "ServiceNotFoundException",
            message: "Service not found.".to_string(),
        },
        Refusal {
            case: "a task set id nothing was created under",
            cluster,
            service: Some(service_name),
            task_set: Some("ecs-svc/0000000000000000099"),
            code: "TaskSetNotFoundException",
            message: task_set_not_found("ecs-svc/0000000000000000099", service_name),
        },
        Refusal {
            case: "a task set of another service of the same cluster",
            cluster,
            service: Some(service_name),
            task_set: None,
            code: "TaskSetNotFoundException",
            message: task_set_not_found(&elsewhere, service_name),
        },
        Refusal {
            case: "a service whose own scheduler places its tasks",
            cluster,
            service: Some("scheduled"),
            task_set: Some("ecs-svc/0000000000000000001"),
            code: "TaskSetNotFoundException",
            message: task_set_not_found("ecs-svc/0000000000000000001", "scheduled"),
        },
        Refusal {
            case: "a service the caller deleted",
            cluster,
            service: Some("deleted"),
            task_set: None,
            code: "ServiceNotActiveException",
            message: "service \"deleted\" is DRAINING and cannot be updated".to_string(),
        },
    ];

    for row in rows {
        let case = row.case;
        let (code, message) = refusal(
            &router,
            "UpdateServicePrimaryTaskSet",
            json!({
                "cluster": row.cluster,
                "service": row.service.unwrap_or(service_name),
                "primaryTaskSet": row.task_set.unwrap_or(&elsewhere),
            }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (row.code, row.message.as_str()),
            "case {case:?}: expected a {} saying {:?}, got {code:?} with {message:?}",
            row.code,
            row.message
        );
    }

    // Nothing was promoted by any of them: the task set the service does hold is still the
    // ACTIVE one it was created as.
    let read = statuses(&router, cluster, service_name, "after the refusals").await;
    assert_eq!(
        read,
        vec![("held".to_string(), "ACTIVE".to_string())],
        "a refused promotion must promote nothing, got {read:?} (task set {held})"
    );
}

#[tokio::test]
async fn promotions_in_flight_together_all_land_and_leave_one_primary_task_set() {
    let router = crate::ecs_wire::service();
    let cluster = "racingprimary";
    let service_name = "web";
    let case = "promotions in flight together";
    crate::ecs_wire::create_cluster(&router, cluster, case).await;
    crate::ecs_wire::register_capacity(&router, cluster, case).await;
    crate::ecs_wire::register_definition(&router, "bluefamily", case).await;
    crate::ecs_wire::register_definition(&router, "greenfamily", case).await;
    register_fargate_definition(&router, "fargatefamily", case).await;
    create_external_service(&router, cluster, service_name, 2, case).await;
    let created = create_task_sets(&router, cluster, service_name, case).await;

    // Three promotions of three different task sets of the one service, in flight
    // together: each is a request the service can serve, so none of them fails and the
    // store is left with one primary task set rather than three.
    let promotions = [0usize, 1, 2].map(|index| {
        let named = created[index].id.clone();
        let router = &router;
        async move {
            let body = ok(
                router,
                "UpdateServicePrimaryTaskSet",
                json!({
                    "cluster": cluster,
                    "service": service_name,
                    "primaryTaskSet": named,
                }),
                case,
            )
            .await;
            string(&body["taskSet"], "status", case)
        }
    });
    let [first, second, third] = promotions;
    let answered = tokio::join!(first, second, third);
    for status in [&answered.0, &answered.1, &answered.2] {
        assert_eq!(
            status, "PRIMARY",
            "case {case:?}: every promotion answers with the task set it promoted, got \
             {answered:?}"
        );
    }

    let read = statuses(&router, cluster, service_name, case).await;
    let primary: Vec<&(String, String)> = read
        .iter()
        .filter(|(_, status)| status == "PRIMARY")
        .collect();
    assert_eq!(
        primary.len(),
        1,
        "case {case:?}: exactly one task set may serve production traffic, got {read:?}"
    );

    // And the service deploys through the one that won: its revision is the primary task
    // set's, whichever promotion landed last.
    let service_view = describe_service(&router, cluster, service_name, case).await;
    let winner = created
        .iter()
        .find(|task_set| task_set.external_id == primary[0].0)
        .unwrap_or_else(|| panic!("case {case:?}: the primary task set is one of {created:?}"));
    assert_eq!(
        (
            string(&service_view, "taskDefinition", case),
            string(&service_view, "launchType", case),
        ),
        (
            winner.task_definition.clone(),
            winner.launch_type.to_string()
        ),
        "case {case:?}: expected the service to deploy with {:?}'s revision and capacity, \
         got {service_view}",
        winner.external_id
    );
}

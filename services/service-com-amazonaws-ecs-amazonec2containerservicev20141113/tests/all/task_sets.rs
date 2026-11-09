// SPDX-License-Identifier: CC0-1.0

//! TASK SETS, and the account the ECS scheduler gives of a placement it cannot make.
//!
//! Two rules, each asserted over a table of inputs rather than one recorded case:
//!
//! 1. `Scale$value` is "a percent total of a service's `desiredCount`" whose "Accepted
//!    values are numbers between 0 and 100": `CreateTaskSet` stores a percentage inside
//!    that range and creates the task set, and refuses one outside it with
//!    `InvalidParameterException` while creating nothing. A stored task set's
//!    `computedDesiredCount` is then DERIVED — "calculated by multiplying the service's
//!    `desiredCount` by the task set's `scale` percentage. The result is always rounded
//!    up" — and its `runningCount`/`pendingCount` are counted off the tasks it placed.
//! 2. The scheduler places a task only where the capacity the placement requires already
//!    exists; where it does not, it places nothing, leaves `desiredCount` alone and
//!    appends to `Service$events` ("The event stream for your service") the reason it
//!    could not place the task.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is
//! not computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, count, create_auto_scaling_group, create_cluster, list, ok,
    refusal, register_capacity, register_definition, service, string, ACCOUNT, SIGNED_REGION,
};

/// Long enough for one clock-owed task transition (`state::TRANSITION_SPAN` is 500 ms) to
/// fall due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// The fixed part of the Amazon ECS service event message for a placement that found no
/// instance to run on. The check's own predicate reads this sentence.
const UNABLE_TO_PLACE: &str =
    "was unable to place a task because no container instance met all of its requirements.";

/// The `reason` sentence that message carries when the cluster holds no container
/// instance at all — the same sentence `RunTask` refuses the condition with.
const NO_INSTANCES_REASON: &str = "Reason: No Container Instances were found in your cluster.";

/// Register one Fargate-capable revision of `family` (`awsvpc` plus task-level cpu and
/// memory), so a case may place it under either launch type.
pub(crate) async fn register_fargate_definition(
    router: &MultiServiceRouter,
    family: &str,
    case: &str,
) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [
                crate::task_lifecycle::container("web", "public.ecr.aws/nginx/nginx:latest"),
            ],
        }),
        case,
    )
    .await;
}

/// Create a service whose tasks come from a TASK SET: the `EXTERNAL` deployment
/// controller, which the `CreateTaskSet` doc names ("This is used when a service uses the
/// EXTERNAL deployment controller type").
pub(crate) async fn create_external_service(
    router: &MultiServiceRouter,
    cluster: &str,
    name: &str,
    desired: i64,
    case: &str,
) -> Value {
    let created = ok(
        router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": name,
            "desiredCount": desired,
            "deploymentController": { "type": "EXTERNAL" },
        }),
        case,
    )
    .await;
    created["service"].clone()
}

/// `DescribeTaskSets` for one service, optionally narrowed to `references`.
pub(crate) async fn describe_task_sets(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    references: Option<Vec<String>>,
    include_tags: bool,
    case: &str,
) -> Value {
    let mut request = json!({ "cluster": cluster, "service": service_name });
    if let Some(references) = references {
        request["taskSets"] = json!(references);
    }
    if include_tags {
        request["include"] = json!(["TAGS"]);
    }
    ok(router, "DescribeTaskSets", request, case).await
}

/// `Scale$value` of a task set, with the modelled default of `0` for a member the wire
/// omits because it equals that default.
fn scale_value(task_set: &Value, case: &str) -> f64 {
    match &task_set["scale"]["value"] {
        Value::Null => 0.0,
        value => value
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a numeric scale.value, got {value}")),
    }
}

/// The `message` of every event a service reports, newest first.
fn event_messages(service_view: &Value, case: &str) -> Vec<String> {
    list(service_view, "events", case)
        .iter()
        .map(|event| string(event, "message", case))
        .collect()
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

/// One row of the scale table.
#[derive(Debug, Clone, Copy)]
struct ScaleCase {
    name: &'static str,
    value: f64,
    /// The refusal message, or `None` when the percentage is inside the accepted range.
    refused_with: Option<&'static str>,
}

#[tokio::test]
async fn create_task_set_accepts_only_a_percent_scale_in_the_documented_range() {
    let router = service();
    let cluster = "scaled";
    create_cluster(&router, cluster, "setup").await;
    register_capacity(&router, cluster, "setup").await;
    register_definition(&router, "scaledfamily", "setup").await;

    // The rule generalises over the percentage, so the rows span the accepted range and
    // both of its published bounds, one step past each bound, and a value far outside.
    // The two sentences are the ones Amazon ECS words each side of the range with.
    let cases = [
        ScaleCase {
            name: "the lower bound",
            value: 0.0,
            refused_with: None,
        },
        ScaleCase {
            name: "a fraction above the lower bound",
            value: 0.5,
            refused_with: None,
        },
        ScaleCase {
            name: "half the service",
            value: 50.0,
            refused_with: None,
        },
        ScaleCase {
            name: "a fraction below the upper bound",
            value: 99.5,
            refused_with: None,
        },
        ScaleCase {
            name: "the upper bound",
            value: 100.0,
            refused_with: None,
        },
        ScaleCase {
            name: "a fraction past the upper bound",
            value: 100.5,
            refused_with: Some("TaskSet Scale value for percent can not be greater than 100.0"),
        },
        ScaleCase {
            name: "far past the upper bound",
            value: 150.0,
            refused_with: Some("TaskSet Scale value for percent can not be greater than 100.0"),
        },
        ScaleCase {
            name: "a fraction below the lower bound",
            value: -0.5,
            refused_with: Some("TaskSet Scale value for percent can not be less than 0.0"),
        },
        ScaleCase {
            name: "far below the lower bound",
            value: -100.0,
            refused_with: Some("TaskSet Scale value for percent can not be less than 0.0"),
        },
    ];

    for (index, test) in cases.iter().enumerate() {
        let case = test.name;
        // One service per row, so the rows cannot interfere through the per-service task
        // set quota or through each other's stored sets.
        let service_name = format!("ext{index}");
        create_external_service(&router, cluster, &service_name, 4, case).await;
        let request = json!({
            "cluster": cluster,
            "service": service_name,
            "taskDefinition": "scaledfamily",
            "launchType": "EC2",
            "scale": { "value": test.value, "unit": "PERCENT" },
        });

        match test.refused_with {
            Some(expected) => {
                let (code, message) =
                    refusal(&router, "CreateTaskSet", request.clone(), case).await;
                assert_eq!(
                    code, "InvalidParameterException",
                    "case {case:?}: expected an InvalidParameterException for scale.value \
                     {}, got {code:?} with {message:?}",
                    test.value
                );
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected the documented out-of-range sentence, got \
                     {message:?}"
                );
                // "AWS rejects it and creates no task set": nothing was stored.
                let read =
                    describe_task_sets(&router, cluster, &service_name, None, false, case).await;
                assert!(
                    list(&read, "taskSets", case).is_empty(),
                    "case {case:?}: a refused scale must create no task set, got {read}"
                );
            }
            None => {
                let created = ok(&router, "CreateTaskSet", request, case).await;
                let created = &created["taskSet"];
                let id = string(created, "id", case);
                assert!(
                    id.starts_with("ecs-svc/"),
                    "case {case:?}: expected an ecs-svc/ task set id, got {created}"
                );
                assert_eq!(
                    string(created, "taskSetArn", case),
                    format!(
                        "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-set/{cluster}/{service_name}/{id}"
                    ),
                    "case {case:?}: unexpected taskSetArn in {created}"
                );
                assert_eq!(
                    string(created, "status", case),
                    "ACTIVE",
                    "case {case:?}: a created task set is not serving production traffic \
                     yet, got {created}"
                );

                // ROUND TRIP: the stored task set reads back with the scale it was
                // created with, through the operation a client reads it with.
                let read =
                    describe_task_sets(&router, cluster, &service_name, None, false, case).await;
                let task_sets = list(&read, "taskSets", case);
                assert_eq!(
                    task_sets.len(),
                    1,
                    "case {case:?}: expected exactly the created task set back, got {read}"
                );
                let stored = &task_sets[0];
                assert_eq!(
                    string(stored, "id", case),
                    id,
                    "case {case:?}: describe must report the created task set, got {read}"
                );
                let actual = scale_value(stored, case);
                assert!(
                    (actual - test.value).abs() < 1e-9,
                    "case {case:?}: expected scale.value {}, got {actual}",
                    test.value
                );
                assert_eq!(
                    string(&stored["scale"], "unit", case),
                    "PERCENT",
                    "case {case:?}: expected scale.unit PERCENT, got {stored}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_task_sets_counts_follow_the_service_count_its_scale_and_the_tasks_it_placed() {
    let router = service();
    let cluster = "computed";
    create_cluster(&router, cluster, "setup").await;
    register_capacity(&router, cluster, "setup").await;
    register_definition(&router, "computedfamily", "setup").await;

    // "This is calculated by multiplying the service's desiredCount by the task set's
    // scale percentage. The result is always rounded up. For example, if the computed
    // desired count is 1.2, it rounds up to 2 tasks." The rows span the service count,
    // the percentage, and both the exact and the rounded-up product.
    let cases: [(&str, i64, f64, i64); 6] = [
        ("a service that wants nothing", 0, 100.0, 0),
        ("the whole service", 1, 100.0, 1),
        ("half of an odd count rounds up", 3, 50.0, 2),
        ("a quarter of four is exact", 4, 25.0, 1),
        ("no share of the service", 2, 0.0, 0),
        ("the documented 1.2 rounds up to 2", 10, 12.0, 2),
    ];

    let mut settling: Vec<(&str, String, i64)> = Vec::new();
    for (index, (case, desired, scale, expected)) in cases.into_iter().enumerate() {
        let service_name = format!("svc{index}");
        create_external_service(&router, cluster, &service_name, desired, case).await;
        let created = ok(
            &router,
            "CreateTaskSet",
            json!({
                "cluster": cluster,
                "service": service_name,
                "taskDefinition": "computedfamily",
                "launchType": "EC2",
                "scale": { "value": scale, "unit": "PERCENT" },
            }),
            case,
        )
        .await;
        let created = &created["taskSet"];
        assert_eq!(
            count(created, "computedDesiredCount", case),
            expected,
            "case {case:?}: expected computedDesiredCount {expected}, got {created}"
        );
        // The scheduler has just placed the task set, so every task it wants is PENDING
        // and none is RUNNING yet: the clock, not the create, starts them.
        assert_eq!(
            count(created, "pendingCount", case),
            expected,
            "case {case:?}: expected pendingCount {expected}, got {created}"
        );
        assert_eq!(
            count(created, "runningCount", case),
            0,
            "case {case:?}: expected runningCount 0 on a just-created task set, got {created}"
        );
        // A task set the create has just placed has not converged on what it wants: the
        // scheduler confirms its steady state a span later, whatever its counts already
        // read.
        assert_eq!(
            string(created, "stabilityStatus", case),
            "STABILIZING",
            "case {case:?}: a task set the scheduler has not confirmed stable yet is \
             STABILIZING, got {created}"
        );

        // The service the task set belongs to reports it, and counts its tasks.
        let read = describe_service(&router, cluster, &service_name, case).await;
        let reported = list(&read, "taskSets", case);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: a service whose tasks come from a task set reports it, got \
             {read}"
        );
        assert_eq!(
            string(&reported[0], "id", case),
            string(created, "id", case),
            "case {case:?}: the service must report the task set it holds, got {read}"
        );
        assert_eq!(
            count(&read, "pendingCount", case),
            expected,
            "case {case:?}: the service counts the tasks its task set placed, got {read}"
        );

        settling.push((case, service_name, expected));
    }

    // The CLOCK, not a reader, is what starts the tasks the task sets placed.
    tokio::time::sleep(ONE_SPAN).await;
    for (case, service_name, expected) in settling {
        let read = describe_task_sets(&router, cluster, &service_name, None, false, case).await;
        let stored = &list(&read, "taskSets", case)[0];
        assert_eq!(
            count(stored, "runningCount", case),
            expected,
            "case {case:?}: expected every placed task RUNNING after one span, got {read}"
        );
        assert_eq!(
            count(stored, "pendingCount", case),
            0,
            "case {case:?}: expected no PENDING task after one span, got {read}"
        );
        assert_eq!(
            string(stored, "stabilityStatus", case),
            "STEADY_STATE",
            "case {case:?}: a task set at its computed count has reached its steady \
             state, got {read}"
        );
    }
}

#[tokio::test]
async fn describe_task_sets_narrows_to_the_references_it_names() {
    let router = service();
    let cluster = "described";
    let service_name = "web";
    let case = "three task sets of one service";
    create_cluster(&router, cluster, case).await;
    register_capacity(&router, cluster, case).await;
    register_definition(&router, "describedfamily", case).await;
    create_external_service(&router, cluster, service_name, 2, case).await;

    let mut ids = Vec::new();
    let mut arns = Vec::new();
    for external in ["blue", "green", "canary"] {
        let created = ok(
            &router,
            "CreateTaskSet",
            json!({
                "cluster": cluster,
                "service": service_name,
                "taskDefinition": "describedfamily",
                "launchType": "EC2",
                "externalId": external,
                "scale": { "value": 50, "unit": "PERCENT" },
                "tags": [{ "key": "colour", "value": external }],
            }),
            case,
        )
        .await;
        ids.push(string(&created["taskSet"], "id", case));
        arns.push(string(&created["taskSet"], "taskSetArn", case));
    }

    // "The ID or full Amazon Resource Name (ARN) of task sets to describe" is optional,
    // so a request that names none describes every task set the service holds — oldest
    // first, the order they were created in.
    let all = describe_task_sets(&router, cluster, service_name, None, false, case).await;
    let reported: Vec<String> = list(&all, "taskSets", case)
        .iter()
        .map(|task_set| string(task_set, "id", case))
        .collect();
    assert_eq!(
        reported, ids,
        "case {case:?}: expected every task set oldest first, got {all}"
    );

    // Each reference form names the same task set, and a name repeated names it once.
    for (form, reference) in [("an id", ids[1].clone()), ("an ARN", arns[1].clone())] {
        let read = describe_task_sets(
            &router,
            cluster,
            service_name,
            Some(vec![reference.clone(), reference.clone()]),
            true,
            form,
        )
        .await;
        let task_sets = list(&read, "taskSets", form);
        assert_eq!(
            task_sets.len(),
            1,
            "case {form:?}: a reference repeated names one task set, got {read}"
        );
        assert_eq!(
            string(&task_sets[0], "id", form),
            ids[1],
            "case {form:?}: expected the task set {reference:?} names, got {read}"
        );
        assert_eq!(
            string(&task_sets[0], "externalId", form),
            "green",
            "case {form:?}: expected the stored externalId back, got {read}"
        );
        assert_eq!(
            list(&task_sets[0], "tags", form),
            vec![json!({ "key": "colour", "value": "green" })],
            "case {form:?}: expected the stored tags when include names TAGS, got {read}"
        );
    }

    // `include` gates the CONTENT of the tags list, exactly as it does on a service: the
    // member itself is reported either way, as the recorded task sets of the real service
    // carry it.
    let without_tags = describe_task_sets(
        &router,
        cluster,
        service_name,
        Some(vec![ids[0].clone()]),
        false,
        case,
    )
    .await;
    assert!(
        list(&list(&without_tags, "taskSets", case)[0], "tags", case).is_empty(),
        "case {case:?}: tags are reported only when include names TAGS, got {without_tags}"
    );

    // A reference that names no task set of this service is a MISSING failure rather
    // than a refusal of the whole batch — in either reference form, and the failure names
    // that task set by the id the reference carried, alone under the resource type, since
    // a task set that resolves to nothing exists in no service to qualify it by
    // ([`crate::task_set_reports`] is the table of reference forms).
    let unknown = "ecs-svc/0000000000000009999";
    let unknown_arn = format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-set/{unknown}");
    let qualified_arn = format!(
        "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-set/{cluster}/{service_name}/{unknown}"
    );
    for (form, reference) in [
        ("an unknown id", unknown.to_string()),
        ("an unknown ARN", qualified_arn.clone()),
    ] {
        let mixed = describe_task_sets(
            &router,
            cluster,
            service_name,
            Some(vec![ids[0].clone(), reference]),
            false,
            form,
        )
        .await;
        assert_eq!(
            list(&mixed, "taskSets", form).len(),
            1,
            "case {form:?}: the task set that exists is still reported, got {mixed}"
        );
        assert_eq!(
            list(&mixed, "failures", form),
            vec![json!({ "arn": unknown_arn, "reason": "MISSING" })],
            "case {form:?}: unexpected failures in {mixed}"
        );
    }

    // The reference list a single describe may carry is bounded.
    let too_many: Vec<String> = (0..101).map(|index| format!("ecs-svc/{index:019}")).collect();
    let (code, message) = refusal(
        &router,
        "DescribeTaskSets",
        json!({ "cluster": cluster, "service": service_name, "taskSets": too_many }),
        "more than a hundred references in one describe",
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "expected the describe-batch refusal, got {code:?} with {message:?}"
    );

    // "For information about the maximum number of task sets and other quotas": the
    // service holds at most five, so the sixth is refused.
    for extra in ["four", "five"] {
        ok(
            &router,
            "CreateTaskSet",
            json!({
                "cluster": cluster,
                "service": service_name,
                "taskDefinition": "describedfamily",
                "launchType": "EC2",
                "externalId": extra,
                "scale": { "value": 0, "unit": "PERCENT" },
            }),
            case,
        )
        .await;
    }
    let (code, message) = refusal(
        &router,
        "CreateTaskSet",
        json!({
            "cluster": cluster,
            "service": service_name,
            "taskDefinition": "describedfamily",
            "launchType": "EC2",
            "scale": { "value": 0, "unit": "PERCENT" },
        }),
        "a sixth task set",
    )
    .await;
    assert_eq!(
        code, "LimitExceededException",
        "expected the task-set quota refusal, got {code:?} with {message:?}"
    );
}

#[tokio::test]
async fn a_create_task_set_repeated_with_the_same_client_token_is_the_same_task_set() {
    let router = service();
    let cluster = "idempotenttaskset";
    let service_name = "web";
    let case = "the same clientToken twice";
    create_cluster(&router, cluster, case).await;
    register_capacity(&router, cluster, case).await;
    register_definition(&router, "idempotentfamily", case).await;
    create_external_service(&router, cluster, service_name, 2, case).await;

    let request = json!({
        "cluster": cluster,
        "service": service_name,
        "taskDefinition": "idempotentfamily",
        "launchType": "EC2",
        "scale": { "value": 100, "unit": "PERCENT" },
        "clientToken": "token-0001",
    });
    let first = ok(&router, "CreateTaskSet", request.clone(), case).await;
    let second = ok(&router, "CreateTaskSet", request, case).await;
    assert_eq!(
        string(&first["taskSet"], "id", case),
        string(&second["taskSet"], "id", case),
        "case {case:?}: the repeat must report the same task set"
    );
    let read = describe_task_sets(&router, cluster, service_name, None, false, case).await;
    assert_eq!(
        list(&read, "taskSets", case).len(),
        1,
        "case {case:?}: the repeat must not mint a second task set, got {read}"
    );
    let service_view = describe_service(&router, cluster, service_name, case).await;
    assert_eq!(
        count(&service_view, "pendingCount", case),
        2,
        "case {case:?}: the repeat must not place a second task set, got {service_view}"
    );
}

/// How a placement under test asks for capacity, and whether that capacity has to be
/// registered to the cluster before Amazon ECS has anywhere to put the task.
#[derive(Debug, Clone, Copy)]
enum Capacity {
    /// A launch type whose capacity is registered to the cluster.
    Registered(&'static str),
    /// The Fargate launch type, whose capacity Amazon Web Services manages.
    Fargate,
    /// An Auto Scaling group capacity provider whose managed scaling is off, so nothing
    /// is coming for the cluster either.
    UnscaledProvider,
}

#[tokio::test]
async fn the_scheduler_records_why_it_could_not_place_a_task_and_keeps_the_desired_count() {
    let router = service();
    register_definition(&router, "bridgefamily", "setup").await;
    register_fargate_definition(&router, "fargatefamily", "setup").await;

    // The rule generalises over the KIND of capacity the placement needs and over how
    // many tasks the service wants, so the rows span both, with the Fargate row as the
    // control: its capacity is not registered to the cluster, so the same empty cluster
    // places its tasks.
    let cases: [(&str, Capacity, i64, Option<&str>); 5] = [
        (
            "an EC2 service on a cluster with no capacity",
            Capacity::Registered("EC2"),
            2,
            Some(NO_INSTANCES_REASON),
        ),
        (
            "one EC2 task on a cluster with no capacity",
            Capacity::Registered("EC2"),
            1,
            Some(NO_INSTANCES_REASON),
        ),
        (
            "an EXTERNAL service on a cluster with no capacity",
            Capacity::Registered("EXTERNAL"),
            3,
            Some(NO_INSTANCES_REASON),
        ),
        (
            "a provider that will not scale its group",
            Capacity::UnscaledProvider,
            2,
            Some("Reason: The specified capacity provider 'unscaled' contains no container instances."),
        ),
        (
            "a Fargate service needs no registered capacity",
            Capacity::Fargate,
            2,
            None,
        ),
    ];

    for (index, (case, capacity, desired, expected_reason)) in cases.into_iter().enumerate() {
        let cluster = format!("nocap{index}");
        create_cluster(&router, &cluster, case).await;
        let mut request = json!({
            "cluster": cluster,
            "serviceName": "web",
            "desiredCount": desired,
        });
        match capacity {
            Capacity::Registered(launch_type) => {
                request["taskDefinition"] = json!("bridgefamily");
                request["launchType"] = json!(launch_type);
            }
            Capacity::Fargate => {
                request["taskDefinition"] = json!("fargatefamily");
                request["launchType"] = json!("FARGATE");
                request["networkConfiguration"] = awsvpc_network_configuration();
            }
            Capacity::UnscaledProvider => {
                request["taskDefinition"] = json!("bridgefamily");
                create_auto_scaling_group(&router, "asg-unscaled", case).await;
                ok(
                    &router,
                    "CreateCapacityProvider",
                    json!({
                        "name": "unscaled",
                        "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-unscaled" },
                    }),
                    case,
                )
                .await;
                ok(
                    &router,
                    "PutClusterCapacityProviders",
                    json!({
                        "cluster": cluster,
                        "capacityProviders": ["unscaled"],
                        "defaultCapacityProviderStrategy": [],
                    }),
                    case,
                )
                .await;
                request["capacityProviderStrategy"] =
                    json!([{ "capacityProvider": "unscaled", "weight": 1 }]);
            }
        }

        let created = ok(&router, "CreateService", request, case).await;
        let created = &created["service"];
        assert_eq!(
            string(created, "status", case),
            "ACTIVE",
            "case {case:?}: a service the scheduler cannot place is still created, got \
             {created}"
        );
        assert_eq!(
            count(created, "desiredCount", case),
            desired,
            "case {case:?}: a failed placement does not lower desiredCount, got {created}"
        );

        match expected_reason {
            Some(reason) => {
                let messages = event_messages(created, case);
                assert_eq!(
                    messages.len(),
                    1,
                    "case {case:?}: expected exactly the placement-failure event, got \
                     {messages:?}"
                );
                assert!(
                    messages[0].contains(UNABLE_TO_PLACE) && messages[0].contains(reason),
                    "case {case:?}: expected the placement-failure event to name the \
                     missing capacity, got {messages:?}"
                );
                assert_eq!(
                    count(created, "runningCount", case),
                    0,
                    "case {case:?}: nothing was placed, so nothing is running, got {created}"
                );
                assert_eq!(
                    count(created, "pendingCount", case),
                    0,
                    "case {case:?}: nothing was placed, so nothing is pending, got {created}"
                );

                // The event is DURABLE: it is still there after the span in which a
                // placed task would have started, and the counts have not moved.
                tokio::time::sleep(ONE_SPAN).await;
                let read = describe_service(&router, &cluster, "web", case).await;
                let messages = event_messages(&read, case);
                assert!(
                    messages.iter().any(|message| message.contains(UNABLE_TO_PLACE)),
                    "case {case:?}: the placement-failure event must persist, got \
                     {messages:?}"
                );
                assert_eq!(
                    (
                        count(&read, "desiredCount", case),
                        count(&read, "runningCount", case)
                    ),
                    (desired, 0),
                    "case {case:?}: the service keeps wanting what it cannot place, got \
                     {read}"
                );
                // "When a service deployment is started, it begins in an IN_PROGRESS
                // state. When the service reaches a steady state, the deployment
                // transitions to a COMPLETED state": this one never reached it.
                assert_eq!(
                    string(&list(&read, "deployments", case)[0], "rolloutState", case),
                    "IN_PROGRESS",
                    "case {case:?}: a rollout that placed nothing has not completed, got \
                     {read}"
                );

                // Asking for the same count again re-runs the scheduler and finds the
                // same unchanged condition, so the event stream is not re-stated.
                let updated = ok(
                    &router,
                    "UpdateService",
                    json!({ "cluster": cluster, "service": "web", "desiredCount": desired }),
                    case,
                )
                .await;
                let messages = event_messages(&updated["service"], case);
                assert_eq!(
                    messages.len(),
                    1,
                    "case {case:?}: an unchanged condition is recorded once, got \
                     {messages:?}"
                );

                // And the capacity arriving is what lets the scheduler place the task
                // set: the next call that changes what the service wants places it.
                register_capacity(&router, &cluster, case).await;
                if matches!(capacity, Capacity::Registered(_)) {
                    let placed = ok(
                        &router,
                        "UpdateService",
                        json!({
                            "cluster": cluster,
                            "service": "web",
                            "desiredCount": desired,
                            "forceNewDeployment": true,
                        }),
                        case,
                    )
                    .await;
                    assert_eq!(
                        count(&placed["service"], "pendingCount", case),
                        desired,
                        "case {case:?}: registered capacity lets the scheduler place the \
                         task set, got {}",
                        placed["service"]
                    );
                }
            }
            None => {
                assert!(
                    event_messages(created, case).is_empty(),
                    "case {case:?}: a placement that needs no registered capacity writes \
                     no failure event, got {created}"
                );
                assert_eq!(
                    count(created, "pendingCount", case),
                    desired,
                    "case {case:?}: expected every wanted task placed, got {created}"
                );
                // A Fargate service's tasks are `awsvpc`, so each of them is placed only
                // once the interface it was allocated is attached.
                tokio::time::sleep(crate::ecs_wire::AWSVPC_START).await;
                let read = describe_service(&router, &cluster, "web", case).await;
                assert_eq!(
                    count(&read, "runningCount", case),
                    desired,
                    "case {case:?}: expected every placed task RUNNING after one span, \
                     got {read}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_task_set_the_scheduler_cannot_place_is_reported_with_the_reason_on_its_service() {
    let router = service();
    let cluster = "tasksetnocap";
    let service_name = "web";
    let case = "a task set on a cluster with no capacity";
    create_cluster(&router, cluster, case).await;
    register_definition(&router, "tasksetfamily", case).await;
    create_external_service(&router, cluster, service_name, 2, case).await;

    let created = ok(
        &router,
        "CreateTaskSet",
        json!({
            "cluster": cluster,
            "service": service_name,
            "taskDefinition": "tasksetfamily",
            "launchType": "EC2",
            "scale": { "value": 100, "unit": "PERCENT" },
        }),
        case,
    )
    .await;
    let created = &created["taskSet"];
    assert_eq!(
        count(created, "computedDesiredCount", case),
        2,
        "case {case:?}: the task set still wants the whole service, got {created}"
    );
    assert_eq!(
        (
            count(created, "runningCount", case),
            count(created, "pendingCount", case)
        ),
        (0, 0),
        "case {case:?}: nothing was placed, so nothing is running or pending, got {created}"
    );
    assert_eq!(
        string(created, "stabilityStatus", case),
        "STABILIZING",
        "case {case:?}: a task set below its computed count is still stabilizing, got \
         {created}"
    );

    let read = describe_service(&router, cluster, service_name, case).await;
    let messages = event_messages(&read, case);
    assert!(
        messages
            .iter()
            .any(|message| message.contains(UNABLE_TO_PLACE)
                && message.contains(NO_INSTANCES_REASON)),
        "case {case:?}: the service's event stream must explain the task set's failed \
         placement, got {messages:?}"
    );
}

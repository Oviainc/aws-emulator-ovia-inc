// SPDX-License-Identifier: CC0-1.0

//! Where an accepted task STARTS in the ECS task lifecycle, driven end to end over HTTP.
//!
//! A task Amazon ECS has accepted but still has to acquire capacity for has not been
//! placed, which is what `PROVISIONING` reports; a task placed onto capacity that already
//! exists is `PENDING` and only its container start is outstanding. Every case here goes
//! through the shared wire harness in [`crate::ecs_wire`], the same front door a client
//! uses, so nothing reaches into the state struct.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, count, create_auto_scaling_group, create_cluster, list, ok,
    refusal, register_capacity, register_definition, service, string,
};

/// Long enough for one clock-owed task transition (`state::TRANSITION_SPAN` is 500 ms) to
/// fall due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// How often a lifecycle poll re-reads a task, and how long it keeps trying. The interval
/// is well inside one transition span, so every status the task passes through is seen.
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const POLL_BUDGET: Duration = Duration::from_secs(10);

/// How the run under test asks for capacity.
#[derive(Debug, Clone, Copy)]
enum Placement {
    /// A strategy naming an Auto Scaling group capacity provider created with this
    /// `managedScaling.status`; `None` leaves the member absent (the documented default,
    /// "the default is off").
    AutoScalingProvider(Option<&'static str>),
    /// A strategy naming one of the predefined Fargate providers, which names no Auto
    /// Scaling group of this account, so no container instance is ever acquired for it.
    FargateProvider,
    /// `launchType=EC2`: the capacity is whatever is already registered to the cluster.
    Ec2LaunchType,
}

/// One row of the initial-status table: how the run asks for capacity, whether the cluster
/// already holds a container instance, how many tasks it asks for, and the `lastStatus`
/// every accepted task must carry (`None` where the run must be refused instead).
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    placement: Placement,
    capacity_registered: bool,
    count: i64,
    expected: Option<&'static str>,
}

/// Read ONE task back through `DescribeTasks` — the stored record, not the response of the
/// call that created it.
async fn describe_one(
    router: &MultiServiceRouter,
    cluster: &str,
    task_arn: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": [task_arn] }),
        case,
    )
    .await;
    assert!(
        list(&body, "failures", case).is_empty(),
        "case {case:?}: expected DescribeTasks to find the task, got {body}"
    );
    list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected one task back, got {body}"))
}

/// Every `lastStatus` one task is observed in, in the order it takes them, until it reaches
/// `RUNNING` or the poll budget runs out. `first` seeds the sequence with the status the
/// caller has already observed.
async fn statuses_until_running(
    router: &MultiServiceRouter,
    cluster: &str,
    task_arn: &str,
    first: &str,
    case: &str,
) -> Vec<String> {
    let mut seen = vec![first.to_string()];
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let task = describe_one(router, cluster, task_arn, case).await;
        let status = string(&task, "lastStatus", case);
        if seen.last() != Some(&status) {
            seen.push(status.clone());
        }
        if status == "RUNNING" || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create an Auto Scaling group capacity provider named `provider` with the given
/// `managedScaling.status` (absent when `None`) and associate it with `cluster`.
async fn provider_on(
    router: &MultiServiceRouter,
    cluster: &str,
    provider: &str,
    managed_scaling: Option<&str>,
    case: &str,
) {
    let group_name = format!("asg-{provider}");
    create_auto_scaling_group(router, &group_name, case).await;
    let mut group = json!({ "autoScalingGroupArn": group_name });
    if let Some(status) = managed_scaling {
        group["managedScaling"] = json!({ "status": status, "targetCapacity": 100 });
    }
    ok(
        router,
        "CreateCapacityProvider",
        json!({ "name": provider, "autoScalingGroupProvider": group }),
        case,
    )
    .await;
    ok(
        router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": cluster,
            "capacityProviders": [provider],
            "defaultCapacityProviderStrategy": [],
        }),
        case,
    )
    .await;
}

#[tokio::test]
async fn run_task_starts_a_task_in_provisioning_only_while_its_capacity_is_still_being_acquired() {
    let router = service();
    let family = "parked";
    register_definition(&router, family, "setup").await;
    // A revision is placed only on infrastructure it was validated against, and the Fargate
    // row below places through a Fargate provider: that needs a revision whose parameters
    // support Fargate (the `awsvpc` network mode with a task-level size).
    let fargate_family = "parkedfargate";
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": fargate_family,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "requiresCompatibilities": ["FARGATE"],
            "containerDefinitions": [
                crate::task_lifecycle::container("web", "public.ecr.aws/nginx/nginx:latest"),
            ],
        }),
        "setup",
    )
    .await;

    // The rows differ along everything the rule generalises over: whether the capacity the
    // placement needs has to be ACQUIRED (the provider's managed scaling, across the whole
    // `ManagedScalingStatus` enum plus the documented default of off), whether the cluster
    // already holds a container instance, the KIND of capacity the run asks for (an Auto
    // Scaling group provider, a predefined Fargate provider, a launch type), and how many
    // tasks one call places.
    let rows = [
        Row {
            case: "managed scaling ENABLED, empty cluster",
            placement: Placement::AutoScalingProvider(Some("ENABLED")),
            capacity_registered: false,
            count: 1,
            expected: Some("PROVISIONING"),
        },
        Row {
            case: "managed scaling ENABLED, empty cluster, three tasks at once",
            placement: Placement::AutoScalingProvider(Some("ENABLED")),
            capacity_registered: false,
            count: 3,
            expected: Some("PROVISIONING"),
        },
        Row {
            case: "managed scaling ENABLED, capacity already registered",
            placement: Placement::AutoScalingProvider(Some("ENABLED")),
            capacity_registered: true,
            count: 2,
            expected: Some("PENDING"),
        },
        Row {
            case: "managed scaling DISABLED, capacity already registered",
            placement: Placement::AutoScalingProvider(Some("DISABLED")),
            capacity_registered: true,
            count: 1,
            expected: Some("PENDING"),
        },
        Row {
            case: "managed scaling DISABLED, empty cluster",
            placement: Placement::AutoScalingProvider(Some("DISABLED")),
            capacity_registered: false,
            count: 1,
            expected: None,
        },
        Row {
            case: "no managed scaling member at all, empty cluster",
            placement: Placement::AutoScalingProvider(None),
            capacity_registered: false,
            count: 1,
            expected: None,
        },
        Row {
            // A Fargate placement acquires no container instance, so nothing of the
            // CLUSTER's capacity is outstanding for it. What such a task does wait on is
            // its own elastic network interface, which the `awsvpc` mode Fargate requires
            // gives it: that is the step `PROVISIONING` reports here.
            case: "a Fargate provider acquires no container instance",
            placement: Placement::FargateProvider,
            capacity_registered: false,
            count: 1,
            expected: Some("PROVISIONING"),
        },
        Row {
            case: "EC2 launch type onto registered capacity",
            placement: Placement::Ec2LaunchType,
            capacity_registered: true,
            count: 1,
            expected: Some("PENDING"),
        },
        Row {
            case: "EC2 launch type on an instance-less cluster",
            placement: Placement::Ec2LaunchType,
            capacity_registered: false,
            count: 1,
            expected: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // Each row gets its OWN cluster, so the capacity one row registers is never the
        // capacity another row's placement finds.
        let cluster = format!("parked{index}");
        create_cluster(&router, &cluster, case).await;
        if row.capacity_registered {
            register_capacity(&router, &cluster, case).await;
        }

        let mut request = json!({
            "cluster": cluster,
            "taskDefinition": family,
            "count": row.count,
        });
        let expected_provider = match row.placement {
            Placement::AutoScalingProvider(managed_scaling) => {
                let provider = format!("provider{index}");
                provider_on(&router, &cluster, &provider, managed_scaling, case).await;
                request["capacityProviderStrategy"] =
                    json!([{ "capacityProvider": provider, "weight": 1 }]);
                Some(provider)
            }
            Placement::FargateProvider => {
                // A cluster uses the providers it HOLDS, so the Fargate provider this row
                // places on is associated with its cluster first.
                ok(
                    &router,
                    "PutClusterCapacityProviders",
                    json!({
                        "cluster": cluster,
                        "capacityProviders": ["FARGATE"],
                        "defaultCapacityProviderStrategy": [],
                    }),
                    case,
                )
                .await;
                request["capacityProviderStrategy"] =
                    json!([{ "capacityProvider": "FARGATE", "weight": 1 }]);
                request["taskDefinition"] = json!(fargate_family);
                request["networkConfiguration"] = awsvpc_network_configuration();
                Some("FARGATE".to_string())
            }
            Placement::Ec2LaunchType => {
                request["launchType"] = json!("EC2");
                None
            }
        };

        let Some(expected_status) = row.expected else {
            // A placement nothing can satisfy is refused, and leaves no task behind: there
            // is no lifecycle state for a task that was never accepted.
            let (code, message) = refusal(&router, "RunTask", request, case).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected an InvalidParameterException, got {code:?} with \
                 {message:?}"
            );
            let listed = ok(&router, "ListTasks", json!({ "cluster": cluster }), case).await;
            assert!(
                list(&listed, "taskArns", case).is_empty(),
                "case {case:?}: a refused RunTask must run no task, got {listed}"
            );
            continue;
        };

        let body = ok(&router, "RunTask", request, case).await;
        assert!(
            list(&body, "failures", case).is_empty(),
            "case {case:?}: expected no failures, got {body}"
        );
        let tasks = list(&body, "tasks", case);
        assert_eq!(
            i64::try_from(tasks.len()).unwrap_or(-1),
            row.count,
            "case {case:?}: expected {} task(s), got {body}",
            row.count
        );

        for task in &tasks {
            assert_eq!(
                string(task, "lastStatus", case),
                expected_status,
                "case {case:?}: expected lastStatus {expected_status:?}, got {task}"
            );
            // Neither status is a task that has started, so neither carries a start time.
            assert!(
                task["startedAt"].is_null(),
                "case {case:?}: a task in {expected_status:?} has not started, got {task}"
            );
            for container in list(task, "containers", case) {
                assert_eq!(
                    string(&container, "lastStatus", case),
                    expected_status,
                    "case {case:?}: expected the container to follow the task into \
                     {expected_status:?}, got {container}"
                );
            }
            match &expected_provider {
                Some(provider) => assert_eq!(
                    string(task, "capacityProviderName", case),
                    *provider,
                    "case {case:?}: the task must name the provider it was placed through, \
                     got {task}"
                ),
                None => assert_eq!(
                    string(task, "launchType", case),
                    "EC2",
                    "case {case:?}: the task must carry the launch type it asked for, got \
                     {task}"
                ),
            }
        }

        // The status is STORED, not made up for the response of the call that placed it.
        let arns: Vec<String> = tasks
            .iter()
            .map(|task| string(task, "taskArn", case))
            .collect();
        let described = ok(
            &router,
            "DescribeTasks",
            json!({ "cluster": cluster, "tasks": arns }),
            case,
        )
        .await;
        let read = list(&described, "tasks", case);
        assert_eq!(
            i64::try_from(read.len()).unwrap_or(-1),
            row.count,
            "case {case:?}: expected DescribeTasks to find every task, got {described}"
        );
        for task in &read {
            assert_eq!(
                string(task, "lastStatus", case),
                expected_status,
                "case {case:?}: expected the stored lastStatus {expected_status:?}, got {task}"
            );
        }
    }
}

#[tokio::test]
async fn a_parked_task_is_placed_only_once_the_capacity_it_waits_for_registers() {
    let router = service();
    let case = "a task parked on an empty managed-scaling provider";
    let cluster = "awaitingcapacity";
    let family = "awaiting";
    create_cluster(&router, cluster, case).await;
    register_definition(&router, family, case).await;
    provider_on(&router, cluster, "scaled", Some("ENABLED"), case).await;

    let body = ok(
        &router,
        "RunTask",
        json!({
            "cluster": cluster,
            "taskDefinition": family,
            "capacityProviderStrategy": [{ "capacityProvider": "scaled", "weight": 1 }],
        }),
        case,
    )
    .await;
    let task = list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected one task, got {body}"));
    let task_arn = string(&task, "taskArn", case);
    assert_eq!(
        string(&task, "lastStatus", case),
        "PROVISIONING",
        "case {case:?}: expected the parked task in PROVISIONING, got {task}"
    );

    // The parked task EXISTS: it was accepted, so it is listed and described like any
    // other task of the cluster.
    let listed = ok(&router, "ListTasks", json!({ "cluster": cluster }), case).await;
    assert_eq!(
        list(&listed, "taskArns", case),
        vec![Value::from(task_arn.clone())],
        "case {case:?}: expected the parked task to be listed, got {listed}"
    );

    // Time alone does not place it: nothing has capacity to place it ON, so no start is
    // owed and the task stays where it is.
    tokio::time::sleep(2 * ONE_SPAN).await;
    let read = describe_one(&router, cluster, &task_arn, case).await;
    assert_eq!(
        string(&read, "lastStatus", case),
        "PROVISIONING",
        "case {case:?}: expected the task to stay parked while its cluster holds no \
         container instance, got {read}"
    );
    assert!(
        read["startedAt"].is_null(),
        "case {case:?}: a parked task has not started, got {read}"
    );
    let described = ok(
        &router,
        "DescribeClusters",
        json!({ "clusters": [cluster] }),
        case,
    )
    .await;
    let view = &list(&described, "clusters", case)[0];
    assert_eq!(
        count(view, "runningTasksCount", case),
        0,
        "case {case:?}: a parked task is not running, got {view}"
    );

    // The capacity the provider had to acquire arrives.
    register_capacity(&router, cluster, case).await;
    let seen = statuses_until_running(&router, cluster, &task_arn, "PROVISIONING", case).await;
    assert_eq!(
        seen,
        vec!["PROVISIONING", "PENDING", "RUNNING"],
        "case {case:?}: expected the parked task to be placed and then started, observed \
         {seen:?}"
    );

    let read = describe_one(&router, cluster, &task_arn, case).await;
    assert!(
        !read["startedAt"].is_null(),
        "case {case:?}: a running task carries a start time, got {read}"
    );
    let described = ok(
        &router,
        "DescribeClusters",
        json!({ "clusters": [cluster] }),
        case,
    )
    .await;
    let view = &list(&described, "clusters", case)[0];
    assert_eq!(
        count(view, "runningTasksCount", case),
        1,
        "case {case:?}: expected the placed task to count as running, got {view}"
    );
}

#[tokio::test]
async fn a_task_parked_after_its_cluster_lost_its_capacity_is_placed_when_the_instance_returns() {
    let router = service();
    let case = "capacity that comes back through a re-registration";
    let cluster = "returningcapacity";
    let family = "returning";
    create_cluster(&router, cluster, case).await;
    register_definition(&router, family, case).await;
    provider_on(&router, cluster, "returnscaled", Some("ENABLED"), case).await;

    // Capacity that EXISTED and then went away: a deregistered instance is still stored,
    // but it no longer counts towards the cluster's registered capacity, so the next
    // placement has nothing to run on.
    let instance_arn = register_capacity(&router, cluster, case).await;
    ok(
        &router,
        "DeregisterContainerInstance",
        json!({ "cluster": cluster, "containerInstance": instance_arn }),
        case,
    )
    .await;

    let body = ok(
        &router,
        "RunTask",
        json!({
            "cluster": cluster,
            "taskDefinition": family,
            "capacityProviderStrategy": [{ "capacityProvider": "returnscaled", "weight": 1 }],
        }),
        case,
    )
    .await;
    let task = list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected one task, got {body}"));
    let task_arn = string(&task, "taskArn", case);
    assert_eq!(
        string(&task, "lastStatus", case),
        "PROVISIONING",
        "case {case:?}: expected the task to be parked on the empty provider, got {task}"
    );

    // The SAME instance registers again, which is the other way this cluster's capacity
    // appears: the parked task is placed from there just as it is for a first
    // registration.
    register_capacity(&router, cluster, case).await;
    let seen = statuses_until_running(&router, cluster, &task_arn, "PROVISIONING", case).await;
    assert_eq!(
        seen,
        vec!["PROVISIONING", "PENDING", "RUNNING"],
        "case {case:?}: expected the returning capacity to place and start the parked task, \
         observed {seen:?}"
    );
}

#[tokio::test]
async fn a_service_task_is_parked_too_while_its_provider_must_acquire_capacity() {
    let router = service();
    let case = "a service placed through an empty managed-scaling provider";
    let cluster = "servicecapacity";
    let family = "serviceawaiting";
    create_cluster(&router, cluster, case).await;
    register_definition(&router, family, case).await;
    provider_on(&router, cluster, "servicescaled", Some("ENABLED"), case).await;

    let created = ok(
        &router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": "parkedservice",
            "taskDefinition": family,
            "desiredCount": 2,
            "capacityProviderStrategy": [
                { "capacityProvider": "servicescaled", "weight": 1 },
            ],
        }),
        case,
    )
    .await;
    let view = &created["service"];
    assert_eq!(
        count(view, "runningCount", case),
        0,
        "case {case:?}: no task of the service can be running yet, got {view}"
    );

    // The scheduler's task set is parked the same way `RunTask`'s is: the tasks exist, but
    // none of them has been placed.
    let listed = ok(&router, "ListTasks", json!({ "cluster": cluster }), case).await;
    let arns = list(&listed, "taskArns", case);
    assert_eq!(
        arns.len(),
        2,
        "case {case:?}: expected the service's task set to exist, got {listed}"
    );
    let arns: Vec<String> = arns
        .iter()
        .map(|arn| {
            arn.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a task ARN, got {arn}"))
                .to_string()
        })
        .collect();
    for arn in &arns {
        let read = describe_one(&router, cluster, arn, case).await;
        assert_eq!(
            string(&read, "lastStatus", case),
            "PROVISIONING",
            "case {case:?}: expected the service's task to be parked, got {read}"
        );
    }

    // Once the capacity exists, the same clock places and starts them.
    register_capacity(&router, cluster, case).await;
    for arn in &arns {
        let seen = statuses_until_running(&router, cluster, arn, "PROVISIONING", case).await;
        assert_eq!(
            seen.last().map(String::as_str),
            Some("RUNNING"),
            "case {case:?}: expected the service's task to run once capacity exists, \
             observed {seen:?}"
        );
    }
    let described = ok(
        &router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": ["parkedservice"] }),
        case,
    )
    .await;
    let view = &list(&described, "services", case)[0];
    assert_eq!(
        count(view, "runningCount", case),
        2,
        "case {case:?}: expected both placed tasks to count as running, got {view}"
    );
}

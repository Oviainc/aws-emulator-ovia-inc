// SPDX-License-Identifier: CC0-1.0

//! Where a task may be placed, and what the cluster then reports, driven end to end
//! over HTTP.
//!
//! The rules under test:
//!
//! 1. `Cluster$statistics` carries the four launch-type counters — running and pending
//!    tasks, active and draining services — for EVERY launch type the model's
//!    `LaunchType` enum defines, and each counter is COUNTED off the cluster's own
//!    records rather than fabricated.
//! 2. A task definition is placed with a launch type only where it was validated
//!    against it (`TaskDefinition$compatibilities`); any other launch type is refused
//!    with the modelled `InvalidParameterException`.
//! 3. A `platformVersion` that names no platform version this service knows is refused
//!    with the modelled `PlatformUnknownException`, whichever operation carries it.
//! 4. A task placed with a launch type whose capacity is REGISTERED to the cluster
//!    needs a container instance of that cluster to run on; a cluster with none refuses
//!    the placement instead of persisting a task that could never run.
//! 5. Deleting a cluster releases what it held, container instances included, so the
//!    deleted cluster reports no registered capacity and a cluster created over the
//!    tombstone starts empty.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same
//! front door a client uses, so nothing reaches into the state struct and a response
//! that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, cluster_with_capacity, count, create_cluster, list, ok, refusal,
    refused, register_capacity, service, string,
};
use crate::task_lifecycle::container;

/// Every launch type the model defines, paired with the segment its `Cluster$statistics`
/// counter names carry. Catalogue source: `smithy_query(query="shape",
/// shape="LaunchType")` -> `["EC2", "FARGATE", "EXTERNAL", "MANAGED_INSTANCES"]`.
const LAUNCH_TYPES: [(&str, &str); 4] = [
    ("EC2", "EC2"),
    ("FARGATE", "Fargate"),
    ("EXTERNAL", "External"),
    ("MANAGED_INSTANCES", "ManagedInstances"),
];

/// The platform versions AWS Fargate PUBLISHES, from the `platformVersion` doc's "Fargate
/// platform versions" page: the newest Linux one (`1.4.0`) and the newest Windows one
/// (`1.0.0`), plus the `LATEST` alias the member's own doc names. The earlier Linux
/// versions (`1.3.0` and below) are retired, so they are rows of the REFUSED table below.
const KNOWN_PLATFORM_VERSIONS: [&str; 3] = ["LATEST", "1.4.0", "1.0.0"];

/// The `PlatformUnknownException` sentence (`state`'s `PLATFORM_UNKNOWN_MESSAGE`): a
/// constant of the condition, naming nothing from the request.
const PLATFORM_UNKNOWN: &str = "The specified platform does not exist.";

/// The `InvalidParameterException` sentence for a cluster with no registered capacity
/// (`state`'s `NO_CONTAINER_INSTANCES_MESSAGE`).
const NO_CONTAINER_INSTANCES: &str = "No Container Instances were found in your cluster.";

/// Register a revision of `family` that is Fargate-compatible: `awsvpc` ("For Amazon
/// ECS tasks on Fargate, the awsvpc network mode is required") plus task-level cpu and
/// memory ("This field is required for Fargate"). `requires` is what the registration
/// asks to be validated against (`requiresCompatibilities`), which is what brings a
/// launch type no placement can name outright into the revision's compatibilities.
async fn register_awsvpc_requiring(
    router: &MultiServiceRouter,
    family: &str,
    requires: &[&str],
    case: &str,
) {
    let mut request = json!({
        "family": family,
        "networkMode": "awsvpc",
        "cpu": "256",
        "memory": "512",
        "containerDefinitions": [container("app", "public.ecr.aws/nginx/nginx:latest")],
    });
    if !requires.is_empty() {
        request["requiresCompatibilities"] = json!(requires);
    }
    ok(router, "RegisterTaskDefinition", request, case).await;
}

/// Register a revision of `family` that is Fargate-compatible and asked to be validated
/// against nothing in particular.
async fn register_awsvpc(router: &MultiServiceRouter, family: &str, case: &str) {
    register_awsvpc_requiring(router, family, &[], case).await;
}

/// Register a revision of `family` in the default `bridge` network mode, which no
/// Fargate or managed-instances placement supports.
async fn register_bridge(router: &MultiServiceRouter, family: &str, case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [container("app", "public.ecr.aws/nginx/nginx:latest")],
        }),
        case,
    )
    .await;
}

/// The `statistics` list of one cluster, read through `DescribeClusters`.
async fn statistics(router: &MultiServiceRouter, cluster: &str, case: &str) -> Vec<Value> {
    let described = ok(
        router,
        "DescribeClusters",
        json!({ "clusters": [cluster], "include": ["STATISTICS"] }),
        case,
    )
    .await;
    let clusters = list(&described, "clusters", case);
    assert_eq!(
        clusters.len(),
        1,
        "case {case:?}: expected the cluster to be described, got {described}"
    );
    list(&clusters[0], "statistics", case)
}

/// The value of one named statistic, which must be present.
fn metric(statistics: &[Value], name: &str, case: &str) -> i64 {
    statistics
        .iter()
        .find(|entry| entry["name"] == json!(name))
        .and_then(|entry| entry["value"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} entry, got {statistics:?}"))
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a number for {name}"))
}

#[tokio::test]
async fn cluster_statistics_carry_every_launch_type_the_model_defines() {
    let case = "every launch type";
    let router = service();
    cluster_with_capacity(&router, "metrics", case).await;

    let reported = statistics(&router, "metrics", case).await;
    // One running-tasks, pending-tasks, active-services and draining-services counter
    // for every launch type, so a client keyed on a launch type's metric finds it
    // whether or not anything is placed with that launch type.
    for (launch_type, segment) in LAUNCH_TYPES {
        for name in [
            format!("running{segment}TasksCount"),
            format!("pending{segment}TasksCount"),
            format!("active{segment}ServiceCount"),
            format!("draining{segment}ServiceCount"),
        ] {
            assert_eq!(
                metric(&reported, &name, case),
                0,
                "case {case:?}: an empty cluster reports {launch_type} metric {name} as \
                 0, got {reported:?}"
            );
        }
    }
    assert_eq!(
        reported.len(),
        4 * LAUNCH_TYPES.len(),
        "case {case:?}: expected four counters per launch type and nothing else, got \
         {reported:?}"
    );
}

#[tokio::test]
async fn each_launch_types_statistics_count_only_the_tasks_placed_with_it() {
    let router = service();

    // Rows differ along the launch type the tasks are placed with and the COUNT placed,
    // so a counter that did not come from the stored tasks cannot pass. `EC2` and
    // `EXTERNAL` place on capacity registered to the cluster; `FARGATE` does not.
    let rows: [(&str, &str, &str, i64, bool); 3] = [
        ("one EC2 task", "EC2", "EC2", 1, false),
        ("three Fargate tasks", "FARGATE", "Fargate", 3, true),
        ("two external tasks", "EXTERNAL", "External", 2, false),
    ];

    for (case, launch_type, segment, placed, awsvpc) in rows {
        let cluster = format!("stats{launch_type}");
        let family = format!("fam{launch_type}");
        cluster_with_capacity(&router, &cluster, case).await;
        if awsvpc {
            register_awsvpc(&router, &family, case).await;
        } else {
            register_bridge(&router, &family, case).await;
        }
        let mut request = json!({
            "cluster": cluster,
            "taskDefinition": family,
            "launchType": launch_type,
            "count": placed,
        });
        if awsvpc {
            // An `awsvpc` revision is launched with the network configuration its
            // elastic network interface takes.
            request["networkConfiguration"] = awsvpc_network_configuration();
        }
        let run = ok(&router, "RunTask", request, case).await;
        assert_eq!(
            list(&run, "tasks", case).len() as i64,
            placed,
            "case {case:?}: expected {placed} tasks, got {run}"
        );

        let reported = statistics(&router, &cluster, case).await;
        let own = metric(&reported, &format!("running{segment}TasksCount"), case)
            + metric(&reported, &format!("pending{segment}TasksCount"), case);
        assert_eq!(
            own, placed,
            "case {case:?}: the {launch_type} task counters must account for every task \
             placed, got {reported:?}"
        );
        // No other launch type's counters moved: each one counts its own records.
        for (other, other_segment) in LAUNCH_TYPES {
            if other == launch_type {
                continue;
            }
            let counted = metric(&reported, &format!("running{other_segment}TasksCount"), case)
                + metric(&reported, &format!("pending{other_segment}TasksCount"), case);
            assert_eq!(
                counted, 0,
                "case {case:?}: no task was placed with {other}, so its counters stay 0, \
                 got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_launch_type_the_revision_was_not_validated_against_is_refused() {
    let router = service();
    let case = "compatibility";
    cluster_with_capacity(&router, "compat", case).await;
    register_bridge(&router, "bridgefam", case).await;
    register_awsvpc(&router, "awsvpcfam", case).await;
    // The same `awsvpc` shape, registered asking to be validated against managed
    // instances as well.
    register_awsvpc_requiring(
        &router,
        "managedfam",
        &["FARGATE", "MANAGED_INSTANCES"],
        case,
    )
    .await;

    // The rule generalises over the revision's parameters and the launch type asked for.
    // The expected column is the documented compatibility of each shape: EC2 supports
    // every network mode; Fargate and managed instances need `awsvpc` (plus task-level
    // cpu/memory for Fargate); `awsvpc` is not available to the external launch type;
    // and managed instances, which a placement cannot name outright, additionally needs
    // the registration to have asked to be validated against it.
    let rows: [(&str, &str, &str, bool); 9] = [
        ("bridge on EC2", "bridgefam", "EC2", true),
        ("bridge on EXTERNAL", "bridgefam", "EXTERNAL", true),
        ("bridge on FARGATE", "bridgefam", "FARGATE", false),
        (
            "bridge on MANAGED_INSTANCES",
            "bridgefam",
            "MANAGED_INSTANCES",
            false,
        ),
        ("awsvpc on EC2", "awsvpcfam", "EC2", true),
        ("awsvpc on FARGATE", "awsvpcfam", "FARGATE", true),
        (
            "awsvpc on MANAGED_INSTANCES",
            "awsvpcfam",
            "MANAGED_INSTANCES",
            false,
        ),
        (
            "awsvpc validated for managed instances",
            "managedfam",
            "MANAGED_INSTANCES",
            true,
        ),
        ("awsvpc on EXTERNAL", "awsvpcfam", "EXTERNAL", false),
    ];

    for (case, family, launch_type, supported) in rows {
        let mut request = json!({
            "cluster": "compat",
            "taskDefinition": family,
            "launchType": launch_type,
        });
        if family != "bridgefam" {
            request["networkConfiguration"] = awsvpc_network_configuration();
        }
        if supported {
            let run = ok(&router, "RunTask", request, case).await;
            assert_eq!(
                list(&run, "tasks", case).len(),
                1,
                "case {case:?}: a supported launch type places the task, got {run}"
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
            "case {case:?}: the refusal must name the launch type asked for, got \
             {message:?}"
        );
    }

    // The same rule guards the service scheduler's placements, which is the other
    // operation that is handed a `launchType`.
    let (code, message) = refusal(
        &router,
        "CreateService",
        json!({
            "cluster": "compat",
            "serviceName": "web",
            "taskDefinition": "bridgefam",
            "desiredCount": 1,
            "launchType": "FARGATE",
        }),
        "a service on an unsupported launch type",
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "expected CreateService to refuse an unsupported launch type, got {code:?} with \
         {message:?}"
    );
    assert_eq!(
        message, "Task definition does not support launch_type FARGATE.",
        "expected the same sentence from CreateService, got {message:?}"
    );
}

#[tokio::test]
async fn a_platform_version_outside_the_published_list_is_refused() {
    let router = service();
    let case = "platform versions";
    create_cluster(&router, "platform", case).await;
    register_awsvpc(&router, "platformfam", case).await;

    // Every published platform version is accepted, including the ones the issue did not
    // cite.
    for version in KNOWN_PLATFORM_VERSIONS {
        let case = format!("platformVersion {version}");
        let run = ok(
            &router,
            "RunTask",
            json!({
                "cluster": "platform",
                "taskDefinition": "platformfam",
                "launchType": "FARGATE",
                "platformVersion": version,
                "networkConfiguration": awsvpc_network_configuration(),
            }),
            &case,
        )
        .await;
        assert_eq!(
            list(&run, "tasks", &case).len(),
            1,
            "case {case:?}: a published platform version places the task, got {run}"
        );
    }

    // Rows past the list: a version that does not exist, one just past the newest, a
    // partial version, the alias in the wrong case, and the retired Linux versions — the
    // member is a string the service matches exactly against what Fargate publishes.
    for version in [
        "9.9.9", "1.5.0", "1.4", "latest", "FARGATE", "1.3.0", "1.2.0", "1.1.0",
    ] {
        let case = format!("unknown platformVersion {version}");
        let (code, message) = refusal(
            &router,
            "RunTask",
            json!({
                "cluster": "platform",
                "taskDefinition": "platformfam",
                "launchType": "FARGATE",
                "platformVersion": version,
            }),
            &case,
        )
        .await;
        assert_eq!(
            code, "PlatformUnknownException",
            "case {case:?}: expected the modelled refusal, got {code:?} with {message:?}"
        );
        assert_eq!(
            message, PLATFORM_UNKNOWN,
            "case {case:?}: expected the fixed sentence for this condition, got \
             {message:?}"
        );
    }

    // The same rule guards the other operations handed a `platformVersion`.
    for operation in ["CreateService", "UpdateService"] {
        let case = format!("{operation} with an unknown platformVersion");
        let request = match operation {
            "CreateService" => json!({
                "cluster": "platform",
                "serviceName": "web",
                "taskDefinition": "platformfam",
                "desiredCount": 1,
                "launchType": "FARGATE",
                "platformVersion": "9.9.9",
            }),
            _ => json!({
                "cluster": "platform",
                "service": "web",
                "platformVersion": "9.9.9",
            }),
        };
        let (code, message) = refusal(&router, operation, request, &case).await;
        assert_eq!(
            code, "PlatformUnknownException",
            "case {case:?}: expected the modelled refusal, got {code:?} with {message:?}"
        );
    }
}

#[tokio::test]
async fn a_cluster_with_no_registered_capacity_cannot_place_a_task_on_it() {
    let router = service();
    let setup = "no capacity";
    register_bridge(&router, "bridgefam", setup).await;
    register_awsvpc(&router, "awsvpcfam", setup).await;

    // The launch types whose capacity is registered to the cluster are refused while it
    // has none; the ones that run on capacity AWS manages are not. The default — neither
    // `launchType` nor a capacity-provider strategy — is the EC2 capacity registered to
    // the cluster, so it is refused too. Each row gets its own empty cluster.
    let rows: [(&str, Value, bool); 4] = [
        (
            "explicit EC2",
            json!({ "taskDefinition": "bridgefam", "launchType": "EC2" }),
            true,
        ),
        (
            "explicit EXTERNAL",
            json!({ "taskDefinition": "bridgefam", "launchType": "EXTERNAL" }),
            true,
        ),
        (
            "no launch type at all",
            json!({ "taskDefinition": "bridgefam" }),
            true,
        ),
        (
            "FARGATE needs no registered capacity",
            json!({
                "taskDefinition": "awsvpcfam",
                "launchType": "FARGATE",
                "networkConfiguration": awsvpc_network_configuration(),
            }),
            false,
        ),
    ];

    for (index, (case, mut request, refused_without_capacity)) in rows.into_iter().enumerate() {
        let cluster = format!("empty{index}");
        create_cluster(&router, &cluster, case).await;
        request["cluster"] = json!(cluster);
        if !refused_without_capacity {
            let run = ok(&router, "RunTask", request, case).await;
            assert_eq!(
                list(&run, "tasks", case).len(),
                1,
                "case {case:?}: expected the task to be placed, got {run}"
            );
            continue;
        }
        let (code, message) = refusal(&router, "RunTask", request.clone(), case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the modelled refusal, got {code:?} with {message:?}"
        );
        assert_eq!(
            message, NO_CONTAINER_INSTANCES,
            "case {case:?}: expected the fixed sentence for this condition, got \
             {message:?}"
        );
        // Nothing was persisted by the refusal: the cluster still lists no task.
        let listed = ok(
            &router,
            "ListTasks",
            json!({ "cluster": cluster }),
            case,
        )
        .await;
        assert!(
            list(&listed, "taskArns", case).is_empty(),
            "case {case:?}: a refused run must leave no task behind, got {listed}"
        );

        // With capacity registered, the same request is placed — so the refusal is the
        // cluster's state, not the request's shape.
        register_capacity(&router, &cluster, case).await;
        let run = ok(&router, "RunTask", request, case).await;
        assert_eq!(
            list(&run, "tasks", case).len(),
            1,
            "case {case:?}: expected the task to be placed once capacity exists, got \
             {run}"
        );
    }
}

#[tokio::test]
async fn deleting_a_cluster_releases_the_container_instances_registered_into_it() {
    let router = service();

    // The rule generalises over how much capacity the cluster held, so rows differ along
    // the number of registered container instances.
    for registered in [1_usize, 2, 3] {
        let case = format!("{registered} registered container instances");
        let cluster = format!("capacity{registered}");
        create_cluster(&router, &cluster, &case).await;
        let mut arns = Vec::new();
        for index in 0..registered {
            let body = ok(
                &router,
                "RegisterContainerInstance",
                json!({
                    "cluster": cluster,
                    "instanceIdentityDocument":
                        crate::ecs_wire::identity_document(&format!("i-{index:017x}")),
                    "instanceIdentityDocumentSignature":
                        crate::ecs_wire::identity_document_signature(),
                }),
                &case,
            )
            .await;
            arns.push(string(
                &body["containerInstance"],
                "containerInstanceArn",
                &case,
            ));
        }

        let described = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": [cluster] }),
            &case,
        )
        .await;
        assert_eq!(
            count(
                &list(&described, "clusters", &case)[0],
                "registeredContainerInstancesCount",
                &case
            ) as usize,
            registered,
            "case {case:?}: the cluster must count the capacity registered into it, got \
             {described}"
        );

        // The delete releases them, so the cluster it answers with reports none.
        let deleted = ok(
            &router,
            "DeleteCluster",
            json!({ "cluster": cluster }),
            &case,
        )
        .await;
        assert_eq!(
            count(
                &deleted["cluster"],
                "registeredContainerInstancesCount",
                &case
            ),
            0,
            "case {case:?}: a deleted cluster reports no registered capacity, got \
             {deleted}"
        );

        // A cluster created over the tombstone starts empty, and the released instances
        // are gone with the cluster they were registered into.
        create_cluster(&router, &cluster, &case).await;
        let read = ok(
            &router,
            "DescribeContainerInstances",
            json!({ "cluster": cluster, "containerInstances": arns.clone() }),
            &case,
        )
        .await;
        assert!(
            list(&read, "containerInstances", &case).is_empty(),
            "case {case:?}: the released instances must not come back with the name, got \
             {read}"
        );
        assert_eq!(
            list(&read, "failures", &case).len(),
            registered,
            "case {case:?}: every released instance is reported missing, got {read}"
        );
        let listed = ok(
            &router,
            "ListContainerInstances",
            json!({ "cluster": cluster }),
            &case,
        )
        .await;
        assert!(
            list(&listed, "containerInstanceArns", &case).is_empty(),
            "case {case:?}: a cluster created over the tombstone holds no capacity, got \
             {listed}"
        );
        // And with no capacity, it cannot place an EC2 task either.
        register_bridge(&router, &format!("fam{registered}"), &case).await;
        refused(
            &router,
            "RunTask",
            json!({ "cluster": cluster, "taskDefinition": format!("fam{registered}") }),
            "InvalidParameterException",
            &case,
        )
        .await;
    }
}

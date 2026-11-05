// SPDX-License-Identifier: CC0-1.0

//! The task sizes a registration may store, the overrides a placement may run, and the
//! deployment bounds a service may be created or updated with — driven end to end over
//! the shared wire harness in [`crate::ecs_wire`].
//!
//! The rules under test, each one level above the request that exposed it:
//!
//! * a `TaskOverride` member replaces something the STORED revision fixed, so it is
//!   weighed against that revision: a container override names a container the revision
//!   declares, and the reservations it replaces stay inside the task's own size
//!   (`ContainerDefinition$cpu`: "the only requirement is that the total amount of CPU
//!   reserved for all containers within a task be lower than the task-level `cpu`
//!   value"; `ContainerDefinition$memory`: "The total amount of memory reserved for all
//!   containers within a task must be lower than the task `memory` value, if one is
//!   specified");
//! * a task-level `cpu`/`memory` — stored or overridden — names one of the published
//!   Fargate configurations where the task runs on Fargate, and Windows containers on
//!   Fargate start a whole vCPU above the smallest Linux size;
//! * `EphemeralStorage$sizeInGiB` is bounded at both published ends ("The minimum
//!   supported value is 21 GiB and the maximum supported value is 200 GiB");
//! * a Fargate container whose logs Amazon ECS itself delivers (the `awslogs` driver)
//!   needs the revision's task execution role, and `pidMode: host` is no Fargate task's
//!   to ask for ("On Fargate for Linux containers, the only valid value is `task`");
//! * a service's deployment bounds have to be ones a deployment could run: a grace
//!   period is never negative, and the floor and the ceiling cannot both sit at 100% or
//!   no task could ever be replaced;
//! * and the Fargate platform version a service runs is one published for the operating
//!   system its revision declares.
//!
//! Every case goes through the HTTP front door, so a refusal that is not computed from
//! the stored revision cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    answered, awsvpc_network_configuration, create_cluster, list, ok, refusal, register_capacity,
    service, string,
};

/// The image every container here starts from; irrelevant to every rule below, so one
/// value.
const IMAGE: &str = "public.ecr.aws/docker/library/nginx:latest";

/// The modelled refusal every request-validation rule here answers with, except the two
/// the registration reports as a client fault and the platform pairing, which has its own
/// modelled error.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The refusal class `RegisterTaskDefinition` reports its cross-field faults as.
const CLIENT: &str = "ClientException";

/// One container definition, with the members a row needs and nothing else. A container
/// that reserves no memory of its own is legal when the TASK reserves some, which every
/// revision here does.
fn container(name: &str, cpu: Option<i64>, memory: Option<i64>) -> Value {
    let mut definition = json!({ "name": name, "image": IMAGE });
    if let Some(memory) = memory {
        definition["memory"] = json!(memory);
    }
    if let Some(cpu) = cpu {
        definition["cpu"] = json!(cpu);
    }
    definition
}

/// Register one Fargate revision: `awsvpc`, a task size, and the containers given.
async fn register_fargate(
    router: &MultiServiceRouter,
    family: &str,
    cpu: &str,
    memory: &str,
    containers: Value,
    case: &str,
) -> Value {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": cpu,
            "memory": memory,
            "containerDefinitions": containers,
        }),
        case,
    )
    .await
}

/// A `RunTask` request for `family` on `cluster`, placed on Fargate, carrying
/// `overrides`.
fn run_with_overrides(cluster: &str, family: &str, overrides: Value) -> Value {
    json!({
        "cluster": cluster,
        "taskDefinition": family,
        "launchType": "FARGATE",
        "networkConfiguration": awsvpc_network_configuration(),
        "overrides": overrides,
    })
}

#[tokio::test]
async fn a_container_override_names_a_container_the_revision_declares() {
    let router = service();
    let case = "setup";
    create_cluster(&router, "ovr", case).await;
    // Two containers, so the rule is exercised on a revision that carries more than one
    // name and on the second entry of the list — not just on "the" container.
    register_fargate(
        &router,
        "ovrnames",
        "1024",
        "2048",
        json!([
            container("app", None, Some(256)),
            container("sidecar", None, Some(256)),
        ]),
        case,
    )
    .await;

    // (case, the `containerOverrides` list, the refusal message the row earns — empty
    // where the placement is accepted).
    let rows: [(&str, Value, &str); 7] = [
        ("no container overrides at all", json!([]), ""),
        (
            "the first container, with a command",
            json!([{ "name": "app", "command": ["echo", "hi"] }]),
            "",
        ),
        (
            "the second container, with an environment variable",
            json!([{ "name": "sidecar", "environment": [{ "name": "K", "value": "v" }] }]),
            "",
        ),
        (
            "both containers at once",
            json!([{ "name": "app" }, { "name": "sidecar" }]),
            "",
        ),
        (
            "a name no container carries",
            json!([{ "name": "does-not-exist", "command": ["echo", "hi"] }]),
            "Override for container named does-not-exist is not a container in the \
             TaskDefinition.",
        ),
        (
            "a name that differs only in case",
            json!([{ "name": "App" }]),
            "Override for container named App is not a container in the TaskDefinition.",
        ),
        (
            "an override with no name at all",
            json!([{ "command": ["echo", "hi"] }]),
            "ContainerOverride.name should not be null or empty.",
        ),
    ];

    for (case, overrides, expected_message) in rows {
        let request = run_with_overrides("ovr", "ovrnames", json!({ "containerOverrides": overrides }));
        if expected_message.is_empty() {
            let run = ok(&router, "RunTask", request, case).await;
            assert_eq!(
                list(&run, "tasks", case).len(),
                1,
                "case {case:?}: an override naming a declared container places the task, \
                 got {run}"
            );
            continue;
        }
        let (code, message) = refusal(&router, "RunTask", request, case).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected the modelled refusal, got {code:?} with {message:?}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the refusal to read {expected_message:?}, got \
             {message:?}"
        );
        // Nothing was placed: the refusal is the whole answer.
        let listed = ok(
            &router,
            "ListTasks",
            json!({ "cluster": "ovr", "family": "ovrnames" }),
            case,
        )
        .await;
        assert_eq!(
            list(&listed, "taskArns", case).len(),
            4,
            "case {case:?}: a refused run places no task, got {listed}"
        );
    }
}

#[tokio::test]
async fn start_task_weighs_its_overrides_against_the_same_revision() {
    let router = service();
    let case = "setup";
    create_cluster(&router, "startovr", case).await;
    let instance = register_capacity(&router, "startovr", case).await;
    // A plain revision whose capacity is the container instance this call names, so the
    // placement reaches the shared override rule with no Fargate task size in play.
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "startovrfam",
            "memory": "512",
            "containerDefinitions": [container("app", None, Some(256))],
        }),
        case,
    )
    .await;

    // (case, the `containerOverrides` list, the refusal message the row earns — empty
    // where the placement is accepted). The same rule the run reaches, through the other
    // placement.
    let rows: [(&str, Value, &str); 3] = [
        (
            "a container the revision declares",
            json!([{ "name": "app", "command": ["echo", "hi"] }]),
            "",
        ),
        (
            "a name no container carries",
            json!([{ "name": "nope" }]),
            "Override for container named nope is not a container in the TaskDefinition.",
        ),
        (
            "memory raised past the task's own",
            json!([{ "name": "app", "memory": 1024 }]),
            "'memory' setting for container 'app' is greater than for task.",
        ),
    ];

    for (case, overrides, expected_message) in rows {
        let request = json!({
            "cluster": "startovr",
            "taskDefinition": "startovrfam",
            "containerInstances": [instance],
            "overrides": { "containerOverrides": overrides },
        });
        if expected_message.is_empty() {
            let started = ok(&router, "StartTask", request, case).await;
            assert_eq!(
                list(&started, "tasks", case).len(),
                1,
                "case {case:?}: an override naming a declared container places the task, \
                 got {started}"
            );
            continue;
        }
        let (code, message) = refusal(&router, "StartTask", request, case).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected the modelled refusal, got {code:?} with {message:?}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the refusal to read {expected_message:?}, got \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn a_container_override_stays_inside_the_tasks_own_size() {
    let router = service();
    let case = "setup";
    create_cluster(&router, "budget", case).await;
    // One container reserving a quarter of each of the task's own reservations, so an
    // override has room to grow into and room to cross.
    register_fargate(
        &router,
        "budgetone",
        "1024",
        "2048",
        json!([container("app", Some(256), Some(512))]),
        case,
    )
    .await;
    // And a two-container revision, so the rule is tested on the TOTAL rather than on one
    // override at a time.
    register_fargate(
        &router,
        "budgettwo",
        "1024",
        "2048",
        json!([
            container("app", Some(256), Some(512)),
            container("sidecar", Some(256), Some(512)),
        ]),
        case,
    )
    .await;

    // (case, the family, the `containerOverrides` list, the refusal message the row earns
    // — empty where the placement is accepted). The task reserves 1024 CPU units and 2048
    // MiB in both families.
    let rows: [(&str, &str, Value, &str); 9] = [
        (
            "memory raised to exactly the task's own",
            "budgetone",
            json!([{ "name": "app", "memory": 2048 }]),
            "",
        ),
        (
            "memory one MiB past the task's own",
            "budgetone",
            json!([{ "name": "app", "memory": 2049 }]),
            "'memory' setting for container 'app' is greater than for task.",
        ),
        (
            "memory far past the task's own",
            "budgetone",
            json!([{ "name": "app", "memory": 4096 }]),
            "'memory' setting for container 'app' is greater than for task.",
        ),
        (
            "cpu raised to exactly the task's own",
            "budgetone",
            json!([{ "name": "app", "cpu": 1024 }]),
            "",
        ),
        (
            "cpu one unit past the task's own",
            "budgetone",
            json!([{ "name": "app", "cpu": 1025 }]),
            "Sum of container 'cpu' greater than task 'cpu'.",
        ),
        (
            "a soft limit above the hard limit it is replaced beside",
            "budgetone",
            json!([{ "name": "app", "memory": 512, "memoryReservation": 1024 }]),
            "Invalid setting for container 'app'. 'memory' must be greater than or equal \
             to 'memoryReservation'.",
        ),
        (
            "two containers adding up to exactly the task's memory",
            "budgettwo",
            json!([{ "name": "app", "memory": 1024 }, { "name": "sidecar", "memory": 1024 }]),
            "",
        ),
        (
            "two containers adding up past the task's memory",
            "budgettwo",
            json!([{ "name": "app", "memory": 1024 }, { "name": "sidecar", "memory": 1025 }]),
            "Sum of container 'memory' greater than task 'memory'.",
        ),
        (
            "one container's override pushing the pair's cpu total past the task's",
            "budgettwo",
            json!([{ "name": "app", "cpu": 769 }]),
            "Sum of container 'cpu' greater than task 'cpu'.",
        ),
    ];

    for (case, family, overrides, expected_message) in rows {
        let request =
            run_with_overrides("budget", family, json!({ "containerOverrides": overrides }));
        if expected_message.is_empty() {
            let (status, body) = answered(&router, "RunTask", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a reservation inside the task's size runs, got {status} \
                 with {body}"
            );
            continue;
        }
        let (code, message) = refusal(&router, "RunTask", request, case).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected the modelled refusal, got {code:?} with {message:?}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the refusal to read {expected_message:?}, got \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn a_task_level_override_names_a_published_fargate_task_size() {
    let router = service();
    let case = "setup";
    create_cluster(&router, "size", case).await;
    register_fargate(
        &router,
        "sizefam",
        "1024",
        "2048",
        json!([container("app", None, Some(256))]),
        case,
    )
    .await;

    // (case, the task-level overrides, whether the placement is accepted). The catalog is
    // the `RegisterTaskDefinition$memory` doc's own list of cpu/memory pairs, so the rows
    // span a listed pair, a stepped pair, a value between two steps, a pair no size
    // carries and the spellings the members document (`1 vCPU`, `2GB`).
    let rows: [(&str, Value, bool); 9] = [
        ("the smallest published pair", json!({ "cpu": "256", "memory": "512" }), true),
        ("a stepped pair", json!({ "cpu": "512", "memory": "3072" }), true),
        (
            "memory between two steps of its cpu",
            json!({ "cpu": "512", "memory": "1536" }),
            false,
        ),
        (
            "memory no size of that cpu reaches",
            json!({ "cpu": "256", "memory": "999999" }),
            false,
        ),
        ("a cpu no size carries", json!({ "cpu": "300", "memory": "2048" }), false),
        (
            "the vCPU and GB spellings the members document",
            json!({ "cpu": "1 vCPU", "memory": "2GB" }),
            true,
        ),
        ("a cpu that spells no number", json!({ "cpu": "all of it", "memory": "2048" }), false),
        ("a negative cpu", json!({ "cpu": "-1", "memory": "2048" }), false),
        (
            "memory alone, moved onto a pair the cpu supports",
            json!({ "memory": "4096" }),
            true,
        ),
    ];

    for (case, overrides, accepted) in rows {
        let request = run_with_overrides("size", "sizefam", overrides.clone());
        if accepted {
            let (status, body) = answered(&router, "RunTask", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {overrides} names a published Fargate size, got {status} \
                 with {body}"
            );
            continue;
        }
        let (code, message) = refusal(&router, "RunTask", request, case).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected the modelled refusal, got {code:?} with {message:?}"
        );
        // The refusal of an OVERRIDDEN task size names neither value: the request that
        // carried them is the one being refused.
        assert_eq!(
            message, "No Fargate configuration exists for given values.",
            "case {case:?}: {overrides} must earn the fixed sentence, got {message:?}"
        );
    }
}

#[tokio::test]
async fn ephemeral_storage_is_bounded_at_both_published_ends() {
    let router = service();
    let case = "setup";
    create_cluster(&router, "ephem", case).await;
    register_fargate(
        &router,
        "ephemfam",
        "1024",
        "2048",
        json!([container("app", None, Some(256))]),
        case,
    )
    .await;

    // (case, sizeInGiB, the refusal message the size earns — empty where it is accepted):
    // each published bound, one past each of them, and a value in between. The range is
    // the `EphemeralStorage$sizeInGiB` doc's: "The minimum supported value is 21 GiB and
    // the maximum supported value is 200 GiB".
    let rows: [(&str, i64, &str); 6] = [
        ("the documented minimum", 21, ""),
        (
            "one below the minimum",
            20,
            "EphemeralStorage size should be at least 21",
        ),
        ("a size inside the range", 100, ""),
        ("the documented maximum", 200, ""),
        (
            "one above the maximum",
            201,
            "EphemeralStorage size should be at most 200",
        ),
        (
            "no storage at all",
            0,
            "EphemeralStorage size should be at least 21",
        ),
    ];

    for (index, (case, size, expected_message)) in rows.iter().enumerate() {
        let accepted = expected_message.is_empty();
        // The registration that STORES the setting.
        let registration = json!({
            "family": format!("ephemreg{index}"),
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "1024",
            "memory": "2048",
            "ephemeralStorage": { "sizeInGiB": size },
            "containerDefinitions": [container("app", None, Some(256))],
        });
        if accepted {
            let (status, body) =
                answered(&router, "RegisterTaskDefinition", registration).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {size} GiB is supported, got {status} with {body}"
            );
            // Read the revision back: the size it stored is the size it was given.
            let stored = ok(
                &router,
                "DescribeTaskDefinition",
                json!({ "taskDefinition": format!("ephemreg{index}") }),
                case,
            )
            .await;
            let read_back = stored["taskDefinition"]["ephemeralStorage"]["sizeInGiB"].as_i64();
            assert_eq!(
                read_back,
                Some(*size),
                "case {case:?}: expected the revision to read back {size} GiB, got {stored}"
            );
        } else {
            let (code, message) =
                refusal(&router, "RegisterTaskDefinition", registration, case).await;
            assert_eq!(
                code, CLIENT,
                "case {case:?}: expected a {CLIENT}, got {code:?} with {message:?}"
            );
            assert_eq!(
                &message, expected_message,
                "case {case:?}: expected the refusal to read {expected_message:?}, got \
                 {message:?}"
            );
        }

        // And the PLACEMENT that overrides it, held to the same range and worded the same
        // way.
        let request = run_with_overrides(
            "ephem",
            "ephemfam",
            json!({ "ephemeralStorage": { "sizeInGiB": size } }),
        );
        if accepted {
            let (status, body) = answered(&router, "RunTask", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: an override of {size} GiB runs, got {status} with {body}"
            );
            continue;
        }
        let (code, message) = refusal(&router, "RunTask", request, case).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected the modelled refusal, got {code:?} with {message:?}"
        );
        assert_eq!(
            &message, expected_message,
            "case {case:?}: expected the override refusal to read {expected_message:?}, \
             got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_fargate_revision_is_held_to_the_platforms_own_requirements() {
    let router = service();

    // (case, the members the registration adds to a Fargate revision, the refusal message
    // the row earns — empty where it is accepted). The rows span the process namespace,
    // the operating-system family's task sizes, and the log driver Amazon ECS delivers for
    // the task.
    let rows: [(&str, Value, &str); 9] = [
        ("the private namespace, declared by naming none", json!({}), ""),
        ("the task process namespace", json!({ "pidMode": "task" }), ""),
        (
            "the host process namespace",
            json!({ "pidMode": "host" }),
            "Tasks using the Fargate launch type do not support pidMode 'host'. The \
             supported value for pidMode is 'task'.",
        ),
        (
            "a Windows family at the smallest Linux task size",
            json!({
                "cpu": "256",
                "memory": "512",
                "runtimePlatform": { "operatingSystemFamily": "WINDOWS_SERVER_2019_CORE" },
            }),
            "No Fargate configuration exists for given values: 256 CPU, 512 memory. See \
             the Amazon ECS documentation for the valid values.",
        ),
        (
            "a Windows family at half a vCPU",
            json!({
                "cpu": "512",
                "memory": "1024",
                "runtimePlatform": { "operatingSystemFamily": "WINDOWS_SERVER_2022_CORE" },
            }),
            "No Fargate configuration exists for given values: 512 CPU, 1024 memory. See \
             the Amazon ECS documentation for the valid values.",
        ),
        (
            "a Windows family at the vCPU its configurations start at",
            json!({
                "cpu": "1024",
                "memory": "2048",
                "runtimePlatform": { "operatingSystemFamily": "WINDOWS_SERVER_2019_CORE" },
            }),
            "",
        ),
        (
            "a Linux family at the smallest Linux task size",
            json!({
                "cpu": "256",
                "memory": "512",
                "runtimePlatform": { "operatingSystemFamily": "LINUX" },
            }),
            "",
        ),
        (
            "the awslogs driver with no task execution role",
            json!({
                "containerDefinitions": [{
                    "name": "app",
                    "image": IMAGE,
                    "memory": 256,
                    "logConfiguration": {
                        "logDriver": "awslogs",
                        "options": { "awslogs-group": "/ecs/demo" },
                    },
                }],
            }),
            "Fargate requires task definition to have execution role ARN to support log \
             driver awslogs.",
        ),
        (
            "a driver Amazon ECS delivers nothing for, with no task execution role",
            json!({
                "containerDefinitions": [{
                    "name": "app",
                    "image": IMAGE,
                    "memory": 256,
                    "logConfiguration": { "logDriver": "splunk" },
                }],
            }),
            "",
        ),
    ];

    for (index, (case, extra, expected_message)) in rows.iter().enumerate() {
        let mut request = json!({
            "family": format!("platformfam{index}"),
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "1024",
            "memory": "2048",
            "containerDefinitions": [container("app", None, Some(256))],
        });
        for (member, value) in extra.as_object().expect("a table row is an object") {
            request[member] = value.clone();
        }
        if expected_message.is_empty() {
            let (status, body) =
                answered(&router, "RegisterTaskDefinition", request.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 for {request}, got {status} with {body}"
            );
            continue;
        }
        let (code, message) =
            refusal(&router, "RegisterTaskDefinition", request, case).await;
        assert_eq!(
            code, CLIENT,
            "case {case:?}: expected a {CLIENT}, got {code:?} with {message:?}"
        );
        assert_eq!(
            &message, expected_message,
            "case {case:?}: expected the refusal to read {expected_message:?}, got \
             {message:?}"
        );
    }

    // The same two settings on a revision that asks for no Fargate compatibility at all:
    // the host process namespace is a container instance's to share, and an EC2 task's
    // agent carries the instance's own credentials, so neither is refused there.
    let (status, body) = answered(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "platformec2",
            "pidMode": "host",
            "containerDefinitions": [{
                "name": "app",
                "image": IMAGE,
                "memory": 256,
                "logConfiguration": { "logDriver": "awslogs" },
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an EC2 revision may share the host namespace and ship its own logs, got {status} \
         with {body}"
    );
}

/// Register a Fargate revision for one operating-system family and return its family
/// name.
async fn register_for_os(
    router: &MultiServiceRouter,
    family: &str,
    os_family: &str,
    case: &str,
) -> String {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "1024",
            "memory": "2048",
            "runtimePlatform": { "operatingSystemFamily": os_family },
            "containerDefinitions": [container("app", None, Some(256))],
        }),
        case,
    )
    .await;
    family.to_string()
}

#[tokio::test]
async fn a_services_platform_version_is_one_published_for_its_revisions_operating_system() {
    let router = service();
    let case = "setup";
    create_cluster(&router, "platform", case).await;
    let linux = register_for_os(&router, "svclinux", "LINUX", case).await;
    let windows = register_for_os(&router, "svcwindows", "WINDOWS_SERVER_2019_CORE", case).await;

    // (case, the revision, the platform version asked for, whether it is accepted). The
    // WINDOWS families have exactly one published Fargate platform version (`1.0.0`), and
    // the Linux versions are not published for them; `LATEST` names no version of either,
    // so it resolves against whichever family the revision declares.
    let rows: [(&str, &str, Option<&str>, bool); 7] = [
        ("Linux on its own version", linux.as_str(), Some("1.4.0"), true),
        ("Linux on LATEST", linux.as_str(), Some("LATEST"), true),
        ("Linux naming none", linux.as_str(), None, true),
        ("Windows on its own version", windows.as_str(), Some("1.0.0"), true),
        ("Windows on LATEST", windows.as_str(), Some("LATEST"), true),
        ("Windows naming none", windows.as_str(), None, true),
        (
            "Windows on the Linux version",
            windows.as_str(),
            Some("1.4.0"),
            false,
        ),
    ];

    let mut created = 0usize;
    for (index, (case, family, platform_version, accepted)) in rows.iter().enumerate() {
        let mut request = json!({
            "cluster": "platform",
            "serviceName": format!("platformsvc{index}"),
            "taskDefinition": family,
            "desiredCount": 0,
            "launchType": "FARGATE",
            "networkConfiguration": awsvpc_network_configuration(),
        });
        if let Some(version) = platform_version {
            request["platformVersion"] = json!(version);
        }
        let (status, body) = answered(&router, "CreateService", request).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a published pairing creates the service, got {status} with \
                 {body}"
            );
            created += 1;
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: a version the revision's operating system has none of is \
             refused, got {status} with {body}"
        );
        let code = body["__type"].as_str().unwrap_or_default();
        assert!(
            code.contains("PlatformUnknownException"),
            "case {case:?}: expected the modelled platform refusal, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("The specified platform does not exist."),
            "case {case:?}: expected the fixed sentence for a platform that does not \
             exist, got {body}"
        );
        let listed = ok(
            &router,
            "ListServices",
            json!({ "cluster": "platform" }),
            case,
        )
        .await;
        assert_eq!(
            list(&listed, "serviceArns", case).len(),
            created,
            "case {case:?}: a refused create stores no service, got {listed}"
        );
    }

    // The same pairing on a PLACEMENT, which names a platform version and a revision too:
    // one rule, both callers.
    for (case, family, platform_version, accepted) in [
        ("a run of the Windows revision on its own version", &windows, "1.0.0", true),
        ("a run of the Windows revision on the Linux version", &windows, "1.4.0", false),
        ("a run of the Linux revision on its own version", &linux, "1.4.0", true),
    ] {
        let request = json!({
            "cluster": "platform",
            "taskDefinition": family,
            "launchType": "FARGATE",
            "platformVersion": platform_version,
            "networkConfiguration": awsvpc_network_configuration(),
        });
        if accepted {
            let run = ok(&router, "RunTask", request, case).await;
            assert_eq!(
                list(&run, "tasks", case).len(),
                1,
                "case {case:?}: a published pairing places the task, got {run}"
            );
            continue;
        }
        let (code, message) = refusal(&router, "RunTask", request, case).await;
        assert_eq!(
            code, "PlatformUnknownException",
            "case {case:?}: expected the modelled platform refusal, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message, "The specified platform does not exist.",
            "case {case:?}: expected the fixed sentence, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_services_deployment_bounds_have_to_leave_the_scheduler_room() {
    let router = service();
    let case = "setup";
    create_cluster(&router, "bounds", case).await;
    register_fargate(
        &router,
        "boundsfam",
        "1024",
        "2048",
        json!([container("app", None, Some(256))]),
        case,
    )
    .await;

    // (case, the members the create adds, the refusal message the row earns — empty where
    // the create is accepted).
    let grace_period_floor = "healthCheckGracePeriod must be at least 0";
    let no_room = "Both maximumPercent and minimumHealthyPercent cannot be 100 as this \
                   will block deployments.";
    let rows: [(&str, Value, &str); 8] = [
        ("no bounds at all", json!({}), ""),
        (
            "a grace period of none, the documented default",
            json!({ "healthCheckGracePeriodSeconds": 0 }),
            "",
        ),
        (
            "a grace period of five minutes",
            json!({ "healthCheckGracePeriodSeconds": 300 }),
            "",
        ),
        (
            "a negative grace period",
            json!({ "healthCheckGracePeriodSeconds": -1 }),
            grace_period_floor,
        ),
        (
            "a far more negative grace period",
            json!({ "healthCheckGracePeriodSeconds": -2147483648_i64 }),
            grace_period_floor,
        ),
        (
            "the replica defaults, spelled out",
            json!({ "deploymentConfiguration": { "minimumHealthyPercent": 100, "maximumPercent": 200 } }),
            "",
        ),
        (
            "a floor under the ceiling",
            json!({ "deploymentConfiguration": { "minimumHealthyPercent": 50, "maximumPercent": 100 } }),
            "",
        ),
        (
            "the floor and the ceiling both at 100",
            json!({ "deploymentConfiguration": { "minimumHealthyPercent": 100, "maximumPercent": 100 } }),
            no_room,
        ),
    ];

    let mut created = 0usize;
    for (index, (case, extra, expected_message)) in rows.iter().enumerate() {
        let name = format!("boundssvc{index}");
        let mut request = json!({
            "cluster": "bounds",
            "serviceName": name,
            "taskDefinition": "boundsfam",
            "desiredCount": 0,
            "launchType": "FARGATE",
            "networkConfiguration": awsvpc_network_configuration(),
        });
        for (member, value) in extra.as_object().expect("a table row is an object") {
            request[member] = value.clone();
        }
        let (status, body) = answered(&router, "CreateService", request).await;
        let expected = if expected_message.is_empty() { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {body}"
        );
        if expected_message.is_empty() {
            created += 1;
            continue;
        }
        let code = body["__type"].as_str().unwrap_or_default();
        assert!(
            code.contains(INVALID_PARAMETER),
            "case {case:?}: expected a {INVALID_PARAMETER}, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(*expected_message),
            "case {case:?}: expected the refusal to read {expected_message:?}, got {body}"
        );
        let listed = ok(&router, "ListServices", json!({ "cluster": "bounds" }), case).await;
        assert_eq!(
            list(&listed, "serviceArns", case).len(),
            created,
            "case {case:?}: a refused create stores no service, got {listed}"
        );
    }

    // The same bounds on the UPDATE of a service, which carries only the members it
    // changes: the pair is weighed on the configuration the service is LEFT with, so a
    // request naming one half of it is refused against the half the service holds.
    let subject = ok(
        &router,
        "CreateService",
        json!({
            "cluster": "bounds",
            "serviceName": "boundsupdate",
            "taskDefinition": "boundsfam",
            "desiredCount": 0,
            "launchType": "FARGATE",
            "networkConfiguration": awsvpc_network_configuration(),
            "deploymentConfiguration": { "minimumHealthyPercent": 50, "maximumPercent": 100 },
        }),
        case,
    )
    .await;
    assert_eq!(
        string(&subject["service"], "serviceName", case),
        "boundsupdate",
        "the subject of the update suite is created"
    );

    // (case, the members the update carries, the refusal message the row earns — empty
    // where the update is accepted).
    let updates: [(&str, Value, &str); 5] = [
        (
            "a floor the stored ceiling has room above",
            json!({ "deploymentConfiguration": { "minimumHealthyPercent": 0 } }),
            "",
        ),
        (
            "both halves at 100 in one request",
            json!({ "deploymentConfiguration": { "minimumHealthyPercent": 100, "maximumPercent": 100 } }),
            no_room,
        ),
        (
            "the floor alone, onto a stored ceiling of 100",
            json!({ "deploymentConfiguration": { "minimumHealthyPercent": 100 } }),
            no_room,
        ),
        (
            "the floor at 100 with room made above it",
            json!({ "deploymentConfiguration": { "minimumHealthyPercent": 100, "maximumPercent": 200 } }),
            "",
        ),
        (
            "a negative grace period",
            json!({ "healthCheckGracePeriodSeconds": -5 }),
            grace_period_floor,
        ),
    ];

    for (case, extra, expected_message) in updates {
        // What the service holds BEFORE this request, so a refused update is shown to
        // have written nothing rather than compared against a constant.
        let before = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": "bounds", "services": ["boundsupdate"] }),
            case,
        )
        .await;
        let held = list(&before, "services", case)[0]["deploymentConfiguration"].clone();
        let mut request = json!({
            "cluster": "bounds",
            "service": "boundsupdate",
        });
        for (member, value) in extra.as_object().expect("a table row is an object") {
            request[member] = value.clone();
        }
        let (status, body) = answered(&router, "UpdateService", request).await;
        let expected = if expected_message.is_empty() { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {body}"
        );
        if expected_message.is_empty() {
            continue;
        }
        let code = body["__type"].as_str().unwrap_or_default();
        assert!(
            code.contains(INVALID_PARAMETER),
            "case {case:?}: expected a {INVALID_PARAMETER}, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected_message),
            "case {case:?}: expected the refusal to read {expected_message:?}, got {body}"
        );
        // And nothing of the refused update reached the service.
        let described = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": "bounds", "services": ["boundsupdate"] }),
            case,
        )
        .await;
        let services = list(&described, "services", case);
        let configuration = &services[0]["deploymentConfiguration"];
        assert_eq!(
            configuration, &held,
            "case {case:?}: a refused update leaves the stored configuration alone, got \
             {configuration}"
        );
    }
}

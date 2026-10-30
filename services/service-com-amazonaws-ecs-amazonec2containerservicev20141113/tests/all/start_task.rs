// SPDX-License-Identifier: CC0-1.0

//! `StartTask`: the placement a CLIENT directs at container instances it names.
//!
//! The rule under test: one task of the named revision is placed on each container
//! instance the call names that this cluster can place a task on, each task recording
//! the instance that hosts it, and every entry naming no such instance is reported as a
//! `MISSING` failure instead of refusing the whole call. Every case goes through the
//! shared wire harness in [`crate::ecs_wire`], the same front door a client uses, so
//! nothing reaches into the state struct.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, count, create_cluster, list, ok, refusal, register_definition,
    register_instance, string,
};

/// How often a lifecycle poll re-reads a task, and how long it keeps trying. The interval
/// is well inside one transition span (`state::TRANSITION_SPAN` is 500 ms), so every
/// status the task passes through is seen.
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const POLL_BUDGET: Duration = Duration::from_secs(10);

/// The `Failure$reason` a batch read reports for an identifier that resolves to no
/// resource — the one this API documents for a resource that is not there.
const MISSING: &str = "MISSING";

/// How one row of the placement table names a container instance.
#[derive(Debug, Clone, Copy)]
enum Named {
    /// A registered instance of the row's cluster, by its container-instance ID — the
    /// member takes "the container instance IDs or full ARN entries".
    Id(usize),
    /// The same instance, by its full ARN.
    Arn(usize),
    /// An id no container instance of this account carries.
    Unknown,
    /// An instance that was registered and then deregistered: it is out of the cluster's
    /// capacity, so it can host nothing.
    Deregistered,
    /// A registered instance of ANOTHER cluster, by full ARN: the `cluster` member scopes
    /// the request, so it is not an instance this call names.
    OtherCluster,
}

/// One row of the placement table: the instances the row registers, how the call names
/// them, and how many tasks and failures that must leave.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// How many container instances the row registers into its own cluster.
    registered: usize,
    named: &'static [Named],
    placed: usize,
    failed: usize,
}

/// Every `lastStatus` one task is observed in, until it reaches `RUNNING` or the poll
/// budget runs out.
async fn status_until_running(
    router: &MultiServiceRouter,
    cluster: &str,
    task_arn: &str,
    case: &str,
) -> String {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let task = describe_one(router, cluster, task_arn, case).await;
        let status = string(&task, "lastStatus", case);
        if status == "RUNNING" || Instant::now() >= deadline {
            return status;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Read ONE task back through `DescribeTasks` — the stored record, not the response of
/// the call that placed it.
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

/// Read ONE container instance back through `DescribeContainerInstances`.
async fn describe_instance(
    router: &MultiServiceRouter,
    cluster: &str,
    instance_arn: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeContainerInstances",
        json!({ "cluster": cluster, "containerInstances": [instance_arn] }),
        case,
    )
    .await;
    list(&body, "containerInstances", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected one instance back, got {body}"))
}

/// The container-instance id of a container instance ARN: its last segment, which is what
/// a caller may name instead of the ARN.
fn instance_id(instance_arn: &str) -> String {
    instance_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn start_task_places_one_task_on_each_container_instance_the_call_names() {
    let router = service_with_definition().await;
    let family = FAMILY;

    // The rows differ along everything the rule generalises over: HOW an instance is
    // named (id or full ARN), how MANY instances one call names (one, several, the
    // published bound of ten), whether an entry names placeable capacity at all (unknown,
    // deregistered, another cluster's), whether the same instance is named twice, and
    // whether placeable and unplaceable entries are mixed in one call.
    let rows = [
        Row {
            case: "one instance, by id",
            registered: 1,
            named: &[Named::Id(0)],
            placed: 1,
            failed: 0,
        },
        Row {
            case: "one instance, by full ARN",
            registered: 1,
            named: &[Named::Arn(0)],
            placed: 1,
            failed: 0,
        },
        Row {
            case: "three instances, one task each",
            registered: 3,
            named: &[Named::Id(0), Named::Arn(1), Named::Id(2)],
            placed: 3,
            failed: 0,
        },
        Row {
            case: "the same instance named twice is one instance",
            registered: 1,
            named: &[Named::Id(0), Named::Arn(0)],
            placed: 1,
            failed: 0,
        },
        Row {
            case: "ten instances, the most one call may name",
            registered: 10,
            named: &[
                Named::Id(0),
                Named::Id(1),
                Named::Id(2),
                Named::Id(3),
                Named::Id(4),
                Named::Id(5),
                Named::Id(6),
                Named::Id(7),
                Named::Id(8),
                Named::Id(9),
            ],
            placed: 10,
            failed: 0,
        },
        Row {
            case: "an unknown id beside a registered instance",
            registered: 1,
            named: &[Named::Id(0), Named::Unknown],
            placed: 1,
            failed: 1,
        },
        Row {
            case: "unknown ids only",
            registered: 1,
            named: &[Named::Unknown, Named::Unknown],
            placed: 0,
            failed: 1,
        },
        Row {
            case: "a deregistered instance is no capacity of the cluster",
            registered: 1,
            named: &[Named::Deregistered],
            placed: 0,
            failed: 1,
        },
        Row {
            case: "a deregistered instance beside a registered one",
            registered: 1,
            named: &[Named::Deregistered, Named::Id(0)],
            placed: 1,
            failed: 1,
        },
        Row {
            case: "an instance of another cluster is not this cluster's",
            registered: 1,
            named: &[Named::OtherCluster],
            placed: 0,
            failed: 1,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // Each row gets its OWN cluster, so the capacity one row registers is never the
        // capacity another row's placement finds.
        let cluster = format!("placed{index}");
        create_cluster(&router, &cluster, case).await;
        let mut instances = Vec::new();
        for slot in 0..row.registered {
            instances.push(
                register_instance(&router, &cluster, &format!("i-{index:04x}{slot:013x}"), case)
                    .await,
            );
        }

        let mut references = Vec::new();
        for named in row.named {
            references.push(match named {
                Named::Id(slot) => instance_id(&instances[*slot]),
                Named::Arn(slot) => instances[*slot].clone(),
                // 32 hex digits, the form a container-instance id takes.
                Named::Unknown => format!("{:032x}", 0xdead_beef_usize + index),
                Named::Deregistered => {
                    let arn = register_instance(
                        &router,
                        &cluster,
                        &format!("i-{index:04x}ffffffffffff{index:x}"),
                        case,
                    )
                    .await;
                    ok(
                        &router,
                        "DeregisterContainerInstance",
                        json!({ "cluster": cluster, "containerInstance": arn }),
                        case,
                    )
                    .await;
                    arn
                }
                Named::OtherCluster => {
                    let other = format!("elsewhere{index}");
                    create_cluster(&router, &other, case).await;
                    register_instance(&router, &other, &format!("i-{index:04x}aaaaaaaaaaaa"), case)
                        .await
                }
            });
        }

        let body = ok(
            &router,
            "StartTask",
            json!({
                "cluster": cluster,
                "taskDefinition": family,
                "containerInstances": references,
            }),
            case,
        )
        .await;

        let tasks = list(&body, "tasks", case);
        assert_eq!(
            tasks.len(),
            row.placed,
            "case {case:?}: expected {} task(s), got {body}",
            row.placed
        );
        let failures = list(&body, "failures", case);
        assert_eq!(
            failures.len(),
            row.failed,
            "case {case:?}: expected {} failure(s), got {body}",
            row.failed
        );
        for failure in &failures {
            assert_eq!(
                string(failure, "reason", case),
                MISSING,
                "case {case:?}: an entry that names no placeable instance is MISSING, got \
                 {failure}"
            );
            assert!(
                !string(failure, "arn", case).is_empty(),
                "case {case:?}: a failure names the identifier the request sent, got {failure}"
            );
        }

        let mut hosts = Vec::new();
        for task in &tasks {
            let host = string(task, "containerInstanceArn", case);
            assert!(
                instances.contains(&host),
                "case {case:?}: a task must be hosted by an instance the call named, got {task}"
            );
            assert!(
                !hosts.contains(&host),
                "case {case:?}: one instance takes one task of a call, got {host:?} twice in \
                 {body}"
            );
            hosts.push(host.clone());
            assert_eq!(
                string(task, "lastStatus", case),
                "PENDING",
                "case {case:?}: a placed task has not started yet, got {task}"
            );
            assert_eq!(
                string(task, "launchType", case),
                "EC2",
                "case {case:?}: a task on a container instance runs on the capacity \
                 registered to the cluster, got {task}"
            );
            assert_eq!(
                string(task, "group", case),
                format!("family:{family}"),
                "case {case:?}: the default group is the family name, got {task}"
            );

            // The placement is STORED: the task reads back with the same host instance,
            // and the instance's own task listing selects it.
            let task_arn = string(task, "taskArn", case);
            let read = describe_one(&router, &cluster, &task_arn, case).await;
            assert_eq!(
                string(&read, "containerInstanceArn", case),
                host,
                "case {case:?}: expected the stored task to record its host instance, got \
                 {read}"
            );
            let listed = ok(
                &router,
                "ListTasks",
                json!({ "cluster": cluster, "containerInstance": host }),
                case,
            )
            .await;
            assert!(
                list(&listed, "taskArns", case).contains(&Value::from(task_arn.clone())),
                "case {case:?}: expected the host instance's tasks to include {task_arn:?}, \
                 got {listed}"
            );
        }
    }
}

/// The family every suite case places, registered once per router.
const FAMILY: &str = "started";

/// A router with one cluster-less revision of [`FAMILY`] registered.
async fn service_with_definition() -> MultiServiceRouter {
    let router = crate::ecs_wire::service();
    register_definition(&router, FAMILY, "setup").await;
    router
}

/// One row of the refusal table: the request that must be refused, the modelled error
/// code it earns, and whether its cluster exists (so the suite can check nothing was
/// placed in it).
struct Refusal {
    case: &'static str,
    request: Value,
    code: &'static str,
    cluster: Option<&'static str>,
}

#[tokio::test]
async fn start_task_refuses_a_request_it_can_place_nothing_for() {
    let router = service_with_definition().await;
    let case = "setup";
    let cluster = "refusals";
    create_cluster(&router, cluster, case).await;
    let instance = register_instance(&router, cluster, "i-0ref0ref0ref0ref0", case).await;
    let id = instance_id(&instance);
    // An `awsvpc` revision gets its own elastic network interface, so a placement of it
    // has to say which subnets and security groups that interface takes.
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "startedawsvpc",
            "networkMode": "awsvpc",
            "containerDefinitions": [
                crate::task_lifecycle::container("web", "public.ecr.aws/nginx/nginx:latest"),
            ],
        }),
        case,
    )
    .await;

    let ten_more: Vec<String> = (0..11).map(|_| id.clone()).collect();
    let rows = [
        Refusal {
            case: "no container instance at all",
            request: json!({
                "cluster": cluster,
                "taskDefinition": FAMILY,
                "containerInstances": [],
            }),
            code: "InvalidParameterException",
            cluster: Some(cluster),
        },
        Refusal {
            case: "one past the ten instances a call may name",
            request: json!({
                "cluster": cluster,
                "taskDefinition": FAMILY,
                "containerInstances": ten_more,
            }),
            code: "InvalidParameterException",
            cluster: Some(cluster),
        },
        Refusal {
            case: "the SERVICE tag set on a task no service placed",
            request: json!({
                "cluster": cluster,
                "taskDefinition": FAMILY,
                "containerInstances": [id.clone()],
                "propagateTags": "SERVICE",
            }),
            code: "InvalidParameterException",
            cluster: Some(cluster),
        },
        Refusal {
            case: "an awsvpc revision placed with no network configuration",
            request: json!({
                "cluster": cluster,
                "taskDefinition": "startedawsvpc",
                "containerInstances": [id.clone()],
            }),
            code: "InvalidParameterException",
            cluster: Some(cluster),
        },
        Refusal {
            case: "a cluster this account does not hold",
            request: json!({
                "cluster": "nosuchcluster",
                "taskDefinition": FAMILY,
                "containerInstances": [id.clone()],
            }),
            code: "ClusterNotFoundException",
            cluster: None,
        },
        Refusal {
            case: "a task definition family nothing registered",
            request: json!({
                "cluster": cluster,
                "taskDefinition": "neverregistered",
                "containerInstances": [id.clone()],
            }),
            code: "ClientException",
            cluster: Some(cluster),
        },
    ];

    for row in rows {
        let case = row.case;
        let (code, message) = refusal(&router, "StartTask", row.request, case).await;
        assert_eq!(
            code, row.code,
            "case {case:?}: expected a {}, got {code:?} with {message:?}",
            row.code
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: a refusal states why, got {message:?}"
        );
        // A refused placement places nothing: there is no task for a call that was never
        // accepted.
        if let Some(cluster) = row.cluster {
            let listed = ok(&router, "ListTasks", json!({ "cluster": cluster }), case).await;
            assert!(
                list(&listed, "taskArns", case).is_empty(),
                "case {case:?}: a refused StartTask must place no task, got {listed}"
            );
        }
    }
}

#[tokio::test]
async fn a_started_task_is_counted_by_the_instance_that_hosts_it_until_it_runs() {
    let router = service_with_definition().await;
    let case = "a task started on one of two registered instances";
    let cluster = "hosting";
    create_cluster(&router, cluster, case).await;
    let host = register_instance(&router, cluster, "i-0host0host0host0h", case).await;
    let idle = register_instance(&router, cluster, "i-0idle0idle0idle0i", case).await;

    let body = ok(
        &router,
        "StartTask",
        json!({
            "cluster": cluster,
            "taskDefinition": FAMILY,
            "containerInstances": [instance_id(&host)],
            "startedBy": "a-batch-job",
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
        string(&task, "containerInstanceArn", case),
        host,
        "case {case:?}: expected the task on the instance the call named, got {task}"
    );
    assert_eq!(
        string(&task, "startedBy", case),
        "a-batch-job",
        "case {case:?}: expected the task to carry what started it, got {task}"
    );
    assert!(
        task["startedAt"].is_null(),
        "case {case:?}: a PENDING task has not started, got {task}"
    );

    // The instance that hosts it counts it, and the one that does not holds nothing.
    let read = describe_instance(&router, cluster, &host, case).await;
    assert_eq!(
        (
            count(&read, "pendingTasksCount", case),
            count(&read, "runningTasksCount", case)
        ),
        (1, 0),
        "case {case:?}: expected the host instance to count one pending task, got {read}"
    );
    let read = describe_instance(&router, cluster, &idle, case).await;
    assert_eq!(
        (
            count(&read, "pendingTasksCount", case),
            count(&read, "runningTasksCount", case)
        ),
        (0, 0),
        "case {case:?}: expected the other instance to hold no task, got {read}"
    );
    let listed = ok(
        &router,
        "ListTasks",
        json!({ "cluster": cluster, "containerInstance": instance_id(&idle) }),
        case,
    )
    .await;
    assert!(
        list(&listed, "taskArns", case).is_empty(),
        "case {case:?}: expected no task on the instance nothing was placed on, got {listed}"
    );

    // The start is owed by the clock, not by this call: once it falls due the task runs,
    // and the host instance counts it as running instead.
    let status = status_until_running(&router, cluster, &task_arn, case).await;
    assert_eq!(
        status, "RUNNING",
        "case {case:?}: expected the placed task to start, observed {status:?}"
    );
    let running = describe_one(&router, cluster, &task_arn, case).await;
    assert!(
        !running["startedAt"].is_null(),
        "case {case:?}: a running task carries a start time, got {running}"
    );
    let read = describe_instance(&router, cluster, &host, case).await;
    assert_eq!(
        (
            count(&read, "pendingTasksCount", case),
            count(&read, "runningTasksCount", case)
        ),
        (0, 1),
        "case {case:?}: expected the host instance to count the task as running, got {read}"
    );
}

#[tokio::test]
async fn a_started_task_carries_the_networking_and_tags_its_placement_asked_for() {
    let router = crate::ecs_wire::service();
    let case = "an awsvpc revision placed with a network configuration and tags";
    let cluster = "startednetworking";
    create_cluster(&router, cluster, case).await;
    let host = register_instance(&router, cluster, "i-0net0net0net0net0", case).await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "startednet",
            "networkMode": "awsvpc",
            "containerDefinitions": [
                crate::task_lifecycle::container("web", "public.ecr.aws/nginx/nginx:latest"),
            ],
            "tags": [{ "key": "from", "value": "definition" }],
        }),
        case,
    )
    .await;

    let body = ok(
        &router,
        "StartTask",
        json!({
            "cluster": cluster,
            "taskDefinition": "startednet",
            "containerInstances": [host.clone()],
            "networkConfiguration": awsvpc_network_configuration(),
            "propagateTags": "TASK_DEFINITION",
            "tags": [{ "key": "from", "value": "request" }, { "key": "own", "value": "tag" }],
            "group": "a-named-group",
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
        string(&task, "group", case),
        "a-named-group",
        "case {case:?}: expected the group the call named, got {task}"
    );
    assert!(
        !list(&task, "attachments", case).is_empty(),
        "case {case:?}: an awsvpc task is allocated its own interface, got {task}"
    );

    // The tag set is STORED, and read back through the operation that reports a task's
    // tags: the key the revision propagated is on the task once, with the propagated
    // value, and the request's own extra key is there too.
    let read = ok(
        &router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": [task_arn], "include": ["TAGS"] }),
        case,
    )
    .await;
    let stored = list(&read, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task back, got {read}"));
    let mut tags: Vec<(String, String)> = list(&stored, "tags", case)
        .iter()
        .map(|tag| (string(tag, "key", case), string(tag, "value", case)))
        .collect();
    tags.sort();
    assert_eq!(
        tags,
        vec![
            ("from".to_string(), "definition".to_string()),
            ("own".to_string(), "tag".to_string()),
        ],
        "case {case:?}: expected the propagated and requested tags, got {stored}"
    );
}

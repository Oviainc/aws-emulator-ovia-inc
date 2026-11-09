// SPDX-License-Identifier: CC0-1.0

//! How the interface an `awsvpc` task is allocated gets ATTACHED, and when a task reports
//! itself CONNECTED — driven end to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rules under test, asserted over tables that span the class rather than over one
//! request:
//!
//! * `Attachment$status` is "the status of the attachment. Valid values are `PRECREATED`,
//!   `CREATED`, `ATTACHING`, `ATTACHED`, `DETACHING`, `DETACHED`, `DELETED`, and
//!   `FAILED`", and `RegisterTaskDefinition$networkMode` says an `awsvpc` task "is
//!   allocated an elastic network interface". Amazon ECS creates that interface, attaches
//!   it, and only then launches the task ("PROVISIONING: Amazon ECS has to perform
//!   additional steps before the task is launched", the ECS task lifecycle), so one read
//!   after another sees the attachment pass PRECREATED, ATTACHING, ATTACHED while the task
//!   passes PROVISIONING, PENDING, RUNNING — the interface in place before the task runs.
//! * `Task$connectivity` is "the connectivity status of a task" (the model's
//!   `Connectivity` enum: `CONNECTED`, `DISCONNECTED`) and `Task$connectivityAt` "the Unix
//!   timestamp for the time when the task last went into `CONNECTED` status". A task is on
//!   the network once the networking it runs with is in place: the instant the interface it
//!   was allocated is attached, or — for a task that shares the networking of the host that
//!   runs it — the instant it starts there.
//! * `SubmitAttachmentStateChanges` is "sent to acknowledge that an attachment changed
//!   states", so the agent's word is what the record holds, and an agent that reports the
//!   interface in place connects the task at that instant. The state a report may name is
//!   `ATTACHED` alone — Amazon ECS takes the interface as far as `ATTACHING` itself — and a
//!   report naming any other state is refused and leaves the progression to the clock.
//!
//! The rows differ along which launch path placed the task (`RunTask`, `StartTask`, a
//! service's own scheduler), along the network mode (every value of the model's
//! `NetworkMode` enum), and along the status the report names (the one it accepts, and every
//! other value `Attachment$status` lists).
//!
//! Nothing here reaches into the state struct: every value asserted is read back through
//! `DescribeTasks`, so a response that is not computed from persisted state cannot pass.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, cluster_with_capacity, create_cluster, list, ok, refusal,
    register_capacity, service, string, AWSVPC_START,
};
use crate::task_lifecycle::container;

/// The cluster every task below runs in.
const CLUSTER: &str = "attachprogress";

/// The image the containers start from; irrelevant to these rules, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// How often a poll re-reads a task. Well inside one step of the interface's progression,
/// so every status it passes through is seen.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long a poll keeps trying before it gives up: the whole start of an `awsvpc` task
/// ([`AWSVPC_START`]) and one more second of margin.
const POLL_BUDGET: Duration = AWSVPC_START.saturating_add(Duration::from_secs(1));

/// Long enough that the FIRST step of an interface's progression would have fallen due
/// (`state::INTERFACE_STEP_SPAN`), with margin for a loaded test machine — what a task
/// nothing is attaching an interface for is read after.
const PAST_THE_FIRST_ATTACH_STEP: Duration =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::INTERFACE_STEP_SPAN
        .saturating_add(Duration::from_millis(600));

/// The statuses the interface of a task that is coming up passes through, in order — three
/// of the values `Attachment$status` lists.
const ATTACH_STATUSES: [&str; 3] = ["PRECREATED", "ATTACHING", "ATTACHED"];

/// The `lastStatus` values a task that is coming up passes through, in order.
const TASK_STATUSES: [&str; 3] = ["PROVISIONING", "PENDING", "RUNNING"];

/// `Task$connectivity` of a task whose networking is in place, from the model's
/// `Connectivity` enum (`smithy_query(query="shape", shape="Connectivity")` -> CONNECTED,
/// DISCONNECTED).
const CONNECTED: &str = "CONNECTED";

/// Register one revision of `family` in `network_mode` with a single container.
async fn register(router: &MultiServiceRouter, family: &str, network_mode: &str, case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": network_mode,
            "containerDefinitions": [container("web", IMAGE)],
        }),
        case,
    )
    .await;
}

/// Read ONE task back through `DescribeTasks` — the stored record, not the answer of the
/// call that created it.
async fn describe(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": [task_arn] }),
        case,
    )
    .await;
    list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected one task back, got {body}"))
}

/// The one attachment an `awsvpc` task holds.
fn only_attachment(task: &Value, case: &str) -> Value {
    let attachments = list(task, "attachments", case);
    assert_eq!(
        attachments.len(),
        1,
        "case {case:?}: expected the one interface an awsvpc task is allocated, got {task}"
    );
    attachments[0].clone()
}

/// One detail of an attachment, by name.
fn detail(attachment: &Value, name: &str) -> Option<String> {
    attachment["details"]
        .as_array()?
        .iter()
        .find(|entry| entry["name"].as_str() == Some(name))
        .and_then(|entry| entry["value"].as_str().map(str::to_string))
}

/// Append `status` to `seen` when it is not the one already at the end of it: the ordered
/// sequence of DISTINCT values a poll observed.
fn observe(seen: &mut Vec<String>, status: &str) {
    if seen.last().map(String::as_str) != Some(status) {
        seen.push(status.to_string());
    }
}

/// Launch one `awsvpc` task of `family` through `path` and return its ARN.
async fn launch(
    router: &MultiServiceRouter,
    path: Launch,
    family: &str,
    case: &str,
) -> String {
    match path {
        Launch::RunTask => {
            let body = ok(
                router,
                "RunTask",
                json!({
                    "cluster": CLUSTER,
                    "taskDefinition": family,
                    "count": 1,
                    "networkConfiguration": awsvpc_network_configuration(),
                }),
                case,
            )
            .await;
            string(&list(&body, "tasks", case)[0], "taskArn", case)
        }
        Launch::StartTask => {
            let host = register_capacity(router, CLUSTER, case).await;
            let body = ok(
                router,
                "StartTask",
                json!({
                    "cluster": CLUSTER,
                    "taskDefinition": family,
                    "containerInstances": [host],
                    "networkConfiguration": awsvpc_network_configuration(),
                }),
                case,
            )
            .await;
            string(&list(&body, "tasks", case)[0], "taskArn", case)
        }
        Launch::CreateService => {
            ok(
                router,
                "CreateService",
                json!({
                    "cluster": CLUSTER,
                    "serviceName": format!("svc{family}"),
                    "taskDefinition": family,
                    "desiredCount": 1,
                    "launchType": "EC2",
                    "networkConfiguration": awsvpc_network_configuration(),
                }),
                case,
            )
            .await;
            let listed = ok(router, "ListTasks", json!({ "cluster": CLUSTER }), case).await;
            let arns = list(&listed, "taskArns", case);
            assert_eq!(
                arns.len(),
                1,
                "case {case:?}: expected the service's one task, got {listed}"
            );
            arns[0]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a task ARN, got {listed}"))
                .to_string()
        }
    }
}

/// Which path placed the task under test. Every one of them allocates the interface out of
/// the network configuration it carries, so the rule has to hold for all three.
#[derive(Debug, Clone, Copy)]
enum Launch {
    RunTask,
    StartTask,
    CreateService,
}

#[tokio::test]
async fn an_awsvpc_task_attaches_its_interface_before_it_runs_and_is_connected_from_then_on() {
    let rows: [(&str, Launch); 3] = [
        ("RunTask", Launch::RunTask),
        ("StartTask", Launch::StartTask),
        ("a service's own scheduler", Launch::CreateService),
    ];

    for (index, (case, path)) in rows.into_iter().enumerate() {
        // A fresh engine per row, so one row's tasks cannot leak into another.
        let router = service();
        cluster_with_capacity(&router, CLUSTER, case).await;
        let family = format!("attachprog{index}");
        register(&router, &family, "awsvpc", case).await;
        let task_arn = launch(&router, path, &family, case).await;

        // The interface is CREATED with the task, so the attachment is there, carries all
        // four details the member documents, and is not attached yet.
        let first = describe(&router, &task_arn, case).await;
        let attachment = only_attachment(&first, case);
        assert_eq!(
            attachment["status"].as_str(),
            Some(ATTACH_STATUSES[0]),
            "case {case:?}: Amazon ECS has only just created the interface, got {attachment}"
        );
        assert_eq!(
            first["lastStatus"].as_str(),
            Some(TASK_STATUSES[0]),
            "case {case:?}: a task whose interface is not attached yet is still being \
             provisioned, got {first}"
        );
        assert!(
            first["connectivity"].is_null() && first["connectivityAt"].is_null(),
            "case {case:?}: a task whose interface is not attached is on no network yet, \
             got {first}"
        );

        // Then the clock, and nothing else, walks both of them on.
        let mut attach_seen: Vec<String> = Vec::new();
        let mut task_seen: Vec<String> = Vec::new();
        let mut connected_at: Option<f64> = None;
        let deadline = Instant::now() + POLL_BUDGET;
        loop {
            let task = describe(&router, &task_arn, case).await;
            let attachment = only_attachment(&task, case);
            let attach_status = string(&attachment, "status", case);
            let task_status = string(&task, "lastStatus", case);
            observe(&mut attach_seen, &attach_status);
            observe(&mut task_seen, &task_status);

            // Every detail of the interface is reported at every stage: they are the
            // interface's own, and it exists from the moment Amazon ECS created it.
            for name in ["subnetId", "networkInterfaceId", "macAddress", "privateIPv4Address"] {
                assert!(
                    detail(&attachment, name).is_some_and(|value| !value.is_empty()),
                    "case {case:?}: the interface reports {name} at every stage, got \
                     {attachment}"
                );
            }

            // Connectivity follows the interface, not the reader: it is reported exactly
            // when the interface is in place, and from then on it never moves.
            match (
                attach_status == "ATTACHED",
                task["connectivity"].as_str(),
                task["connectivityAt"].as_f64(),
            ) {
                (false, connectivity, at) => assert_eq!(
                    (connectivity, at),
                    (None, None),
                    "case {case:?}: a task whose interface reports {attach_status:?} is on \
                     no network yet, got {task}"
                ),
                (true, connectivity, at) => {
                    assert_eq!(
                        connectivity,
                        Some(CONNECTED),
                        "case {case:?}: a task whose interface is attached is CONNECTED, \
                         got {task}"
                    );
                    let at = at.unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: a CONNECTED task carries the instant it went \
                             CONNECTED, got {task}"
                        )
                    });
                    if let Some(first) = connected_at {
                        assert_eq!(
                            at, first,
                            "case {case:?}: the instant a task went CONNECTED is stored, \
                             not minted per read, got {task}"
                        );
                    }
                    connected_at = Some(at);
                }
            }

            // The invariant the whole rule exists for: a task that is RUNNING has its
            // interface in place.
            if task_status == "RUNNING" {
                assert_eq!(
                    attach_status, "ATTACHED",
                    "case {case:?}: a running task's interface is attached, got {task}"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the task to run once its interface was attached; \
                 observed {task_seen:?} and {attach_seen:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        // Each sequence is the documented one, in order and with nothing skipped: the
        // ATTACHING the agent acknowledges is a status a caller can actually observe.
        assert_eq!(
            attach_seen,
            ATTACH_STATUSES.map(String::from).to_vec(),
            "case {case:?}: expected the interface to pass through every status it is \
             documented to take, observed {attach_seen:?}"
        );
        assert_eq!(
            task_seen,
            TASK_STATUSES.map(String::from).to_vec(),
            "case {case:?}: expected the task to pass through the lifecycle in order, \
             observed {task_seen:?}"
        );

        // And the interface was in place BEFORE the task started: one instant read off the
        // task, the other off the same record.
        let running = describe(&router, &task_arn, case).await;
        let started_at = running["startedAt"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: a running task carries its start instant, got {running}")
        });
        let connected_at = connected_at
            .unwrap_or_else(|| panic!("case {case:?}: expected a CONNECTED task, got {running}"));
        assert!(
            connected_at <= started_at,
            "case {case:?}: expected the task on the network before it started, got \
             connectivityAt {connected_at} and startedAt {started_at}"
        );
    }
}

#[tokio::test]
async fn a_task_that_was_allocated_no_interface_is_connected_once_it_runs() {
    // The rows span the network modes that allocate no interface of their own (the model's
    // `NetworkMode` enum less `awsvpc`): such a task shares the networking of the host that
    // runs it, so it holds no attachment and is on the network once it has started there.
    for (index, network_mode) in ["bridge", "host", "none"].into_iter().enumerate() {
        let case = network_mode;
        let router = service();
        cluster_with_capacity(&router, CLUSTER, case).await;
        let family = format!("noiface{index}");
        register(&router, &family, network_mode, case).await;
        let body = ok(
            &router,
            "RunTask",
            json!({ "cluster": CLUSTER, "taskDefinition": family, "count": 1 }),
            case,
        )
        .await;
        let task_arn = string(&list(&body, "tasks", case)[0], "taskArn", case);

        // Such a task is placed straight away, so it is PENDING with only its own start
        // outstanding — and nothing is on a network before it has started.
        let placed = describe(&router, &task_arn, case).await;
        assert_eq!(
            (
                placed["lastStatus"].as_str(),
                list(&placed, "attachments", case).len(),
                placed["connectivity"].as_str(),
            ),
            (Some("PENDING"), 0, None),
            "case {case:?}: expected a placed task with no interface and no connectivity, \
             got {placed}"
        );

        let deadline = Instant::now() + POLL_BUDGET;
        let running = loop {
            let task = describe(&router, &task_arn, case).await;
            if string(&task, "lastStatus", case) == "RUNNING" {
                break task;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the clock to start the task, got {task}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        assert_eq!(
            running["connectivity"].as_str(),
            Some(CONNECTED),
            "case {case:?}: a started task is on the networking of the host that runs it, \
             got {running}"
        );
        assert!(
            running["connectivityAt"].is_number(),
            "case {case:?}: a CONNECTED task carries the instant it went CONNECTED, got \
             {running}"
        );
    }
}

#[tokio::test]
async fn an_agent_report_moves_the_interface_and_amazon_ecs_carries_on_from_there() {
    // The rows span every status `Attachment$status` documents ("Valid values are
    // PRECREATED, CREATED, ATTACHING, ATTACHED, DETACHING, DETACHED, DELETED, and
    // FAILED"). The agent plane accepts ONE of them in a report — `ATTACHED`, the attach the
    // agent completed — so that row is taken and connects the task, and every other row is
    // refused with the out-of-set sentence and leaves the interface where Amazon ECS's own
    // progression had it, for the clock to carry on.
    let statuses = [
        "PRECREATED",
        "CREATED",
        "ATTACHING",
        "ATTACHED",
        "DETACHING",
        "DETACHED",
        "DELETED",
        "FAILED",
    ];

    // Every row is launched and reported on BEFORE the clock is given its time, so one
    // wait settles all of them: each row holds its own engine, so no row's clock is
    // another's.
    let mut reported_rows = Vec::new();
    for (index, reported) in statuses.into_iter().enumerate() {
        let case = reported;
        let router = service();
        cluster_with_capacity(&router, CLUSTER, case).await;
        let family = format!("agentattach{index}");
        register(&router, &family, "awsvpc", case).await;
        let task_arn = launch(&router, Launch::RunTask, &family, case).await;
        let created = only_attachment(&describe(&router, &task_arn, case).await, case);
        let attachment_id = string(&created, "id", case);
        let status_before = string(&created, "status", case);

        let request = json!({
            "cluster": CLUSTER,
            "attachments": [{ "attachmentArn": attachment_id, "status": reported }],
        });
        if reported == "ATTACHED" {
            let acknowledged = ok(
                &router,
                "SubmitAttachmentStateChanges",
                request,
                case,
            )
            .await;
            assert_eq!(
                acknowledged["acknowledgment"].as_str(),
                Some("ACK"),
                "case {case:?}: the agent is told the report was taken, got {acknowledged}"
            );
        } else {
            let (code, message) =
                refusal(&router, "SubmitAttachmentStateChanges", request, case).await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (
                    "InvalidParameterException",
                    "attachmentStatus should be one of [ATTACHED]"
                ),
                "case {case:?}: expected the out-of-set refusal, got {code} with {message:?}"
            );
        }

        let read = describe(&router, &task_arn, case).await;
        let attachment = only_attachment(&read, case);
        let expected_now = if reported == "ATTACHED" {
            "ATTACHED"
        } else {
            status_before.as_str()
        };
        assert_eq!(
            attachment["status"].as_str(),
            Some(expected_now),
            "case {case:?}: expected the interface at {expected_now:?} after the report, got \
             {attachment}"
        );
        assert_eq!(
            read["connectivity"].as_str(),
            (expected_now == "ATTACHED").then_some(CONNECTED),
            "case {case:?}: a task is CONNECTED exactly when its interface is in place, got \
             {read}"
        );
        reported_rows.push((case, router, task_arn));
    }

    // And Amazon ECS carries the progression on from wherever it is: every interface it is
    // still attaching reaches ATTACHED, whether the agent's report was taken or refused.
    tokio::time::sleep(POLL_BUDGET).await;
    for (case, router, task_arn) in &reported_rows {
        let settled = describe(router, task_arn, case).await;
        let attachment = only_attachment(&settled, case);
        assert_eq!(
            attachment["status"].as_str(),
            Some("ATTACHED"),
            "case {case:?}: expected the interface ATTACHED once the clock had run, got \
             {attachment}"
        );
        assert_eq!(
            settled["connectivity"].as_str(),
            Some(CONNECTED),
            "case {case:?}: connectivity follows the interface the task holds, got {settled}"
        );
    }
}

#[tokio::test]
async fn a_task_that_is_not_placed_yet_is_given_no_interface_and_is_on_no_network() {
    // A task whose capacity its provider still has to ACQUIRE has not been placed anywhere
    // ("PROVISIONING: Amazon ECS has to perform additional steps before the task is
    // launched"), so nothing is creating or attaching an interface for it and it is on no
    // network — however long it waits. The attach begins when the placement does: the
    // capacity registering is what starts it, and from there the task passes through the
    // documented statuses like any other.
    let case = "a task parked on an empty managed-scaling provider";
    let router = service();
    create_cluster(&router, CLUSTER, case).await;
    register(&router, "parkediface", "awsvpc", case).await;
    ok(
        &router,
        "CreateCapacityProvider",
        json!({
            "name": "parkedscaled",
            "autoScalingGroupProvider": {
                "autoScalingGroupArn": "asg-parkedscaled",
                "managedScaling": { "status": "ENABLED", "targetCapacity": 100 },
            },
        }),
        case,
    )
    .await;
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": CLUSTER,
            "capacityProviders": ["parkedscaled"],
            "defaultCapacityProviderStrategy": [],
        }),
        case,
    )
    .await;
    let body = ok(
        &router,
        "RunTask",
        json!({
            "cluster": CLUSTER,
            "taskDefinition": "parkediface",
            "capacityProviderStrategy": [{ "capacityProvider": "parkedscaled", "weight": 1 }],
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        case,
    )
    .await;
    let task_arn = string(&list(&body, "tasks", case)[0], "taskArn", case);

    // Time alone attaches nothing: the first step of an attach would have been due long
    // before now, and the interface is still the one Amazon ECS created, with the task on no
    // network.
    tokio::time::sleep(PAST_THE_FIRST_ATTACH_STEP).await;
    let parked = describe(&router, &task_arn, case).await;
    assert_eq!(
        (
            parked["lastStatus"].as_str(),
            only_attachment(&parked, case)["status"].as_str(),
            parked["connectivity"].as_str(),
            parked["connectivityAt"].as_f64(),
        ),
        (
            Some(TASK_STATUSES[0]),
            Some(ATTACH_STATUSES[0]),
            None,
            None
        ),
        "case {case:?}: expected a parked task to hold the interface Amazon ECS created and \
         to be on no network, got {parked}"
    );

    // The capacity arrives, and from there the attach runs: the interface reaches ATTACHED,
    // the task is CONNECTED, and only then does it run.
    register_capacity(&router, CLUSTER, case).await;
    let mut attach_seen: Vec<String> = Vec::new();
    // The placement itself is owed a span before the attach even begins, so this poll waits
    // out one more span than a launch onto capacity that already exists.
    let deadline = Instant::now() + POLL_BUDGET + AWSVPC_START;
    loop {
        let task = describe(&router, &task_arn, case).await;
        let attachment = only_attachment(&task, case);
        observe(&mut attach_seen, &string(&attachment, "status", case));
        if string(&task, "lastStatus", case) == "RUNNING" {
            assert_eq!(
                (
                    attachment["status"].as_str(),
                    task["connectivity"].as_str()
                ),
                (Some("ATTACHED"), Some(CONNECTED)),
                "case {case:?}: a task that ran once its capacity arrived holds an attached \
                 interface and is CONNECTED, got {task}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the registered capacity to place and start the task; \
             observed {attach_seen:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert_eq!(
        attach_seen,
        ATTACH_STATUSES.map(String::from).to_vec(),
        "case {case:?}: expected the attach to begin with the placement and pass through \
         every documented status, observed {attach_seen:?}"
    );
}

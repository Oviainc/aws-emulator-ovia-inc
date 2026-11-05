// SPDX-License-Identifier: CC0-1.0

//! The elastic network interface an `awsvpc` task is allocated, driven end to end over the
//! shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test, asserted over a table that spans the class rather than over one
//! request: `RegisterTaskDefinition$networkMode` — "If the network mode is `awsvpc`, the
//! task is allocated an elastic network interface" — and `Attachment$details`, which "for
//! elastic network interfaces ... includes the network interface ID, the MAC address, the
//! subnet ID, and the private IPv4 address". A task does not run before the interface it
//! was allocated is in place, so a task the clock has brought to `RUNNING` reports that
//! attachment as `ATTACHED` carrying all four details, and the interface's values are its
//! own: two tasks never share an id, a MAC address or an address, and every read of one
//! task reports the same values.
//!
//! The rows differ along the network mode (every value of the model's `NetworkMode` enum),
//! along how many tasks a launch places, and along which read reports the task — the run's
//! own answer, which is taken before the interface is attached, and `DescribeTasks` after
//! the transition the clock owes has fallen due.
//!
//! Nothing here reaches into the state struct: every value asserted is read back through
//! `DescribeTasks`, so a response that is not computed from persisted state cannot pass.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{awsvpc_network_configuration, cluster_with_capacity, list, ok, service, string};
use crate::task_lifecycle::container;

/// The cluster every task below runs in.
const CLUSTER: &str = "taskinterfaces";

/// The image the containers start from; irrelevant to this rule, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// The subnets an `awsvpc` launch configures here, so the reported `subnetId` is one of
/// them.
const SUBNET: &str = "subnet-0123456789abcdef0";

/// Long enough for the two clock-owed task transitions (`PENDING` then `RUNNING`, each
/// `state::TRANSITION_SPAN`) to fall due, with margin for a loaded test machine.
const TWO_SPANS: Duration = Duration::from_millis(
    2 * service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::TRANSITION_SPAN
        .as_millis() as u64
        + 400,
);

/// The `Attachment$details` keys an attached elastic network interface reports, from the
/// member's own doc sentence.
const INTERFACE_DETAILS: [&str; 4] = [
    "networkInterfaceId",
    "macAddress",
    "subnetId",
    "privateIPv4Address",
];

/// One detail of an attachment, by name.
fn detail(attachment: &Value, name: &str, case: &str) -> Option<String> {
    list(attachment, "details", case)
        .into_iter()
        .find(|entry| entry["name"].as_str() == Some(name))
        .and_then(|entry| entry["value"].as_str().map(str::to_string))
}

/// Register one revision of `family` in `network_mode` with a single container.
async fn register(
    router: &MultiServiceRouter,
    family: &str,
    network_mode: &str,
    case: &str,
) {
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

/// Run `count` tasks of `family` and return the ARNs the run reported, after asserting the
/// attachment each task was created with is the one Amazon ECS creates and not yet attached.
async fn run_tasks(
    router: &MultiServiceRouter,
    family: &str,
    network_mode: &str,
    count: i64,
    case: &str,
) -> Vec<String> {
    let mut request = json!({ "cluster": CLUSTER, "taskDefinition": family, "count": count });
    if network_mode == "awsvpc" {
        request["networkConfiguration"] = awsvpc_network_configuration();
    }
    let body = ok(router, "RunTask", request, case).await;
    let tasks = list(&body, "tasks", case);
    assert_eq!(
        tasks.len() as i64,
        count,
        "case {case:?}: expected {count} task(s), got {body}"
    );
    tasks
        .iter()
        .map(|task| string(task, "taskArn", case))
        .collect()
}

#[tokio::test]
async fn an_awsvpc_task_reports_the_interface_it_was_allocated_once_it_runs() {
    // The rows span the `NetworkMode` enum and the count a launch places: only `awsvpc`
    // allocates an interface, and every task it places gets its OWN.
    let rows: [(&str, &str, i64, usize); 5] = [
        ("awsvpc, one task", "awsvpc", 1, 1),
        ("awsvpc, three tasks", "awsvpc", 3, 1),
        ("bridge", "bridge", 1, 0),
        ("host", "host", 1, 0),
        ("none", "none", 1, 0),
    ];

    // Every address, MAC and interface id handed out WITHIN one row, so no two interfaces
    // of one store share one. A row runs on a fresh engine, so uniqueness is a property of
    // one store's allocations.
    for (case, network_mode, count, interfaces_per_task) in rows {
        let mut addresses: BTreeSet<String> = BTreeSet::new();
        let mut interface_ids: BTreeSet<String> = BTreeSet::new();
        let mut mac_addresses: BTreeSet<String> = BTreeSet::new();
        let mut allocated = 0usize;
        // A fresh engine per row, so one row's tasks cannot leak into another.
        let router = service();
        cluster_with_capacity(&router, CLUSTER, case).await;
        let family = format!("taskiface{network_mode}");
        register(&router, &family, network_mode, case).await;
        let arns = run_tasks(&router, &family, network_mode, count, case).await;

        // Amazon ECS CREATES the interface before the task runs, so the attachment already
        // carries the interface's own details; what the task reaching RUNNING moves is the
        // attachment's status.
        for arn in &arns {
            let task = describe(&router, arn, case).await;
            for attachment in list(&task, "attachments", case) {
                assert_eq!(
                    attachment["status"].as_str(),
                    Some("PRECREATED"),
                    "case {case:?}: a task that is not running yet holds the attachment \
                     Amazon ECS created, got {attachment}"
                );
                for name in INTERFACE_DETAILS {
                    assert!(
                        detail(&attachment, name, case).is_some(),
                        "case {case:?}: the created interface reports {name}, got \
                         {attachment}"
                    );
                }
            }
        }

        tokio::time::sleep(TWO_SPANS).await;

        for arn in &arns {
            let task = describe(&router, arn, case).await;
            assert_eq!(
                task["lastStatus"].as_str(),
                Some("RUNNING"),
                "case {case:?}: the clock owes the task its start, got {task}"
            );
            let attachments = list(&task, "attachments", case);
            assert_eq!(
                attachments.len(),
                interfaces_per_task,
                "case {case:?}: expected {interfaces_per_task} attachment(s) for the \
                 {network_mode} network mode, got {task}"
            );
            for attachment in &attachments {
                assert_eq!(
                    attachment["status"].as_str(),
                    Some("ATTACHED"),
                    "case {case:?}: a running task's interface is in place, got {attachment}"
                );
                for name in INTERFACE_DETAILS {
                    let value = detail(attachment, name, case).unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: expected the interface to report {name}, got \
                             {attachment}"
                        )
                    });
                    assert!(
                        !value.is_empty(),
                        "case {case:?}: expected a non-empty {name}, got {attachment}"
                    );
                }
                assert_eq!(
                    detail(attachment, "subnetId", case).as_deref(),
                    Some(SUBNET),
                    "case {case:?}: the interface reports the subnet the launch \
                     configured, got {attachment}"
                );
                let address = detail(attachment, "privateIPv4Address", case)
                    .unwrap_or_else(|| panic!("case {case:?}: expected an address"));
                let octets: Vec<&str> = address.split('.').collect();
                assert_eq!(
                    octets.len(),
                    4,
                    "case {case:?}: expected four dotted octets, got {address:?}"
                );
                assert!(
                    octets
                        .iter()
                        .all(|octet| octet.parse::<u8>().is_ok()),
                    "case {case:?}: every octet of a private IPv4 address is a byte, got \
                     {address:?}"
                );
                let interface_id = detail(attachment, "networkInterfaceId", case)
                    .unwrap_or_else(|| panic!("case {case:?}: expected an interface id"));
                assert!(
                    interface_id.starts_with("eni-"),
                    "case {case:?}: an elastic network interface id starts with eni-, got \
                     {interface_id:?}"
                );
                let mac = detail(attachment, "macAddress", case)
                    .unwrap_or_else(|| panic!("case {case:?}: expected a MAC address"));
                let groups: Vec<&str> = mac.split(':').collect();
                assert!(
                    groups.len() == 6
                        && groups
                            .iter()
                            .all(|group| u8::from_str_radix(group, 16).is_ok()),
                    "case {case:?}: expected six hexadecimal octets, got {mac:?}"
                );

                allocated += 1;
                addresses.insert(address);
                interface_ids.insert(interface_id);
                mac_addresses.insert(mac);

                // The second read reports the SAME interface: the values are stored, not
                // minted per read.
                let again = describe(&router, arn, case).await;
                assert_eq!(
                    list(&again, "attachments", case),
                    attachments,
                    "case {case:?}: two reads of one task report one interface, got {again}"
                );
            }

            // Every container of an awsvpc task reports the interface its task holds.
            for reported in list(&task, "containers", case) {
                let interfaces = list(&reported, "networkInterfaces", case);
                assert_eq!(
                    interfaces.len(),
                    interfaces_per_task,
                    "case {case:?}: a container reports the interfaces its task holds, got \
                     {reported}"
                );
                for interface in &interfaces {
                    assert!(
                        attachments.iter().any(|attachment| {
                            attachment["id"] == interface["attachmentId"]
                                && detail(attachment, "privateIPv4Address", case).as_deref()
                                    == interface["privateIpv4Address"].as_str()
                        }),
                        "case {case:?}: the container's interface is one of the task's \
                         attachments, got {interface} against {attachments:?}"
                    );
                }
            }
        }

        assert_eq!(
            (addresses.len(), interface_ids.len(), mac_addresses.len()),
            (allocated, allocated, allocated),
            "case {case:?}: every interface allocated gets its own address, id and MAC: \
             {allocated} allocated, {} address(es), {} id(s), {} MAC(s)",
            addresses.len(),
            interface_ids.len(),
            mac_addresses.len()
        );
    }
}

/// Read ONE task back through `DescribeTasks` — the stored record, not the answer of the
/// call that changed it.
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

// SPDX-License-Identifier: CC0-1.0

//! WHICH networking the elastic network interface an `awsvpc` task is allocated sits in,
//! and what the interface's OWN identity is — driven end to end over the shared wire
//! harness in [`crate::ecs_wire`].
//!
//! The rule under test, asserted over a table that spans the class rather than over one
//! request: `RegisterTaskDefinition$networkMode` — "If the network mode is `awsvpc`, the
//! task is allocated an elastic network interface" — and `Attachment$details`, which "for
//! elastic network interfaces ... includes the network interface ID, the MAC address, the
//! subnet ID, and the private IPv4 address". That interface is allocated out of the
//! networking the LAUNCH named, so the `subnetId` it reports is one of the subnets that
//! launch configured and never a value of the service's own choosing, while the other
//! three details are the INTERFACE's: a well-formed interface id, a unicast locally
//! administered MAC address and a private IPv4 address, each belonging to exactly one
//! interface of the account and unchanged by anything that moves the attachment's STATUS —
//! the clock taking the interface to `ATTACHED`, or an agent acknowledging a status with
//! `SubmitAttachmentStateChanges` ("Sent to acknowledge that an attachment changed
//! states").
//!
//! Member sources: `smithy_query(query="shape", shape="AwsVpcConfiguration")` — `subnets`
//! is "The IDs of the subnets associated with the task or service", required, "with a
//! limit of 16 subnets" — and `smithy_query(query="doc", shape="Attachment",
//! member="details")` for the four details quoted above.
//!
//! The rows differ along how many subnets the launch names (one, two, three, and the
//! documented maximum of sixteen), along WHICH ids those subnets are, along how many tasks
//! the launch places, and along the path that placed them (`RunTask`, `StartTask`, a
//! service's own scheduler) — every row in ONE store, so "no two interfaces share an
//! identity" is asserted across every interface the account holds rather than within one
//! launch.
//!
//! Nothing here reaches into the state struct: every value asserted is read back through
//! `DescribeTasks`, so a response that is not computed from persisted state cannot pass.

use std::collections::BTreeSet;
use std::net::Ipv4Addr;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    create_cluster, list, ok, register_capacity, service, string, AWSVPC_START,
};
use crate::task_lifecycle::container;

/// The cluster every task below runs in.
const CLUSTER: &str = "ifacesubnets";

/// The image the containers start from; irrelevant to this rule, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// The security group every launch below configures; irrelevant to this rule, so one
/// value.
const SECURITY_GROUP: &str = "sg-0123456789abcdef0";

/// The `Attachment$details` key the subnet the launch named is reported under.
const SUBNET_DETAIL: &str = "subnetId";

/// The `Attachment$details` keys that are the interface's OWN identity, as opposed to the
/// subnet, which comes off the launch.
const IDENTITY_DETAILS: [&str; 3] = ["networkInterfaceId", "macAddress", "privateIPv4Address"];

/// Which path placed the tasks of a row. All three allocate the interface out of the
/// network configuration the placement runs under, so the rule has to hold for each.
#[derive(Debug, Clone, Copy)]
enum Launch {
    RunTask,
    StartTask,
    Scheduler,
}

/// One row of the table: how the tasks were placed, which subnets the launch named, and
/// how many tasks it placed.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    launch: Launch,
    /// The counter the row's subnet ids start at, so no two rows name the same subnet.
    first_subnet: u64,
    /// How many subnets the launch names (`subnets` is required, so never none).
    subnets: usize,
    /// How many tasks the launch places.
    tasks: usize,
}

/// The rows. `StartTask` places one task per container instance it names, so the row that
/// goes through it places one.
const ROWS: [Row; 5] = [
    Row {
        case: "RunTask naming one subnet",
        launch: Launch::RunTask,
        first_subnet: 0x10,
        subnets: 1,
        tasks: 1,
    },
    Row {
        case: "RunTask naming two subnets, placing two tasks",
        launch: Launch::RunTask,
        first_subnet: 0x20,
        subnets: 2,
        tasks: 2,
    },
    Row {
        case: "RunTask naming the documented maximum of sixteen subnets",
        launch: Launch::RunTask,
        first_subnet: 0x30,
        subnets: 16,
        tasks: 1,
    },
    Row {
        case: "StartTask naming one subnet",
        launch: Launch::StartTask,
        first_subnet: 0x40,
        subnets: 1,
        tasks: 1,
    },
    Row {
        case: "a service's own scheduler, three subnets, two tasks",
        launch: Launch::Scheduler,
        first_subnet: 0x50,
        subnets: 3,
        tasks: 2,
    },
];

/// One task a row placed, beside the identity its interface reported the moment Amazon
/// ECS created it — what every later read of that task is compared against.
#[derive(Debug, Clone)]
struct PlacedTask {
    task_arn: String,
    created_identity: Vec<String>,
}

/// What one row of the table placed: the row itself, the subnets its launch named, and the
/// tasks that launch produced.
#[derive(Debug, Clone)]
struct Placed {
    row: Row,
    subnets: Vec<String>,
    tasks: Vec<PlacedTask>,
}

/// EC2-shaped subnet ids, distinct per row: `subnet-` and 17 hexadecimal digits, the form
/// a subnet id takes. ECS resolves none of them — it allocates the task's interface out of
/// the configuration it was given — so they only have to be well shaped and distinct.
fn subnet_ids(first: u64, count: usize) -> Vec<String> {
    (0..count as u64)
        .map(|index| format!("subnet-{:017x}", first + index))
        .collect()
}

/// The `networkConfiguration` a launch of an `awsvpc` task definition carries.
fn network_configuration(subnets: &[String]) -> Value {
    json!({
        "awsvpcConfiguration": {
            "subnets": subnets,
            "securityGroups": [SECURITY_GROUP],
        }
    })
}

/// Register one `awsvpc` revision of `family` with a single container.
async fn register(router: &MultiServiceRouter, family: &str, case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "containerDefinitions": [container("web", IMAGE)],
        }),
        case,
    )
    .await;
}

/// Read ONE task back through `DescribeTasks` — the stored record, not the answer of the
/// call that placed it.
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
    assert_eq!(
        attachments[0]["type"].as_str(),
        Some("ElasticNetworkInterface"),
        "case {case:?}: the interface a task is allocated is an elastic network interface, \
         got {task}"
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

/// The interface's own identity, as the attachment reports it: the three
/// [`IDENTITY_DETAILS`] in that order. Panics on a detail that is missing, which is itself
/// the defect `Attachment$details` describes.
fn identity(attachment: &Value, case: &str) -> Vec<String> {
    IDENTITY_DETAILS
        .iter()
        .map(|name| {
            detail(attachment, name).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the created interface reports {name}, got {attachment}"
                )
            })
        })
        .collect()
}

/// Assert that the identity the attachment reports is one an interface could really have.
///
/// * The id is `eni-` and the published number of hexadecimal digits for an EC2 resource
///   id (8 for the short form, 17 for the long one), lowercase.
/// * The MAC address is six hexadecimal octets whose first has the IEEE 802 group bit
///   clear (a unicast address, not a multicast one) and its local bit set (a locally
///   administered address, which is what a virtualised interface is given).
/// * The private IPv4 address is a dotted-quad address in one of the RFC 1918 private
///   ranges — the only addresses a VPC subnet hands out.
fn assert_well_formed(attachment: &Value, case: &str) {
    let values = identity(attachment, case);
    let (interface_id, mac, address) = (&values[0], &values[1], &values[2]);

    let digits = interface_id.strip_prefix("eni-").unwrap_or_default();
    assert!(
        (digits.len() == 8 || digits.len() == 17)
            && digits
                .chars()
                .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase()),
        "case {case:?}: expected an elastic network interface id (eni- and 8 or 17 \
         lowercase hexadecimal digits), got {interface_id:?}"
    );

    let groups: Vec<&str> = mac.split(':').collect();
    assert_eq!(
        groups.len(),
        6,
        "case {case:?}: expected six colon-separated octets in a MAC address, got {mac:?}"
    );
    let octets: Vec<u8> = groups
        .iter()
        .map(|group| {
            assert!(
                group.len() == 2 && group.chars().all(|digit| digit.is_ascii_hexdigit()),
                "case {case:?}: expected each octet of a MAC address to be two hexadecimal \
                 digits, got {group:?} in {mac:?}"
            );
            u8::from_str_radix(group, 16).unwrap_or_default()
        })
        .collect();
    assert_eq!(
        (octets[0] & 0x01, octets[0] & 0x02),
        (0x00, 0x02),
        "case {case:?}: expected a unicast, locally administered MAC address, got {mac:?}"
    );

    let parsed: Ipv4Addr = address.parse().unwrap_or_else(|_| {
        panic!("case {case:?}: expected a dotted-quad IPv4 address, got {address:?}")
    });
    assert!(
        parsed.is_private(),
        "case {case:?}: expected a private IPv4 address, got {address:?}"
    );
}

/// Place the tasks of one row and return their ARNs.
async fn launch(
    router: &MultiServiceRouter,
    row: &Row,
    family: &str,
    host: &str,
    subnets: &[String],
) -> Vec<String> {
    let case = row.case;
    let configuration = network_configuration(subnets);
    let arns: Vec<String> = match row.launch {
        Launch::RunTask => {
            let body = ok(
                router,
                "RunTask",
                json!({
                    "cluster": CLUSTER,
                    "taskDefinition": family,
                    "count": row.tasks,
                    "networkConfiguration": configuration,
                }),
                case,
            )
            .await;
            list(&body, "tasks", case)
                .iter()
                .map(|task| string(task, "taskArn", case))
                .collect()
        }
        Launch::StartTask => {
            let body = ok(
                router,
                "StartTask",
                json!({
                    "cluster": CLUSTER,
                    "taskDefinition": family,
                    "containerInstances": [host],
                    "networkConfiguration": configuration,
                }),
                case,
            )
            .await;
            list(&body, "tasks", case)
                .iter()
                .map(|task| string(task, "taskArn", case))
                .collect()
        }
        Launch::Scheduler => {
            ok(
                router,
                "CreateService",
                json!({
                    "cluster": CLUSTER,
                    "serviceName": format!("svc{family}"),
                    "taskDefinition": family,
                    "desiredCount": row.tasks,
                    "launchType": "EC2",
                    "networkConfiguration": configuration,
                }),
                case,
            )
            .await;
            // The tasks the scheduler placed, read back through the listing filtered to
            // this row's own family — one store holds every row's tasks.
            let listed = ok(
                router,
                "ListTasks",
                json!({ "cluster": CLUSTER, "family": family }),
                case,
            )
            .await;
            list(&listed, "taskArns", case)
                .iter()
                .map(|arn| {
                    arn.as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: expected a task ARN, got {arn}"))
                        .to_string()
                })
                .collect()
        }
    };
    assert_eq!(
        arns.len(),
        row.tasks,
        "case {case:?}: expected {} task(s) to be placed, got {arns:?}",
        row.tasks
    );
    arns
}

#[tokio::test]
async fn an_awsvpc_interface_sits_in_a_configured_subnet_and_keeps_its_own_identity() {
    let router = service();
    create_cluster(&router, CLUSTER, "setup").await;
    let host = register_capacity(&router, CLUSTER, "setup").await;

    // Every row is placed BEFORE the clock is given its time, so one wait settles all of
    // them — and every interface of every row is allocated out of the one store, which is
    // what makes the uniqueness assertion below about the account rather than one launch.
    let mut placed: Vec<Placed> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let family = format!("ifacesub{index}");
        register(&router, &family, case).await;
        let subnets = subnet_ids(row.first_subnet, row.subnets);
        let arns = launch(&router, row, &family, &host, &subnets).await;

        // Amazon ECS CREATES the interface with the task, so the identity is already
        // reported and already well formed, before anything has attached it.
        let mut created = Vec::new();
        for arn in &arns {
            let task = describe(&router, arn, case).await;
            let attachment = only_attachment(&task, case);
            assert_well_formed(&attachment, case);
            let subnet = detail(&attachment, SUBNET_DETAIL).unwrap_or_else(|| {
                panic!("case {case:?}: the interface reports its subnet, got {attachment}")
            });
            assert!(
                subnets.contains(&subnet),
                "case {case:?}: expected the interface in one of the subnets the launch \
                 named ({subnets:?}), got {subnet:?}"
            );
            created.push(PlacedTask {
                task_arn: arn.clone(),
                created_identity: identity(&attachment, case),
            });
        }
        placed.push(Placed {
            row: *row,
            subnets,
            tasks: created,
        });
    }

    tokio::time::sleep(AWSVPC_START).await;

    // Every address, MAC and interface id the store handed out, so no two interfaces of
    // the account share one.
    let mut handed_out: [BTreeSet<String>; 3] =
        [BTreeSet::new(), BTreeSet::new(), BTreeSet::new()];
    let mut allocated = 0usize;

    for Placed { row, subnets, tasks } in &placed {
        let case = row.case;
        for PlacedTask {
            task_arn,
            created_identity: before,
        } in tasks
        {
            let task = describe(&router, task_arn, case).await;
            let attachment = only_attachment(&task, case);
            assert_eq!(
                (
                    task["lastStatus"].as_str(),
                    attachment["status"].as_str(),
                    task["connectivity"].as_str(),
                ),
                (Some("RUNNING"), Some("ATTACHED"), Some("CONNECTED")),
                "case {case:?}: expected the clock to attach the interface and run the \
                 task, got {task}"
            );

            // Attaching the interface moved its attachment's STATUS and nothing else: the
            // subnet is still one the launch named and the identity is the one the created
            // interface already reported.
            let subnet = detail(&attachment, SUBNET_DETAIL).unwrap_or_else(|| {
                panic!("case {case:?}: the attached interface reports its subnet, got {attachment}")
            });
            assert!(
                subnets.contains(&subnet),
                "case {case:?}: expected the attached interface in one of the subnets the \
                 launch named ({subnets:?}), got {subnet:?}"
            );
            let after = identity(&attachment, case);
            assert_eq!(
                &after, before,
                "case {case:?}: expected the interface to keep the identity it was created \
                 with, got {after:?} against {before:?}"
            );
            assert_well_formed(&attachment, case);

            allocated += 1;
            for (seen, value) in handed_out.iter_mut().zip(after) {
                seen.insert(value);
            }
        }
    }

    for (name, seen) in IDENTITY_DETAILS.iter().zip(&handed_out) {
        assert_eq!(
            seen.len(),
            allocated,
            "detail {name:?}: expected each of the {allocated} interfaces allocated to have \
             its own {name}, got {} distinct value(s): {seen:?}",
            seen.len()
        );
    }
}

#[tokio::test]
async fn an_agent_report_moves_the_status_and_leaves_the_interface_identity_alone() {
    // The rows span statuses from every part of `Attachment$status`'s own list ("Valid
    // values are PRECREATED, CREATED, ATTACHING, ATTACHED, DETACHING, DETACHED, DELETED,
    // and FAILED"): one on the progression Amazon ECS takes an interface through, one past
    // the end of it, and one that ends it. Whichever the agent reports, the interface it
    // reports on keeps the id, the MAC address and the address it was created with — those
    // belong to the interface, not to the state of its attachment.
    let reported_statuses = ["CREATED", "ATTACHED", "DETACHED", "FAILED"];

    let router = service();
    create_cluster(&router, CLUSTER, "setup").await;
    register_capacity(&router, CLUSTER, "setup").await;

    for (index, reported) in reported_statuses.into_iter().enumerate() {
        let case = reported;
        let family = format!("ifaceack{index}");
        register(&router, &family, case).await;
        let subnets = subnet_ids(0x100 + index as u64 * 0x10, 2);
        let body = ok(
            &router,
            "RunTask",
            json!({
                "cluster": CLUSTER,
                "taskDefinition": family,
                "count": 1,
                "networkConfiguration": network_configuration(&subnets),
            }),
            case,
        )
        .await;
        let task_arn = string(&list(&body, "tasks", case)[0], "taskArn", case);

        let created = only_attachment(&describe(&router, &task_arn, case).await, case);
        let before = identity(&created, case);
        let subnet_before = detail(&created, SUBNET_DETAIL);
        let attachment_id = string(&created, "id", case);

        ok(
            &router,
            "SubmitAttachmentStateChanges",
            json!({
                "cluster": CLUSTER,
                "attachments": [{ "attachmentArn": attachment_id, "status": reported }],
            }),
            case,
        )
        .await;

        let acknowledged = only_attachment(&describe(&router, &task_arn, case).await, case);
        assert_eq!(
            acknowledged["status"].as_str(),
            Some(reported),
            "case {case:?}: the record holds the status the agent reported, got \
             {acknowledged}"
        );
        assert_eq!(
            identity(&acknowledged, case),
            before,
            "case {case:?}: expected the reported-on interface to keep its own identity, \
             got {acknowledged}"
        );
        assert_eq!(
            detail(&acknowledged, SUBNET_DETAIL),
            subnet_before,
            "case {case:?}: expected the reported-on interface to stay in its subnet, got \
             {acknowledged}"
        );
        assert_well_formed(&acknowledged, case);
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetVirtualMachine`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter` — the harness of `gateway_lifecycle` — so what is asserted is the
//! wire behaviour of the operation.
//!
//! The rule under test: `GetVirtualMachine` resolves the virtual machine its `ResourceArn`
//! names out of the persisted store and reports THAT record's members, so every member it
//! answers with is the one the store holds for that machine (and the `VmwareTags` only a
//! read of a single machine carries), an ARN the store holds no machine under is refused
//! with the modelled `ResourceNotFoundException`, and the read changes nothing a later
//! caller sees.
//!
//! A virtual machine is not created by any operation of this model: it is part of the
//! inventory of the hypervisor it runs on, which reaches the service out of band
//! (`state::record_virtual_machine`) — the same out-of-band channel
//! `virtual_machine_listing` uses. The tables below span the class the rule generalises
//! over: machines differing in name length (each published bound of the modelled `Name`),
//! case, host name, path form and length (each published bound of the modelled `Path`),
//! whether they have ever been backed up, and how many VMware tags they carry and which
//! members those tags state; machines on more than one hypervisor; and ARNs differing in
//! resource type, partition, id and length, including each published bound of the modelled
//! `ResourceArn` and one past it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{service_state, VirtualMachineInventory, VmwareTag};

use crate::gateway_lifecycle::{build_engine, build_router, call, create, Reply, REGION};

/// A `LastBackupDate` to report: a fixed instant, so an assertion is about the value the
/// store holds rather than about the clock.
const LAST_BACKUP_EPOCH_SECS: u64 = 1_700_000_000;

/// An ARN of this service naming `resource_type`/`id`, in the region the harness signs
/// for.
fn arn_of(resource_type: &str, id: &str) -> String {
    format!(
        "arn:aws:backup-gateway:{REGION}:{}:{resource_type}/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A virtual-machine ARN in `partition`, so the rows span every partition the modelled
/// `ResourceArn` pattern accepts (`aws`, `aws-cn`, `aws-us-gov`).
fn arn_in_partition(partition: &str, id: &str) -> String {
    format!(
        "arn:{partition}:backup-gateway:{REGION}:{}:vm/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A `ResourceArn`-shaped ARN of exactly `len` characters, for the published length bounds
/// of the modelled shape: `@length(min: 50, max: 500)`
/// (`smithy_query(query="shape",shape="VirtualMachineDetails")`, member `ResourceArn`).
/// The id segment absorbs the length, so every character stays inside the pattern's
/// `[a-zA-Z-0-9]+`.
fn arn_of_length(len: usize) -> String {
    let prefix = format!(
        "arn:aws:backup-gateway:{REGION}:{}:v/",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert!(
        len > prefix.len(),
        "a {len}-character ARN cannot hold the {}-character prefix",
        prefix.len()
    );
    format!("{prefix}{}", "x".repeat(len - prefix.len()))
}

/// The refusal message, wherever the envelope carries it: a refusal the framework made
/// before the handler ran states it in `message`, a modelled error the handler returned in
/// the `Message` member.
fn message_of(reply: &Reply) -> String {
    reply
        .str_at("/message")
        .or_else(|| reply.str_at("/Message"))
        .unwrap_or_else(|| panic!("no message in {:?}", reply.body))
        .to_string()
}

/// One modelled `VmwareTag` (`smithy_query(query="shape",shape="VmwareTag")`:
/// `VmwareCategory` len 1..80, `VmwareTagName` len 1..80, `VmwareTagDescription` a plain
/// string — none of the three required, so a tag may state any subset of them).
fn vmware_tag(
    category: Option<&str>,
    tag_name: Option<&str>,
    description: Option<&str>,
) -> VmwareTag {
    VmwareTag {
        vmware_category: category.map(str::to_string),
        vmware_tag_name: tag_name.map(str::to_string),
        vmware_tag_description: description.map(str::to_string),
    }
}

/// The `VmwareTags` list as the awsJson wire must carry it, in the order the inventory
/// reported the tags: one object per tag, holding exactly the members that tag states.
fn expected_tags(tags: &[VmwareTag]) -> Value {
    Value::Array(
        tags.iter()
            .map(|tag| {
                let mut object = Map::new();
                if let Some(category) = &tag.vmware_category {
                    object.insert("VmwareCategory".to_string(), json!(category));
                }
                if let Some(name) = &tag.vmware_tag_name {
                    object.insert("VmwareTagName".to_string(), json!(name));
                }
                if let Some(description) = &tag.vmware_tag_description {
                    object.insert("VmwareTagDescription".to_string(), json!(description));
                }
                Value::Object(object)
            })
            .collect(),
    )
}

fn backed_up_at(secs: u64) -> Option<SystemTime> {
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

/// One virtual machine of a hypervisor's inventory.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    /// `VirtualMachineDetails$Name`: len 1..100, pattern `^[a-zA-Z0-9-]*$`.
    name: String,
    /// `VirtualMachineDetails$HostName`: the same `Name` shape.
    host_name: String,
    /// `VirtualMachineDetails$Path`: len 1..4096, pattern `^[^\x00]+$` — the machine's
    /// place in its hypervisor's inventory, and its identity there.
    path: String,
    /// `VirtualMachineDetails$LastBackupDate`, absent for a machine never backed up.
    last_backup_date: Option<SystemTime>,
    /// `VirtualMachineDetails$VmwareTags` — the member a listing's `VirtualMachine`
    /// summary does not carry.
    vmware_tags: Vec<VmwareTag>,
}

/// The machines the tests report for the FIRST hypervisor: rows differing along
/// everything a read generalises over — name at the modelled 1-character minimum and
/// 100-character maximum, mixed case and digits, host name equal to and different from the
/// name, path depth, spaces and non-ASCII characters, the 1-character minimum and the
/// 4096-character maximum of `Path`, whether the machine has ever been backed up, and how
/// many VMware tags it carries (none, one, several) and which of a tag's three optional
/// members it states.
fn first_inventory() -> Vec<Row> {
    vec![
        Row {
            case: "a single-character name, a one-character path, never backed up, untagged",
            name: "v".to_string(),
            host_name: "v".to_string(),
            path: "/".to_string(),
            last_backup_date: None,
            vmware_tags: Vec::new(),
        },
        Row {
            case: "a nested path, a backup date and one fully stated tag",
            name: "vm-app-01".to_string(),
            host_name: "vm-app-01-host".to_string(),
            path: "/Datacenter/vm/production/vm-app-01".to_string(),
            last_backup_date: backed_up_at(LAST_BACKUP_EPOCH_SECS),
            vmware_tags: vec![vmware_tag(
                Some("Environment"),
                Some("Production"),
                Some("The production estate"),
            )],
        },
        Row {
            case: "mixed case and digits, a path with spaces and three tags",
            name: "VmMixedCase99".to_string(),
            host_name: "VmMixedCase99".to_string(),
            path: "/Datacenter 2/vm folder/Vm Mixed Case 99".to_string(),
            last_backup_date: backed_up_at(LAST_BACKUP_EPOCH_SECS + 86_400),
            vmware_tags: vec![
                vmware_tag(Some("Environment"), Some("Staging"), None),
                vmware_tag(Some("Owner"), Some("platform-team"), Some("Owning team")),
                vmware_tag(Some("Tier"), Some("gold"), Some("Backup tier gold")),
            ],
        },
        Row {
            case: "a name at the modelled 100-character maximum, a tag stating only its name",
            name: "v".repeat(100),
            host_name: "v".repeat(100),
            path: "/Datacenter/vm/long-name".to_string(),
            last_backup_date: None,
            vmware_tags: vec![vmware_tag(None, Some("nameless-category"), None)],
        },
        Row {
            case: "a path with non-ASCII characters and a tag stating only its category",
            name: "vm-unicode".to_string(),
            host_name: "vm-unicode-host".to_string(),
            path: "/Rechenzentrum/вм/ünïcödé-vm".to_string(),
            last_backup_date: backed_up_at(1),
            vmware_tags: vec![vmware_tag(Some("Standort"), None, None)],
        },
        Row {
            case: "a path at the modelled 4096-character maximum, a tag at the 80-character \
                   category maximum",
            name: "vm-long-path".to_string(),
            host_name: "vm-long-path".to_string(),
            path: format!("/{}", "p".repeat(4095)),
            last_backup_date: backed_up_at(LAST_BACKUP_EPOCH_SECS - 1),
            vmware_tags: vec![vmware_tag(
                Some(&"c".repeat(80)),
                Some(&"t".repeat(80)),
                Some("A description with spaces, punctuation — and non-ASCII"),
            )],
        },
    ]
}

/// The machines of the SECOND hypervisor, including a path another hypervisor also has: a
/// path identifies a machine ON its hypervisor, so two hypervisors may each hold one, and
/// each is its own record with its own ARN and its own tags.
fn second_inventory() -> Vec<Row> {
    vec![
        Row {
            case: "a machine at a path the first hypervisor also has",
            name: "vm-app-01".to_string(),
            host_name: "vm-app-01-standby".to_string(),
            path: "/Datacenter/vm/production/vm-app-01".to_string(),
            last_backup_date: None,
            vmware_tags: vec![vmware_tag(Some("Environment"), Some("Standby"), None)],
        },
        Row {
            case: "a second machine on the second hypervisor, untagged",
            name: "vm-db-02".to_string(),
            host_name: "vm-db-02".to_string(),
            path: "/Datacenter/vm/databases/vm-db-02".to_string(),
            last_backup_date: backed_up_at(LAST_BACKUP_EPOCH_SECS + 3_600),
            vmware_tags: Vec::new(),
        },
    ]
}

/// Import a hypervisor through `ImportHypervisorConfiguration` and hand back the ARN the
/// service minted. A hypervisor record stands for the machine at its host, so every call
/// names a host of its own.
async fn import_hypervisor(router: &MultiServiceRouter, name: &str, host: &str) -> String {
    let reply = call(
        router,
        "ImportHypervisorConfiguration",
        &json!({ "Name": name, "Host": host }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "ImportHypervisorConfiguration({name:?}) must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
        .to_string()
}

/// Report `rows` as the inventory of `hypervisor_arn`, and hand back the ARNs the store
/// now holds them under, in row order.
///
/// On AWS a virtual machine reaches the service from the hypervisor it runs on, when that
/// hypervisor's metadata is synced — no operation of this model creates one — so the
/// store's inventory is the only place a machine can come from, and a test that wants one
/// read back puts it there, exactly as `GetVirtualMachine` reads it.
fn record_inventory(
    engine: &Arc<engine::InMemoryEngine>,
    hypervisor_arn: &str,
    rows: &[Row],
) -> Vec<String> {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    rows.iter()
        .map(|row| {
            store
                .record_virtual_machine(
                    REGION,
                    hypervisor_arn,
                    VirtualMachineInventory {
                        name: row.name.clone(),
                        host_name: row.host_name.clone(),
                        path: row.path.clone(),
                        last_backup_date: row.last_backup_date,
                        vmware_tags: row.vmware_tags.clone(),
                    },
                    SystemTime::now(),
                )
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the store must record a machine on an imported hypervisor",
                        row.case
                    )
                })
                .arn
        })
        .collect()
}

/// How many virtual machines the store holds.
fn stored_machine_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let store = state.lock();
    store.virtual_machine_count()
}

/// The `HypervisorId` a machine on `hypervisor_arn` must report: the ARN's resource id,
/// read here out of the ARN the import handed back rather than out of the crate.
fn hypervisor_id(hypervisor_arn: &str) -> String {
    hypervisor_arn
        .rsplit('/')
        .next()
        .expect("a hypervisor ARN carries a resource id")
        .to_string()
}

async fn get_machine(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(router, "GetVirtualMachine", &json!({ "ResourceArn": arn })).await
}

/// Assert that `reply` reports exactly the machine `row` was recorded as, under `arn`, on
/// the hypervisor `hypervisor_arn`.
fn assert_reports_row(reply: &Reply, case: &str, row: &Row, arn: &str, hypervisor_arn: &str) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: GetVirtualMachine must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    let member = |name: &str| {
        reply
            .body
            .pointer(&format!("/VirtualMachine/{name}"))
            .cloned()
    };
    assert_eq!(
        member("ResourceArn"),
        Some(json!(arn)),
        "case {case:?}: the read must report the ARN it resolved, got {:?}",
        member("ResourceArn")
    );
    assert_eq!(
        member("Name"),
        Some(json!(row.name)),
        "case {case:?}: the read must report the stored Name, got {:?}",
        member("Name")
    );
    assert_eq!(
        member("HostName"),
        Some(json!(row.host_name)),
        "case {case:?}: the read must report the stored HostName, got {:?}",
        member("HostName")
    );
    assert_eq!(
        member("Path"),
        Some(json!(row.path)),
        "case {case:?}: the read must report the stored Path, got {:?}",
        member("Path")
    );
    assert_eq!(
        member("HypervisorId"),
        Some(json!(hypervisor_id(hypervisor_arn))),
        "case {case:?}: the read must report the id of the hypervisor the machine is on, \
         got {:?}",
        member("HypervisorId")
    );
    let expected_backup_date = row.last_backup_date.map(|date| {
        json!(date
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the rows' backup dates are after the epoch")
            .as_secs())
    });
    assert_eq!(
        member("LastBackupDate").map(|value| json!(value.as_f64().unwrap_or_default() as u64)),
        expected_backup_date,
        "case {case:?}: the read must report the stored LastBackupDate, got {:?}",
        member("LastBackupDate")
    );
    assert_eq!(
        member("VmwareTags"),
        Some(expected_tags(&row.vmware_tags)),
        "case {case:?}: the read must report the stored VmwareTags in order, got {:?}",
        member("VmwareTags")
    );
}

/// Every machine the store holds is read back by its own ARN, with exactly the members the
/// inventory recorded for it — whatever its name, host name, path, backup date or VMware
/// tags, and whichever hypervisor it is on.
#[tokio::test]
async fn every_recorded_machine_is_read_back_by_its_own_arn() {
    let engine = build_engine();
    let router = build_router(&engine);
    let first = import_hypervisor(&router, "hv-first", "198.51.100.1").await;
    let second = import_hypervisor(&router, "hv-second", "198.51.100.2").await;

    let first_rows = first_inventory();
    let second_rows = second_inventory();
    let first_arns = record_inventory(&engine, &first, &first_rows);
    let second_arns = record_inventory(&engine, &second, &second_rows);

    for (hypervisor_arn, rows, arns) in [
        (&first, &first_rows, &first_arns),
        (&second, &second_rows, &second_arns),
    ] {
        for (row, arn) in rows.iter().zip(arns) {
            let reply = get_machine(&router, arn).await;
            assert_reports_row(&reply, row.case, row, arn, hypervisor_arn);
        }
    }

    // Two hypervisors hold a machine at the SAME path, and each read reports its own
    // record: one ARN per machine, never one record serving both.
    let shared_path = &second_rows[0].path;
    let on_first = first_rows
        .iter()
        .position(|row| &row.path == shared_path)
        .expect("the first inventory holds the shared path");
    assert_ne!(
        first_arns[on_first], second_arns[0],
        "a path two hypervisors each hold must be two records, got one ARN {:?}",
        first_arns[on_first]
    );
    let from_first = get_machine(&router, &first_arns[on_first]).await;
    let from_second = get_machine(&router, &second_arns[0]).await;
    assert_ne!(
        from_first.body.pointer("/VirtualMachine/HypervisorId"),
        from_second.body.pointer("/VirtualMachine/HypervisorId"),
        "two machines at one path must each report their own hypervisor, got {:?} and {:?}",
        from_first.body,
        from_second.body
    );
}

/// A read of one machine and the listing that holds it agree about every member they both
/// carry: both are built from the one stored record, and only the read adds the
/// `VmwareTags` the `VirtualMachine` summary does not declare.
#[tokio::test]
async fn a_read_agrees_with_the_listing_about_every_machine() {
    let engine = build_engine();
    let router = build_router(&engine);
    let hypervisor = import_hypervisor(&router, "hv-agree", "198.51.100.3").await;
    let rows = first_inventory();
    let arns = record_inventory(&engine, &hypervisor, &rows);

    let listed = call(
        &router,
        "ListVirtualMachines",
        &json!({ "HypervisorArn": hypervisor, "MaxResults": rows.len() }),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "ListVirtualMachines must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    let summaries: BTreeMap<String, Value> = listed
        .body
        .get("VirtualMachines")
        .and_then(Value::as_array)
        .expect("the listing must carry VirtualMachines")
        .iter()
        .map(|machine| {
            (
                machine["ResourceArn"].as_str().unwrap_or_default().to_string(),
                machine.clone(),
            )
        })
        .collect();

    for (row, arn) in rows.iter().zip(&arns) {
        let read = get_machine(&router, arn).await;
        let details = read
            .body
            .pointer("/VirtualMachine")
            .unwrap_or_else(|| panic!("case {:?}: no VirtualMachine in {:?}", row.case, read.body));
        let summary = summaries
            .get(arn)
            .unwrap_or_else(|| panic!("case {:?}: the listing must hold {arn}", row.case));
        for member in [
            "ResourceArn",
            "Name",
            "HostName",
            "Path",
            "HypervisorId",
            "LastBackupDate",
        ] {
            assert_eq!(
                details.get(member),
                summary.get(member),
                "case {:?}: a read and a listing must agree about {member}, got {:?} and {:?}",
                row.case,
                details.get(member),
                summary.get(member)
            );
        }
        assert_eq!(
            summary.get("VmwareTags"),
            None,
            "case {:?}: the listing's VirtualMachine summary declares no VmwareTags, got {:?}",
            row.case,
            summary.get("VmwareTags")
        );
        assert_eq!(
            details.get("VmwareTags"),
            Some(&expected_tags(&row.vmware_tags)),
            "case {:?}: only the read carries VmwareTags, got {:?}",
            row.case,
            details.get("VmwareTags")
        );
    }
}

/// A machine re-reported by its hypervisor's inventory keeps its ARN and is read back as
/// the inventory NOW describes it: the read answers from the record, so a changed record
/// changes the answer and nothing is fixed at the moment the machine was discovered.
#[tokio::test]
async fn a_resynced_machine_is_read_back_as_the_inventory_now_reports_it() {
    let engine = build_engine();
    let router = build_router(&engine);
    let hypervisor = import_hypervisor(&router, "hv-resync", "198.51.100.4").await;

    let before = Row {
        case: "as first discovered",
        name: "vm-resync".to_string(),
        host_name: "vm-resync-host".to_string(),
        path: "/Datacenter/vm/vm-resync".to_string(),
        last_backup_date: None,
        vmware_tags: Vec::new(),
    };
    let arns = record_inventory(&engine, &hypervisor, std::slice::from_ref(&before));
    let arn = arns[0].clone();
    let reply = get_machine(&router, &arn).await;
    assert_reports_row(&reply, before.case, &before, &arn, &hypervisor);

    // The same PATH is the same machine, so this updates the record rather than minting a
    // second one — every other member moves with the inventory.
    let after = Row {
        case: "after a re-sync at the same path",
        name: "vm-resync-renamed".to_string(),
        host_name: "vm-resync-host-2".to_string(),
        path: before.path.clone(),
        last_backup_date: backed_up_at(LAST_BACKUP_EPOCH_SECS),
        vmware_tags: vec![
            vmware_tag(Some("Environment"), Some("Production"), None),
            vmware_tag(Some("Tier"), Some("silver"), Some("Backup tier silver")),
        ],
    };
    let resynced = record_inventory(&engine, &hypervisor, std::slice::from_ref(&after));
    assert_eq!(
        resynced[0], arn,
        "a re-sync at the same path must keep the machine's ARN, got {:?}",
        resynced[0]
    );
    let reply = get_machine(&router, &arn).await;
    assert_reports_row(&reply, after.case, &after, &arn, &hypervisor);
}

/// A well-formed `ResourceArn` the store holds no virtual machine under is refused with
/// the modelled `ResourceNotFoundException` at HTTP 400, carrying `ErrorCode`
/// `VirtualMachineNotFound` and the fixed sentence this service states for a missing
/// machine —
/// whatever the ARN's resource type, partition, id or length, including at each published
/// bound of the modelled shape's length, and whether or not the ARN names some OTHER
/// resource this service really holds.
#[tokio::test]
async fn an_arn_with_no_recorded_machine_is_resource_not_found() {
    let engine = build_engine();
    let router = build_router(&engine);
    // A gateway, a hypervisor and a machine that really exist, so a refusal below is about
    // the ARN that was asked for and not about an empty store.
    let live_gateway = create(&router, "gw-live").await;
    let live_hypervisor = import_hypervisor(&router, "hv-live", "198.51.100.5").await;
    let rows = second_inventory();
    let live_machines = record_inventory(&engine, &live_hypervisor, &rows);

    // A hypervisor whose machines went with it when it was deleted: its machines' ARNs are
    // well formed and were once served, and the store no longer holds them.
    let doomed = import_hypervisor(&router, "hv-doomed", "198.51.100.6").await;
    let doomed_machines = record_inventory(&engine, &doomed, &rows);
    let deleted = call(
        &router,
        "DeleteHypervisor",
        &json!({ "HypervisorArn": doomed }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteHypervisor must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );

    let mut cases: Vec<(String, String)> = vec![
        (
            "a virtual machine no inventory ever reported".to_string(),
            arn_of("vm", "vm-1111111111111111"),
        ),
        (
            "another virtual machine id".to_string(),
            arn_of("vm", "vm-2222222222nonexistent"),
        ),
        (
            "a gateway that exists, which is not a virtual machine".to_string(),
            live_gateway.clone(),
        ),
        (
            "a hypervisor that exists, which is not a virtual machine".to_string(),
            live_hypervisor.clone(),
        ),
        (
            "a resource type this service does not name".to_string(),
            arn_of("appliance", "ap-1111111111111111"),
        ),
        (
            "the china partition".to_string(),
            arn_in_partition("aws-cn", "vm-3333333333333333"),
        ),
        (
            "the govcloud partition".to_string(),
            arn_in_partition("aws-us-gov", "vm-4444444444444444"),
        ),
        // The published bounds of the modelled `ResourceArn` length: both are accepted by
        // the boundary validator, so both reach the handler, which resolves them.
        (
            "at the 50-character minimum length".to_string(),
            arn_of_length(50),
        ),
        (
            "at the 500-character maximum length".to_string(),
            arn_of_length(500),
        ),
    ];
    for (row, arn) in rows.iter().zip(&doomed_machines) {
        cases.push((
            format!("a machine of a deleted hypervisor: {}", row.case),
            arn.clone(),
        ));
    }

    for (case, arn) in &cases {
        let refused = get_machine(&router, arn).await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode"),
            ),
            (
                400,
                Some("ResourceNotFoundException"),
                Some("VirtualMachineNotFound"),
            ),
            "case {case:?}: GetVirtualMachine on an ARN with no stored machine must answer \
             400 ResourceNotFoundException / VirtualMachineNotFound, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            message_of(&refused),
            "The virtual machine is not found.",
            "case {case:?}: the refusal must state the fixed sentence for a missing machine \
             and echo nothing of the request, got {:?}",
            refused.body
        );
        assert!(
            refused.body.pointer("/VirtualMachine").is_none(),
            "case {case:?}: a refused read must report no VirtualMachine, got {:?}",
            refused.body
        );
    }

    // The machines the store really does hold are untouched by every refusal above.
    for (row, arn) in rows.iter().zip(&live_machines) {
        let reply = get_machine(&router, arn).await;
        assert_reports_row(&reply, row.case, row, arn, &live_hypervisor);
    }
}

/// A `ResourceArn` outside the modelled constraints is refused as invalid — not answered as
/// a missing machine, and not answered 501: the shape of the request is wrong, which is a
/// different claim from "no such machine".
#[tokio::test]
async fn a_request_outside_the_modelled_constraint_is_refused_with_400() {
    let engine = build_engine();
    let router = build_router(&engine);
    let hypervisor = import_hypervisor(&router, "hv-refuse", "198.51.100.7").await;
    let rows = second_inventory();
    let arns = record_inventory(&engine, &hypervisor, &rows);

    let rows_to_refuse: Vec<(&str, Value)> = vec![
        ("no ResourceArn at all", json!({})),
        (
            "one character under the 50-character minimum",
            json!({ "ResourceArn": arn_of_length(49) }),
        ),
        (
            "one character over the 500-character maximum",
            json!({ "ResourceArn": arn_of_length(501) }),
        ),
        (
            "a partition the pattern does not accept",
            json!({ "ResourceArn": format!(
                "arn:aws-iso:backup-gateway:{REGION}:{}:vm/vm-0000000000000000",
                engine::EMULATOR_ACCOUNT_ID
            ) }),
        ),
        (
            "no resource id after the resource type",
            json!({ "ResourceArn": format!(
                "arn:aws:backup-gateway:{REGION}:{}:vm-0000000000000000000000",
                engine::EMULATOR_ACCOUNT_ID
            ) }),
        ),
        (
            "an underscore the id's character class rejects",
            json!({ "ResourceArn": arn_of("vm", "vm_0000000000000000") }),
        ),
    ];

    for (case, body) in &rows_to_refuse {
        let refused = call(&router, "GetVirtualMachine", body).await;
        assert_eq!(
            (refused.status, refused.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a request outside the modelled constraint must answer 400 \
             ValidationException, got {} {:?}",
            refused.status,
            refused.body
        );
    }

    // Nothing the refusals touched: every stored machine still reads back as it stood.
    for (row, arn) in rows.iter().zip(&arns) {
        let reply = get_machine(&router, arn).await;
        assert_reports_row(&reply, row.case, row, arn, &hypervisor);
    }
}

/// The read reports the store without changing it: reading a machine twice answers the
/// same body, leaves the store's machine count alone, and leaves the listing exactly as it
/// was — including for an ARN the read refused.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let engine = build_engine();
    let router = build_router(&engine);
    let hypervisor = import_hypervisor(&router, "hv-idempotent", "198.51.100.8").await;
    let rows = first_inventory();
    let arns = record_inventory(&engine, &hypervisor, &rows);

    let count_before = stored_machine_count(&engine);
    let listing_before = call(
        &router,
        "ListVirtualMachines",
        &json!({ "MaxResults": rows.len() }),
    )
    .await;

    for (row, arn) in rows.iter().zip(&arns) {
        let first = get_machine(&router, arn).await;
        let second = get_machine(&router, arn).await;
        assert_eq!(
            first.body, second.body,
            "case {:?}: reading a machine twice must answer the same body, got {:?} then {:?}",
            row.case, first.body, second.body
        );
    }
    // A refused read writes nothing either: it must not mint the machine it could not find.
    let missing = arn_of("vm", "vm-9999999999999999");
    for _ in 0..3 {
        let refused = get_machine(&router, &missing).await;
        assert_eq!(
            refused.status, 400,
            "a refused read must stay refused, got {} {:?}",
            refused.status, refused.body
        );
    }

    assert_eq!(
        stored_machine_count(&engine),
        count_before,
        "reads must not change how many machines the store holds, expected {count_before}",
    );
    let listing_after = call(
        &router,
        "ListVirtualMachines",
        &json!({ "MaxResults": rows.len() }),
    )
    .await;
    assert_eq!(
        listing_after.body, listing_before.body,
        "reads must leave the listing as it was, got {:?} then {:?}",
        listing_before.body, listing_after.body
    );
}

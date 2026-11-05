// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the inventory a METADATA SYNC materialises: the path from
//! `ImportHypervisorConfiguration` to a non-empty `ListVirtualMachines`, driven with wire
//! calls only.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle` and the workflow helpers of
//! `metadata_sync_over_the_wire`, so what is asserted is the wire behaviour and nothing
//! here reaches into `backup_gateway::state` to arrange a scene.
//!
//! The rule under test: a virtual machine is part of the INVENTORY of the hypervisor it
//! runs on, and the service event that materialises that inventory is a metadata sync of
//! the hypervisor — so a hypervisor whose metadata has never been synced lists no machine,
//! and once a sync of it has fallen due (`LatestMetadataSyncStatus` `SUCCEEDED`) the
//! listing reports the machines that sync discovered, each with the `ResourceArn`,
//! `HypervisorId`, `Name`, `HostName` and `Path` the modelled `VirtualMachine` declares and
//! each readable through `GetVirtualMachine`. Syncing the same hypervisor again reports the
//! same machines rather than minting a second copy of each, and a machine belongs to ONE
//! hypervisor, which is what the listing's optional `HypervisorArn` filters on.
//!
//! The tables below span that class rather than one request: hypervisors differing in name
//! form and length, in host form (IPv4, DNS, IPv6) and in the mappings and IAM role their
//! sync runs under; a hypervisor that is never synced beside them; every page size over the
//! machines two syncs discovered, filtered and unfiltered; and a re-sync, before and after
//! it falls due.
//!
//! Waiting here is a real wait ([`std::thread::sleep`]), as in the sibling
//! `metadata_sync_over_the_wire` suite: nothing but the clock carries a sync along, and the
//! machines it discovered are reported from the instant it falls due.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    VmwareToAwsTagMapping, MAX_VIRTUAL_MACHINE_NAME, METADATA_SYNC_SUCCEEDS_AFTER,
    SYNCED_VIRTUAL_MACHINES,
};

use crate::gateway_lifecycle::{build_engine, build_router, call, Reply, REGION};
use crate::hypervisor_property_mappings::{import, mapping};
use crate::metadata_sync_over_the_wire::{role_arn, served_hypervisor_with_mappings, start_sync};

/// Long enough past [`METADATA_SYNC_SUCCEEDS_AFTER`] that a sync started before the wait
/// has fallen due on a loaded machine too.
const PAST_THE_SYNC: Duration = METADATA_SYNC_SUCCEEDS_AFTER.saturating_add(Duration::from_millis(
    150,
));

/// One `VirtualMachine` as a listing or a read reports it
/// (`smithy_query(query="shape",shape="VirtualMachine")`: `HostName`, `HypervisorId`,
/// `Name`, `Path`, `ResourceArn`, `LastBackupDate`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Reported {
    resource_arn: String,
    hypervisor_id: String,
    name: String,
    host_name: String,
    path: String,
    /// Whether the reply carried `LastBackupDate` at all — absent for a machine that has
    /// never been backed up.
    has_last_backup_date: bool,
}

impl Reported {
    fn from_value(machine: &Value) -> Self {
        let string_at = |member: &str| {
            machine
                .get(member)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Self {
            resource_arn: string_at("ResourceArn"),
            hypervisor_id: string_at("HypervisorId"),
            name: string_at("Name"),
            host_name: string_at("HostName"),
            path: string_at("Path"),
            has_last_backup_date: machine.get("LastBackupDate").is_some(),
        }
    }
}

/// The `VirtualMachines` a `ListVirtualMachines` reply carries, in the order it carried
/// them.
fn listed(reply: &Reply) -> Vec<Reported> {
    reply
        .body
        .get("VirtualMachines")
        .and_then(Value::as_array)
        .map(|machines| machines.iter().map(Reported::from_value).collect())
        .unwrap_or_default()
}

/// `ListVirtualMachines`, with the members a case states and no others.
async fn list(
    router: &MultiServiceRouter,
    hypervisor_arn: Option<&str>,
    max_results: Option<u32>,
    next_token: Option<&str>,
) -> Reply {
    let mut body = json!({});
    if let Some(arn) = hypervisor_arn {
        body["HypervisorArn"] = json!(arn);
    }
    if let Some(max) = max_results {
        body["MaxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        body["NextToken"] = json!(token);
    }
    call(router, "ListVirtualMachines", &body).await
}

/// The machines `ListVirtualMachines` reports for `hypervisor_arn`, asserting the call was
/// served.
async fn inventory_of(router: &MultiServiceRouter, case: &str, hypervisor_arn: &str) -> Vec<Reported> {
    let reply = list(router, Some(hypervisor_arn), None, None).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: ListVirtualMachines must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    listed(&reply)
}

/// `GetVirtualMachine` for one machine's own ARN.
async fn get_machine(router: &MultiServiceRouter, resource_arn: &str) -> Reply {
    call(
        router,
        "GetVirtualMachine",
        &json!({ "ResourceArn": resource_arn }),
    )
    .await
}

/// The `hypervisor/<id>` resource id of `hypervisor_arn` — what every machine of that
/// hypervisor must report as its `HypervisorId`, read off the ARN the import handed back
/// rather than out of the crate.
fn hypervisor_id(hypervisor_arn: &str) -> String {
    hypervisor_arn
        .rsplit('/')
        .next()
        .expect("a hypervisor ARN carries a resource id")
        .to_string()
}

/// Assert everything the modelled `VirtualMachine` requires of one reported machine: its
/// ARN is a `vm/<id>` ARN of this account in the region the hypervisor was addressed in,
/// its `HypervisorId` is the hypervisor's own, its `Name` satisfies the member's published
/// bounds and pattern, its `Path` places it in the hypervisor's inventory, and it has never
/// been backed up.
fn assert_well_formed(case: &str, machine: &Reported, hypervisor_arn: &str) {
    let expected_prefix = format!(
        "arn:aws:backup-gateway:{REGION}:{}:vm/",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert!(
        machine.resource_arn.starts_with(&expected_prefix),
        "case {case:?}: a machine's ResourceArn must be a vm ARN of the hypervisor's own \
         region ({expected_prefix}), got {:?}",
        machine.resource_arn
    );
    assert_eq!(
        machine.hypervisor_id,
        hypervisor_id(hypervisor_arn),
        "case {case:?}: a machine must report the id of the hypervisor it runs on, got {:?}",
        machine.hypervisor_id
    );
    // `VirtualMachine$Name` is @length(min: 1, max: 100) with pattern `^[a-zA-Z0-9-]*$`,
    // and `HostName` targets the same shape
    // (`smithy_query(query="shape",shape="VirtualMachine")`).
    for (member, value) in [("Name", &machine.name), ("HostName", &machine.host_name)] {
        assert!(
            (1..=MAX_VIRTUAL_MACHINE_NAME).contains(&value.chars().count())
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "case {case:?}: a machine's {member} must satisfy the modelled bounds and \
             pattern, got {value:?}"
        );
    }
    assert!(
        machine.path.contains(&machine.name) && machine.path.starts_with('/'),
        "case {case:?}: a machine's Path must place it in its hypervisor's inventory under \
         its own name ({:?}), got {:?}",
        machine.name,
        machine.path
    );
    assert!(
        !machine.has_last_backup_date,
        "case {case:?}: a machine no backup has run for must report no LastBackupDate, got \
         {machine:?}"
    );
}

/// A hypervisor the client serves and then syncs, over the wire: what its sync must make
/// the listing report.
struct Row {
    case: &'static str,
    name: &'static str,
    host: &'static str,
    role: String,
    mappings: Vec<VmwareToAwsTagMapping>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an IPv4 host, one mapping, a plain role",
            name: "hv-inventory-0",
            host: "192.0.2.100",
            role: role_arn("aws", "backup-gateway-inventory"),
            mappings: vec![mapping("department", "finance", "Department", "Finance")],
        },
        Row {
            case: "a DNS host, three mappings, a service-role path",
            name: "hv-inventory-1",
            host: "hv-inventory-1.example.com",
            role: role_arn("aws", "service-role/backup-gateway-inventory"),
            mappings: vec![
                mapping("department", "legal", "Department", "Legal"),
                mapping("tier", "gold", "Tier", "Gold"),
                mapping("owner", "platform", "Owner", "Platform"),
            ],
        },
        Row {
            case: "an IPv6 host, no mappings at all, a role in another partition",
            name: "h",
            host: "2001:db8::70",
            role: role_arn("aws-cn", "cn-inventory"),
            mappings: vec![],
        },
    ]
}

/// A sync is what materialises a hypervisor's inventory: before one has fallen due the
/// listing reports nothing for the hypervisor, and afterwards it reports the machines the
/// sync discovered — each well formed, each readable on its own, and each belonging to that
/// hypervisor alone.
#[tokio::test]
async fn a_metadata_sync_materialises_the_inventory_a_listing_reports() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows = rows();
    let mut synced: Vec<(&Row, String)> = Vec::new();
    for row in &rows {
        let hypervisor =
            served_hypervisor_with_mappings(&router, row.name, row.host, &row.role, &row.mappings)
                .await;

        // Before any sync the hypervisor's inventory is empty, whatever its mappings say.
        let before = list(&router, Some(&hypervisor), None, None).await;
        assert_eq!(
            (before.status, listed(&before), before.next_token()),
            (200, Vec::new(), None),
            "case {:?}: a hypervisor whose metadata was never synced must list no machine, \
             got {} {:?}",
            row.case,
            before.status,
            before.body
        );

        let started = start_sync(&router, &hypervisor).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: the sync must answer 200, got {} {:?}",
            row.case, started.status, started.body
        );

        // The sync it just started has not read anything yet, so the inventory it will
        // report is not reported before it falls due.
        let in_flight = list(&router, Some(&hypervisor), None, None).await;
        assert_eq!(
            (in_flight.status, listed(&in_flight), in_flight.next_token()),
            (200, Vec::new(), None),
            "case {:?}: a sync still in flight must report no machine yet, got {} {:?}",
            row.case,
            in_flight.status,
            in_flight.body
        );

        synced.push((row, hypervisor));
    }

    // A hypervisor nobody ever syncs, so the listings below separate "synced" from
    // "exists".
    let never_synced = import(&router, "hv-inventory-unsynced", "192.0.2.199").await;

    // Nothing but the clock carries the syncs along.
    std::thread::sleep(PAST_THE_SYNC);

    let mut all_arns: BTreeSet<String> = BTreeSet::new();
    for (row, hypervisor) in &synced {
        let machines = inventory_of(&router, row.case, hypervisor).await;
        assert_eq!(
            machines.len(),
            SYNCED_VIRTUAL_MACHINES,
            "case {:?}: a succeeded sync must report the hypervisor's machines, got {:?}",
            row.case,
            machines
        );
        for machine in &machines {
            assert_well_formed(row.case, machine, hypervisor);
            assert!(
                all_arns.insert(machine.resource_arn.clone()),
                "case {:?}: a machine belongs to one hypervisor, so no ARN may be reported \
                 twice, got {:?}",
                row.case,
                machine.resource_arn
            );

            // Every member a listing reports comes back from a read of that one machine,
            // which adds the VMware tags the inventory attached to it — none here.
            let read = get_machine(&router, &machine.resource_arn).await;
            assert_eq!(
                (
                    read.status,
                    read.body
                        .get("VirtualMachine")
                        .map(Reported::from_value)
                        .as_ref(),
                    read.body.pointer("/VirtualMachine/VmwareTags").cloned()
                ),
                (200, Some(machine), Some(json!([]))),
                "case {:?}: GetVirtualMachine must report the machine the listing holds, \
                 got {} {:?}",
                row.case,
                read.status,
                read.body
            );
        }

        // The listing is ordered by ARN
        // (`smithy_query(query="shape",shape="ListVirtualMachinesOutput")`:
        // "ordered by their Amazon Resource Names (ARNs)").
        let mut sorted = machines.clone();
        sorted.sort_by(|left, right| left.resource_arn.cmp(&right.resource_arn));
        assert_eq!(
            machines.iter().map(|m| &m.resource_arn).collect::<Vec<_>>(),
            sorted.iter().map(|m| &m.resource_arn).collect::<Vec<_>>(),
            "case {:?}: the machines must be reported in ARN order, got {:?}",
            row.case,
            machines
        );
    }

    // The unfiltered listing is the whole-account one: every machine every sync discovered
    // and nothing else.
    let unfiltered = list(&router, None, None, None).await;
    assert_eq!(
        (
            unfiltered.status,
            listed(&unfiltered)
                .into_iter()
                .map(|machine| machine.resource_arn)
                .collect::<BTreeSet<_>>()
        ),
        (200, all_arns.clone()),
        "the unfiltered listing must report every synced machine and no other, got {} {:?}",
        unfiltered.status,
        unfiltered.body
    );

    // ... and a hypervisor that was never synced still has no inventory, however many
    // other hypervisors have one.
    let unsynced = list(&router, Some(&never_synced), None, None).await;
    assert_eq!(
        (unsynced.status, listed(&unsynced), unsynced.next_token()),
        (200, Vec::new(), None),
        "a hypervisor nobody synced must list no machine, got {} {:?}",
        unsynced.status,
        unsynced.body
    );
}

/// Syncing the same hypervisor again reports the same machines: a machine is identified by
/// where it sits in its hypervisor's inventory, so a re-sync updates the records it already
/// has rather than minting a second copy of each — and while the new sync is in flight the
/// inventory the previous one established still stands.
#[tokio::test]
async fn a_resync_reports_the_same_machines_and_mints_no_second_copy() {
    let engine = build_engine();
    let router = build_router(&engine);

    let hypervisor = served_hypervisor_with_mappings(
        &router,
        "hv-inventory-resynced",
        "192.0.2.110",
        &role_arn("aws-us-gov", "gov-inventory"),
        &[mapping("tier", "silver", "Tier", "Silver")],
    )
    .await;

    start_sync(&router, &hypervisor).await;
    std::thread::sleep(PAST_THE_SYNC);
    let first = inventory_of(&router, "the first sync", &hypervisor).await;
    assert_eq!(
        first.len(),
        SYNCED_VIRTUAL_MACHINES,
        "the first sync must report the hypervisor's machines, got {first:?}"
    );

    // A second sync is in flight: the inventory the first one established is what the
    // service still holds, so it is what the listing still reports.
    let restarted = start_sync(&router, &hypervisor).await;
    assert_eq!(
        restarted.status, 200,
        "starting the sync again must answer 200, got {} {:?}",
        restarted.status, restarted.body
    );
    let during = inventory_of(&router, "a re-sync in flight", &hypervisor).await;
    assert_eq!(
        during, first,
        "a re-sync in flight must not change what the listing reports, got {during:?}"
    );

    // ... and once it has fallen due, the machines are the same records, under the same
    // ARNs, in the same order.
    std::thread::sleep(PAST_THE_SYNC);
    let after = inventory_of(&router, "a re-sync that fell due", &hypervisor).await;
    assert_eq!(
        after, first,
        "a re-sync must report the machines the previous sync did, got {after:?}"
    );

    // Nothing else appeared account-wide either, so the re-sync minted no second copy.
    let unfiltered = list(&router, None, None, None).await;
    assert_eq!(
        listed(&unfiltered).len(),
        SYNCED_VIRTUAL_MACHINES,
        "a re-sync must mint no second copy of a machine, got {:?}",
        unfiltered.body
    );
}

/// The inventory a sync materialises pages and filters like every other listing of this
/// service: every page size from one machine to one past the whole set, filtered to one
/// hypervisor and unfiltered, reaches exactly the machines of that listing once each, in ARN
/// order, and the final page carries no token.
#[tokio::test]
async fn the_synced_inventory_pages_and_filters_like_any_listing() {
    let engine = build_engine();
    let router = build_router(&engine);

    let first = served_hypervisor_with_mappings(
        &router,
        "hv-inventory-paged-0",
        "192.0.2.120",
        &role_arn("aws", "paged-inventory-0"),
        &[mapping("tier", "gold", "Tier", "Gold")],
    )
    .await;
    let second = served_hypervisor_with_mappings(
        &router,
        "hv-inventory-paged-1",
        "192.0.2.121",
        &role_arn("aws", "paged-inventory-1"),
        &[mapping("tier", "bronze", "Tier", "Bronze")],
    )
    .await;
    start_sync(&router, &first).await;
    start_sync(&router, &second).await;
    std::thread::sleep(PAST_THE_SYNC);

    let whole: Vec<String> = listed(&list(&router, None, None, None).await)
        .into_iter()
        .map(|machine| machine.resource_arn)
        .collect();
    assert_eq!(
        whole.len(),
        2 * SYNCED_VIRTUAL_MACHINES,
        "two synced hypervisors must report both inventories, got {whole:?}"
    );

    // (case, the filter, the machines that filter selects)
    let filters: Vec<(String, Option<&str>, Vec<String>)> = vec![
        ("no filter at all".to_string(), None, whole.clone()),
        (
            "filtered to the first hypervisor".to_string(),
            Some(first.as_str()),
            inventory_of(&router, "the first hypervisor", &first)
                .await
                .into_iter()
                .map(|machine| machine.resource_arn)
                .collect(),
        ),
        (
            "filtered to the second hypervisor".to_string(),
            Some(second.as_str()),
            inventory_of(&router, "the second hypervisor", &second)
                .await
                .into_iter()
                .map(|machine| machine.resource_arn)
                .collect(),
        ),
    ];

    for (filter_case, filter, expected) in &filters {
        for page_size in 1..=(whole.len() + 1) {
            let case = format!("{filter_case}, MaxResults {page_size}");
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            // One more round than pages are needed, so a listing that never stops is a
            // failure rather than a hang.
            for _ in 0..=(whole.len() + 1) {
                let reply = list(
                    &router,
                    *filter,
                    Some(page_size as u32),
                    token.as_deref(),
                )
                .await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: a page must be served, got {} {:?}",
                    reply.status, reply.body
                );
                let page = listed(&reply);
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: a page must hold at most MaxResults machines, got {:?}",
                    reply.body
                );
                walked.extend(page.into_iter().map(|machine| machine.resource_arn));
                token = reply.next_token();
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                (token.as_ref(), &walked),
                (None, expected),
                "case {case:?}: paging must reach exactly the machines of the listing once \
                 each, in ARN order, and stop"
            );
        }
    }
}

/// Deleting a hypervisor takes its inventory with it: a machine is a machine ON a
/// hypervisor, so once the hypervisor is gone the service holds no inventory for it and the
/// machines it reported are neither listed nor readable.
#[tokio::test]
async fn deleting_a_hypervisor_takes_the_inventory_its_sync_materialised() {
    let engine = build_engine();
    let router = build_router(&engine);

    let kept = served_hypervisor_with_mappings(
        &router,
        "hv-inventory-kept",
        "192.0.2.130",
        &role_arn("aws", "kept-inventory"),
        &[mapping("tier", "gold", "Tier", "Gold")],
    )
    .await;
    let deleted = served_hypervisor_with_mappings(
        &router,
        "hv-inventory-deleted",
        "192.0.2.131",
        &role_arn("aws", "deleted-inventory"),
        &[mapping("tier", "gold", "Tier", "Gold")],
    )
    .await;
    start_sync(&router, &kept).await;
    start_sync(&router, &deleted).await;
    std::thread::sleep(PAST_THE_SYNC);

    let kept_machines = inventory_of(&router, "the hypervisor that stays", &kept).await;
    let doomed = inventory_of(&router, "the hypervisor that is deleted", &deleted).await;
    assert_eq!(
        (kept_machines.len(), doomed.len()),
        (SYNCED_VIRTUAL_MACHINES, SYNCED_VIRTUAL_MACHINES),
        "both syncs must have materialised an inventory, got {kept_machines:?} and {doomed:?}"
    );

    let removed = call(
        &router,
        "DeleteHypervisor",
        &json!({ "HypervisorArn": deleted }),
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "DeleteHypervisor must answer 200, got {} {:?}",
        removed.status, removed.body
    );

    let after = list(&router, None, None, None).await;
    assert_eq!(
        (after.status, listed(&after)),
        (200, kept_machines.clone()),
        "deleting a hypervisor must leave exactly the machines of the hypervisors that \
         remain, got {} {:?}",
        after.status,
        after.body
    );
    for machine in &doomed {
        let read = get_machine(&router, &machine.resource_arn).await;
        assert_eq!(
            (read.status, read.code.as_deref()),
            (400, Some("ResourceNotFoundException")),
            "a machine of a deleted hypervisor must not be readable, got {} {:?}",
            read.status,
            read.body
        );
    }
}

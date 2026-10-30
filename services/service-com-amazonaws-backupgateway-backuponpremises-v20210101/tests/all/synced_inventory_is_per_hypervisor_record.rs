// SPDX-License-Identifier: CC0-1.0

//! Tests for WHOSE machines a metadata sync publishes: the inventory one hypervisor record
//! reports is derived from the WHOLE identity of that record — the complete ARN this store
//! keyed it under — so no two records ever publish a machine with the same `Name`,
//! `HostName` or `Path`, and every sync of one record publishes the same machines again.
//!
//! The wire case drives real `POST /` awsJson1_0 requests through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle`, and signs each request for the region its
//! hypervisor belongs to — the region is an ARN segment, and the resource id behind an ARN
//! is minted from a per-region sequence, so the hypervisors of two regions can carry the
//! same resource id while being two different records.
//!
//! The table of the second case addresses the generator directly, because the segments it
//! has to span — partition, region, account, resource id, and a resource id longer than a
//! machine name may be — are not all reachable over the wire of one emulator: a client can
//! only address a region.
//!
//! A metadata sync's precondition is that the hypervisor's property mappings are
//! configured (`metadata_sync_requires_property_mappings`), and the appliance that puts
//! them is associated per region; so the wire case seeds the mappings straight into the
//! `property_mappings` field `PutHypervisorPropertyMappings` writes, exactly as the
//! `metadata_sync` suite does, and every call about the inventory itself goes over the
//! wire.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    synced_inventory, StoredPropertyMappings, VirtualMachineInventory, MAX_VIRTUAL_MACHINE_NAME,
    METADATA_SYNC_SUCCEEDS_AFTER, SYNCED_VIRTUAL_MACHINES,
};

use crate::gateway_lifecycle::{build_engine, build_router, call_in_region, Reply, REGION};
use crate::hypervisor_property_mappings::{mapping, set_mappings};

/// Long enough past [`METADATA_SYNC_SUCCEEDS_AFTER`] that a sync started before the wait
/// has fallen due on a loaded machine too — the span the sibling suites wait.
const PAST_THE_SYNC: Duration =
    METADATA_SYNC_SUCCEEDS_AFTER.saturating_add(Duration::from_millis(150));

/// One hypervisor the wire case imports: which region it is addressed in, and what the
/// import asks for.
struct Row {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    host: &'static str,
}

/// Hypervisors spanning the thing the rule generalises over — which RECORD a sync belongs
/// to: two regions holding their first hypervisor each (whose minted resource ids are the
/// per-region sequence's first, so they coincide), a third region holding two of them, and
/// a second hypervisor of the harness's own region.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the first hypervisor of the harness's region",
            region: REGION,
            name: "hv-per-record-0",
            host: "192.0.2.10",
        },
        Row {
            case: "a second hypervisor of the harness's region",
            region: REGION,
            name: "hv-per-record-1",
            host: "192.0.2.11",
        },
        Row {
            case: "the first hypervisor of a second region",
            region: "eu-central-1",
            name: "hv-per-record-2",
            host: "192.0.2.12",
        },
        Row {
            case: "the first hypervisor of a third region",
            region: "ap-southeast-2",
            name: "hv-per-record-3",
            host: "192.0.2.13",
        },
        Row {
            case: "a second hypervisor of that third region",
            region: "ap-southeast-2",
            name: "hv-per-record-4",
            host: "192.0.2.14",
        },
    ]
}

/// Import a hypervisor in `region` over the wire and hand back the ARN the import minted.
async fn import_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    host: &str,
) -> String {
    let reply = call_in_region(
        router,
        region,
        "ImportHypervisorConfiguration",
        &json!({ "Name": name, "Host": host }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
        .to_string()
}

/// `StartVirtualMachinesMetadataSync` for one hypervisor, signed for its own region.
async fn start_sync_in(router: &MultiServiceRouter, region: &str, arn: &str) -> Reply {
    call_in_region(
        router,
        region,
        "StartVirtualMachinesMetadataSync",
        &json!({ "HypervisorArn": arn }),
    )
    .await
}

/// The `(Name, HostName, Path)` of every machine `ListVirtualMachines` reports for
/// `hypervisor_arn` in its own region.
async fn published_machines(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    hypervisor_arn: &str,
) -> Vec<(String, String, String)> {
    let reply = call_in_region(
        router,
        region,
        "ListVirtualMachines",
        &json!({ "HypervisorArn": hypervisor_arn }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: ListVirtualMachines must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    let member = |machine: &Value, name: &str| {
        machine
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    reply
        .body
        .get("VirtualMachines")
        .and_then(Value::as_array)
        .map(|machines| {
            machines
                .iter()
                .map(|machine| {
                    (
                        member(machine, "Name"),
                        member(machine, "HostName"),
                        member(machine, "Path"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `VirtualMachine$Name` is `@length(min: 1, max: 100)` with pattern `^[a-zA-Z0-9-]*$`, and
/// `HostName` targets the same shape
/// (`smithy_query(query="shape",shape="VirtualMachine")`).
fn assert_modelled_name(case: &str, member: &str, value: &str) {
    assert!(
        (1..=MAX_VIRTUAL_MACHINE_NAME).contains(&value.chars().count())
            && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "case {case:?}: a machine's {member} must satisfy the modelled bounds and pattern, \
         got {value:?}"
    );
}

/// Two hypervisors are two records, so their syncs publish two inventories: whichever
/// regions they were imported in, no machine of one carries the `Name`, `HostName` or
/// `Path` of a machine of the other — and a re-sync of one of them publishes exactly what
/// its first sync did.
#[tokio::test]
async fn syncs_of_two_hypervisors_never_publish_the_same_machine() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows = rows();
    let mut imported: Vec<(&Row, String)> = Vec::new();
    for row in &rows {
        let arn = import_in(&router, row.region, row.name, row.host).await;
        // The sync's precondition: the record's property mappings, seeded here (see the
        // module docs) so what the cases below compare is the inventory each sync
        // publishes.
        set_mappings(
            &engine,
            &arn,
            Some(StoredPropertyMappings {
                iam_role_arn: format!(
                    "arn:aws:iam::{}:role/per-record",
                    engine::EMULATOR_ACCOUNT_ID
                ),
                vmware_to_aws_tag_mappings: vec![mapping("tier", "gold", "Tier", "Gold")],
            }),
        );
        let started = start_sync_in(&router, row.region, &arn).await;
        assert_eq!(
            (started.status, started.str_at("/HypervisorArn")),
            (200, Some(arn.as_str())),
            "case {:?}: the sync must answer 200 with the record's own ARN, got {} {:?}",
            row.case,
            started.status,
            started.body
        );
        imported.push((row, arn));
    }

    // Nothing but the clock carries a sync along.
    std::thread::sleep(PAST_THE_SYNC);

    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut host_names: BTreeSet<String> = BTreeSet::new();
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for (row, arn) in &imported {
        let machines = published_machines(&router, row.case, row.region, arn).await;
        assert_eq!(
            machines.len(),
            SYNCED_VIRTUAL_MACHINES,
            "case {:?}: a succeeded sync must publish the record's machines, got {machines:?}",
            row.case
        );
        for (name, host_name, path) in &machines {
            assert_modelled_name(row.case, "Name", name);
            assert_modelled_name(row.case, "HostName", host_name);
            assert!(
                names.insert(name.clone()),
                "case {:?}: no two records may publish one machine Name, got {name:?} a \
                 second time",
                row.case
            );
            assert!(
                host_names.insert(host_name.clone()),
                "case {:?}: no two records may publish one machine HostName, got \
                 {host_name:?} a second time",
                row.case
            );
            assert!(
                paths.insert(path.clone()),
                "case {:?}: no two records may publish one machine Path, got {path:?} a \
                 second time",
                row.case
            );
        }

        // Derived from the record's identity and from nothing else, so syncing it again
        // publishes the machines it already had.
        let restarted = start_sync_in(&router, row.region, arn).await;
        assert_eq!(
            restarted.status, 200,
            "case {:?}: starting the sync again must answer 200, got {} {:?}",
            row.case, restarted.status, restarted.body
        );
        std::thread::sleep(PAST_THE_SYNC);
        let again = published_machines(&router, row.case, row.region, arn).await;
        assert_eq!(
            again, machines,
            "case {:?}: a re-sync must publish the machines the previous sync did, got \
             {again:?}",
            row.case
        );
    }

    assert_eq!(
        names.len(),
        rows.len() * SYNCED_VIRTUAL_MACHINES,
        "every record's machines must be its own, got {names:?}"
    );
}

/// One ARN of this service, as the generator sees it.
struct ArnRow {
    case: &'static str,
    arn: String,
}

/// ARNs differing in ONE segment each, so the derivation is pinned on every part of a
/// record's identity: the ARN form is
/// `arn:(aws|aws-cn|aws-us-gov):backup-gateway:<region>:<account>:<type>/<id>`
/// (`smithy_query(query="shape",shape="VirtualMachine",member="ResourceArn")`, whose
/// `@length` caps an ARN at 500 characters).
fn arn_rows() -> Vec<ArnRow> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    // A resource id far longer than a machine name may be, and a second one agreeing with
    // it on every character a clipped name could keep: the fingerprint is what has to tell
    // them apart.
    let long_id = format!("hv-{}", "a".repeat(MAX_VIRTUAL_MACHINE_NAME * 2));
    let long_id_twin = format!("{long_id}-twin");
    vec![
        ArnRow {
            case: "the first hypervisor of one region",
            arn: format!("arn:aws:backup-gateway:{REGION}:{account}:hypervisor/hv-0000000000000000"),
        },
        ArnRow {
            case: "the same resource id in another region",
            arn: format!(
                "arn:aws:backup-gateway:eu-central-1:{account}:hypervisor/hv-0000000000000000"
            ),
        },
        ArnRow {
            case: "another resource id in the first region",
            arn: format!("arn:aws:backup-gateway:{REGION}:{account}:hypervisor/hv-0000000000000001"),
        },
        ArnRow {
            case: "the same resource id in another account",
            arn: format!(
                "arn:aws:backup-gateway:{REGION}:111122223333:hypervisor/hv-0000000000000000"
            ),
        },
        ArnRow {
            case: "the same resource id in another partition",
            arn: format!(
                "arn:aws-cn:backup-gateway:cn-north-1:{account}:hypervisor/hv-0000000000000000"
            ),
        },
        ArnRow {
            case: "the same resource id in the third partition",
            arn: format!(
                "arn:aws-us-gov:backup-gateway:us-gov-west-1:{account}:hypervisor/hv-0000000000000000"
            ),
        },
        ArnRow {
            case: "a resource id longer than a machine name may be",
            arn: format!("arn:aws:backup-gateway:{REGION}:{account}:hypervisor/{long_id}"),
        },
        ArnRow {
            case: "a longer resource id agreeing with it on every leading character",
            arn: format!("arn:aws:backup-gateway:{REGION}:{account}:hypervisor/{long_id_twin}"),
        },
        ArnRow {
            case: "a string that is no ARN of this service at all",
            arn: "hypervisor".to_string(),
        },
    ]
}

/// The inventory a record publishes is a function of its WHOLE ARN: two ARNs differing in
/// any one segment publish disjoint machines, every published name stays inside the
/// modelled bounds however long the ARN is, and asking twice for one ARN answers the same
/// machines.
#[tokio::test]
async fn the_published_inventory_is_a_function_of_the_whole_hypervisor_arn() {
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut paths: BTreeSet<String> = BTreeSet::new();

    for row in arn_rows() {
        let machines = synced_inventory(&row.arn);
        assert_eq!(
            machines.len(),
            SYNCED_VIRTUAL_MACHINES,
            "case {:?}: a sync must publish the record's machines, got {machines:?}",
            row.case
        );

        for machine in &machines {
            let VirtualMachineInventory {
                name,
                host_name,
                path,
                last_backup_date,
                vmware_tags,
            } = machine;
            assert_modelled_name(row.case, "Name", name);
            assert_modelled_name(row.case, "HostName", host_name);
            assert_eq!(
                (host_name, last_backup_date.is_some(), vmware_tags.len()),
                (name, false, 0),
                "case {:?}: a machine answers to its own name, has never been backed up and \
                 carries no VMware tag, got {machine:?}",
                row.case
            );
            assert!(
                path.starts_with('/') && path.ends_with(name.as_str()),
                "case {:?}: a machine's Path must place it in its hypervisor's inventory \
                 under its own name ({name:?}), got {path:?}",
                row.case
            );
            assert!(
                names.insert(name.clone()),
                "case {:?}: two ARNs differing in one segment must publish different \
                 machines, got {name:?} a second time",
                row.case
            );
            assert!(
                paths.insert(path.clone()),
                "case {:?}: two ARNs differing in one segment must publish different \
                 machine paths, got {path:?} a second time",
                row.case
            );
        }

        // Nothing but the ARN goes into the answer, so asking again answers the same.
        let again = synced_inventory(&row.arn);
        assert_eq!(
            again
                .iter()
                .map(|machine| (&machine.name, &machine.path))
                .collect::<Vec<_>>(),
            machines
                .iter()
                .map(|machine| (&machine.name, &machine.path))
                .collect::<Vec<_>>(),
            "case {:?}: the inventory of one ARN must not change between two reads",
            row.case
        );
    }
}

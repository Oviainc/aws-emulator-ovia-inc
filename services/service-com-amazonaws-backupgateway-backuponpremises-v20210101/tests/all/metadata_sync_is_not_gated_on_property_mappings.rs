// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a `StartVirtualMachinesMetadataSync` is NOT conditioned on:
//! anything the hypervisor's property mappings SAY.
//!
//! The sync has exactly one precondition — that the hypervisor's property mappings are
//! configured at all, the refusal the sibling `metadata_sync_requires_property_mappings`
//! suite pins — and once they are, nothing about them or about the rest of the record
//! conditions the sync: not how many mappings were put (an empty list included), not which
//! VMware categories or Amazon tag keys they map, not the partition, path or length of the
//! IAM role they were put with, and not the hypervisor's own name or host. Every such
//! record syncs, answers 200 with the stored record's ARN, carries
//! `LatestMetadataSyncStatus` to `SUCCEEDED` on the clock alone and materialises the
//! inventory `ListVirtualMachines` and `GetVirtualMachine` then report.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle` and the workflow helpers of
//! `metadata_sync_over_the_wire`, so what is asserted is the wire behaviour and nothing
//! here reaches into `backup_gateway::state` to arrange a scene.
//!
//! The table below spans that class rather than one request: hypervisors differing in name
//! form and length, in host form (IPv4, DNS, IPv6), in the mappings their put carried (one,
//! two, an empty list, each with its own category and key forms) and in the partition and
//! path of the role it named.

use std::time::Duration;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    VmwareToAwsTagMapping, METADATA_SYNC_SUCCEEDS_AFTER, SYNCED_VIRTUAL_MACHINES,
    SYNC_METADATA_SUCCEEDED,
};

use crate::gateway_lifecycle::{build_engine, build_router, call, Reply};
use crate::hypervisor_property_mappings::mapping;
use crate::metadata_sync_over_the_wire::{role_arn, served_hypervisor_with_mappings, start_sync};
use crate::metadata_sync_requires_property_mappings::listed_arns;

/// Long enough past [`METADATA_SYNC_SUCCEEDS_AFTER`] that a sync started before the wait
/// has fallen due on a loaded machine too.
const PAST_THE_SYNC: Duration =
    METADATA_SYNC_SUCCEEDS_AFTER.saturating_add(Duration::from_millis(150));

/// One hypervisor whose gateway put its property mappings, so the sync's precondition is
/// met — and whose row differs from every other in what those mappings say.
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
            case: "one mapping, a plain role, an IPv4 host",
            name: "hv-ungated-0",
            host: "192.0.2.140",
            role: role_arn("aws", "backup-gateway-ungated"),
            mappings: vec![mapping("department", "finance", "Department", "Finance")],
        },
        Row {
            case: "two mappings, a service-role path, a DNS host",
            name: "hv-ungated-1",
            host: "hv-ungated-1.example.com",
            role: role_arn("aws", "service-role/backup-gateway-ungated"),
            mappings: vec![
                mapping("department", "finance", "Department", "Finance"),
                mapping("tier", "gold", "Tier", "Gold"),
            ],
        },
        Row {
            case: "an empty mapping list, a role in another partition, an IPv6 host",
            name: "hv-ungated-2",
            host: "2001:db8::90",
            role: role_arn("aws-cn", "cn-ungated"),
            mappings: vec![],
        },
        Row {
            case: "the minimum of every mapping member, a one-character name and the \
                   shortest role the model allows",
            name: "h",
            host: "198.51.100.140",
            // `IamRoleArn` is `@length(min: 20, max: 2048)` with pattern
            // `^arn:(aws|aws-cn|aws-us-gov):iam::([0-9]+):role/(\S+)$`.
            role: "arn:aws:iam::1:role/r".to_string(),
            mappings: vec![mapping("c", "t", "k", "")],
        },
    ]
}

async fn get_hypervisor(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(router, "GetHypervisor", &json!({ "HypervisorArn": arn })).await
}

/// The `hypervisor/<id>` resource id of `hypervisor_arn`, read off the ARN the import
/// handed back — what every machine of that hypervisor reports as its `HypervisorId`.
fn hypervisor_id(hypervisor_arn: &str) -> &str {
    hypervisor_arn
        .rsplit('/')
        .next()
        .expect("a hypervisor ARN carries a resource id")
}

/// A hypervisor whose mappings are configured syncs whatever those mappings say: the start
/// is served, the clock alone carries it to `SUCCEEDED`, and the inventory it discovered is
/// what the listing reports and a read of each machine resolves.
#[tokio::test]
async fn a_configured_hypervisor_syncs_whatever_its_mappings_say() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows = rows();
    let mut synced: Vec<(&Row, String)> = Vec::new();
    for row in &rows {
        let hypervisor =
            served_hypervisor_with_mappings(&router, row.name, row.host, &row.role, &row.mappings)
                .await;

        // Before the sync the hypervisor has no inventory, so the machines below are the
        // ones this sync materialised.
        assert_eq!(
            listed_arns(&router, Some(&hypervisor)).await,
            (200, Vec::new()),
            "case {:?}: a hypervisor whose metadata was never synced must list no machine",
            row.case
        );

        let started = start_sync(&router, &hypervisor).await;
        assert_eq!(
            (started.status, started.str_at("/HypervisorArn")),
            (200, Some(hypervisor.as_str())),
            "case {:?}: the sync must answer 200 with the record's own ARN, got {} {:?}",
            row.case,
            started.status,
            started.body
        );
        synced.push((row, hypervisor));
    }

    // Nothing but the clock carries a sync along.
    std::thread::sleep(PAST_THE_SYNC);

    for (row, hypervisor) in &synced {
        let read = get_hypervisor(&router, hypervisor).await;
        assert_eq!(
            (
                read.status,
                read.str_at("/Hypervisor/LatestMetadataSyncStatus"),
                read.body
                    .pointer("/Hypervisor/LastSuccessfulMetadataSyncTime")
                    .is_some()
            ),
            (200, Some(SYNC_METADATA_SUCCEEDED), true),
            "case {:?}: the sync must have succeeded, with the time it succeeded at, got \
             {} {:?}",
            row.case,
            read.status,
            read.body
        );

        let (status, machines) = listed_arns(&router, Some(hypervisor)).await;
        assert_eq!(
            (status, machines.len()),
            (200, SYNCED_VIRTUAL_MACHINES),
            "case {:?}: a succeeded sync must make the hypervisor's machines listable, got \
             {machines:?}",
            row.case
        );

        // ... and every machine the listing holds reads back as a machine of that
        // hypervisor.
        for resource_arn in &machines {
            let machine = call(
                &router,
                "GetVirtualMachine",
                &json!({ "ResourceArn": resource_arn }),
            )
            .await;
            assert_eq!(
                (
                    machine.status,
                    machine.str_at("/VirtualMachine/ResourceArn"),
                    machine.str_at("/VirtualMachine/HypervisorId")
                ),
                (
                    200,
                    Some(resource_arn.as_str()),
                    Some(hypervisor_id(hypervisor))
                ),
                "case {:?}: GetVirtualMachine must report the listed machine as one of this \
                 hypervisor's, got {} {:?}",
                row.case,
                machine.status,
                machine.body
            );
        }
    }
}

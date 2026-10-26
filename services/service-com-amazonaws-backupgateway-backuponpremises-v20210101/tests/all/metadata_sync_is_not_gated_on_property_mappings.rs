// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE thing a `StartVirtualMachinesMetadataSync` is conditioned
//! on: the hypervisor its `HypervisorArn` resolves to.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle` and the workflow helpers of
//! `metadata_sync_over_the_wire`, so what is asserted is the wire behaviour and nothing
//! here reaches into `backup_gateway::state` to arrange a scene.
//!
//! The rule under test: a metadata sync of a hypervisor this service holds is accepted
//! whatever else is true of that record — a hypervisor's PROPERTY MAPPINGS are the
//! optional VMware-to-Amazon tag mappings `PutHypervisorPropertyMappings` sets, not a
//! precondition of a sync — so `ImportHypervisorConfiguration` followed by
//! `StartVirtualMachinesMetadataSync` answers 200 with the stored record's ARN, carries
//! `LatestMetadataSyncStatus` to `SUCCEEDED` on the clock alone and materialises the
//! inventory `ListVirtualMachines` and `GetVirtualMachine` then report. Only an ARN this
//! service holds no hypervisor under is refused, with the modelled `AccessDeniedException`
//! (HTTP 403, `ErrorCode` `MappingsNotConfigured`) — the recorded answer, which reveals
//! nothing about whether the resource exists.
//!
//! The table below spans that class rather than one request: hypervisors differing in name
//! form and length, in host form (IPv4, DNS, IPv6) and in how far along the optional
//! mappings workflow they were taken (imported only, served by a gateway, mappings put,
//! mappings put as an empty list with a role in another partition); and, for the refusal,
//! ARNs differing in resource type, partition, id and length, including each published
//! bound of `ServerArn`.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    VmwareToAwsTagMapping, MAPPINGS_NOT_CONFIGURED, METADATA_SYNC_SUCCEEDS_AFTER,
    SYNCED_VIRTUAL_MACHINES, SYNC_METADATA_SUCCEEDED,
};

use crate::gateway_lifecycle::{build_engine, build_router, call, create, Reply};
use crate::hypervisor_property_mappings::{arn_of_length, hypervisor_arn_in, import, mapping};
use crate::metadata_sync_over_the_wire::{
    associate_gateway, role_arn, served_hypervisor_with_mappings, start_sync,
};

/// Long enough past [`METADATA_SYNC_SUCCEEDS_AFTER`] that a sync started before the wait
/// has fallen due on a loaded machine too.
const PAST_THE_SYNC: Duration =
    METADATA_SYNC_SUCCEEDS_AFTER.saturating_add(Duration::from_millis(150));

/// How far along the OPTIONAL property-mappings workflow a row's hypervisor is taken
/// before its metadata is synced. The sync keys on none of it.
enum Mappings {
    /// Imported and nothing else: no gateway serves it and no mappings were put.
    NotTouched,
    /// A gateway serves the hypervisor, but no mappings were put for it.
    GatewayOnly,
    /// A gateway serves it and these mappings were put with that IAM role.
    Put {
        role: String,
        mappings: Vec<VmwareToAwsTagMapping>,
    },
}

struct Row {
    case: &'static str,
    name: &'static str,
    host: &'static str,
    mappings: Mappings,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "imported and nothing else, an IPv4 host",
            name: "hv-ungated-0",
            host: "192.0.2.140",
            mappings: Mappings::NotTouched,
        },
        Row {
            case: "a gateway serves it, no mappings put, a DNS host",
            name: "hv-ungated-1",
            host: "hv-ungated-1.example.com",
            mappings: Mappings::GatewayOnly,
        },
        Row {
            case: "two mappings put with a service-role path, an IPv6 host",
            name: "hv-ungated-2",
            host: "2001:db8::90",
            mappings: Mappings::Put {
                role: role_arn("aws", "service-role/backup-gateway-ungated"),
                mappings: vec![
                    mapping("department", "finance", "Department", "Finance"),
                    mapping("tier", "gold", "Tier", "Gold"),
                ],
            },
        },
        Row {
            case: "an empty mapping list put with a role in another partition, a \
                   one-character name",
            name: "h",
            host: "198.51.100.140",
            mappings: Mappings::Put {
                role: role_arn("aws-cn", "cn-ungated"),
                mappings: vec![],
            },
        },
    ]
}

/// Build the row's hypervisor with wire calls only, and hand back the ARN the import
/// minted.
async fn hypervisor_for(router: &MultiServiceRouter, row: &Row) -> String {
    match &row.mappings {
        Mappings::NotTouched => import(router, row.name, row.host).await,
        Mappings::GatewayOnly => {
            let hypervisor = import(router, row.name, row.host).await;
            associate_gateway(router, &format!("gw-{}", row.name), &hypervisor).await;
            hypervisor
        }
        Mappings::Put { role, mappings } => {
            served_hypervisor_with_mappings(router, row.name, row.host, role, mappings).await
        }
    }
}

async fn get_hypervisor(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(router, "GetHypervisor", &json!({ "HypervisorArn": arn })).await
}

/// The `ResourceArn`s `ListVirtualMachines` reports, for one hypervisor when `filter` is
/// set and for the whole account when it is not.
async fn listed_arns(router: &MultiServiceRouter, filter: Option<&str>) -> (u16, Vec<String>) {
    let mut body = json!({});
    if let Some(arn) = filter {
        body["HypervisorArn"] = json!(arn);
    }
    let reply = call(router, "ListVirtualMachines", &body).await;
    let arns = reply
        .body
        .get("VirtualMachines")
        .and_then(Value::as_array)
        .map(|machines| {
            machines
                .iter()
                .map(|machine| {
                    machine
                        .get("ResourceArn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    (reply.status, arns)
}

/// The `hypervisor/<id>` resource id of `hypervisor_arn`, read off the ARN the import
/// handed back — what every machine of that hypervisor reports as its `HypervisorId`.
fn hypervisor_id(hypervisor_arn: &str) -> &str {
    hypervisor_arn
        .rsplit('/')
        .next()
        .expect("a hypervisor ARN carries a resource id")
}

/// A hypervisor this service holds syncs whatever property mappings it carries: the start
/// is served, the clock alone carries it to `SUCCEEDED`, and the inventory it discovered is
/// what the listing reports and a read of each machine resolves.
#[tokio::test]
async fn every_stored_hypervisor_syncs_whatever_mappings_it_carries() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows = rows();
    let mut synced: Vec<(&Row, String)> = Vec::new();
    for row in &rows {
        let hypervisor = hypervisor_for(&router, row).await;

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

/// Only an ARN this service holds no hypervisor under is refused, and the refusal writes
/// nothing: no sync, and so no inventory, anywhere.
#[tokio::test]
async fn an_arn_no_hypervisor_is_stored_under_is_the_one_refusal() {
    let engine = build_engine();
    let router = build_router(&engine);

    // A gateway that really exists, so its refusal is about the hypervisor the sync
    // resolves and not about an empty store.
    let live_gateway = create(&router, "gw-ungated-live").await;

    let rows: Vec<(&str, String)> = vec![
        (
            "a hypervisor ARN that was never imported",
            hypervisor_arn_in("aws", "hv-ffffffffffffff20"),
        ),
        (
            "a never-imported ARN in the aws-cn partition",
            hypervisor_arn_in("aws-cn", "hv-ffffffffffffff21"),
        ),
        (
            "a never-imported ARN in the aws-us-gov partition",
            hypervisor_arn_in("aws-us-gov", "hv-ffffffffffffff22"),
        ),
        ("a gateway ARN that really names a gateway", live_gateway),
        (
            "the 50-character minimum of the modelled ServerArn",
            arn_of_length(50),
        ),
        (
            "the 500-character maximum of the modelled ServerArn",
            arn_of_length(500),
        ),
    ];

    for (case, arn) in &rows {
        let reply = start_sync(&router, arn).await;
        assert_eq!(
            (
                reply.status,
                reply.code.as_deref(),
                reply.str_at("/ErrorCode")
            ),
            (
                403,
                Some("AccessDeniedException"),
                Some(MAPPINGS_NOT_CONFIGURED)
            ),
            "case {case:?}: a sync for an ARN no hypervisor is stored under must be refused \
             403 AccessDeniedException, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // Waiting changes nothing: a refused sync was never started, so it never falls due.
    std::thread::sleep(PAST_THE_SYNC);
    assert_eq!(
        listed_arns(&router, None).await,
        (200, Vec::new()),
        "a refused sync must materialise no inventory"
    );
}

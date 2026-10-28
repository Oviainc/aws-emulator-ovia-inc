// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the REGION scope of this service's listings: `ListGateways`,
//! `ListHypervisors` and `ListVirtualMachines`.
//!
//! Every case drives real `POST /` awsJson1_0 requests through the `MultiServiceRouter` —
//! the harness of `gateway_lifecycle` — so what is asserted is the wire behaviour, and the
//! region a request carries is the one its SigV4 credential scope names, exactly as a
//! client addressing a regional endpoint would.
//!
//! The rule under test: every resource of this service is regional (its region is a
//! segment of the ARN minted for it), so a listing reports only the resources of the
//! Region the listing request was addressed to — `ListGateways` is documented as listing
//! "backup gateways owned by an Amazon Web Services account in an Amazon Web Services
//! Region" (`smithy_query(query="doc",shape="ListGateways")`) — and the records of every
//! other region are no more part of it than another account's would be. One in-memory
//! store serves every region a client addresses, so the filter is what keeps the listings
//! of two regions apart.
//!
//! The table below spans the class the rule generalises over: several regions (the
//! harness's own, three others in three different continents' prefixes, and one no record
//! was ever created in), differing in how many gateways, hypervisors and virtual machines
//! each holds — including a region with gateways but no hypervisor and one with no record
//! at all — asserted on all three listings, unpaged and at a page size that truncates.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{service_state, VirtualMachineInventory};

use crate::gateway_lifecycle::{
    build_engine, build_router, call_in_region, create_body, issued_activation_key_for, Reply,
    REGION,
};

/// One region the table seeds, and how many records of each kind it holds.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    gateways: usize,
    /// How many hypervisors to import; each gets `machines_per_hypervisor` machines.
    hypervisors: usize,
    machines_per_hypervisor: usize,
}

/// The regions under test. Region names are only ARN segments to this service, so what
/// matters is that they differ; these are spelled as AWS spells them.
const ROWS: [Row; 5] = [
    Row {
        case: "the region the sibling suites sign for",
        region: REGION,
        gateways: 2,
        hypervisors: 1,
        machines_per_hypervisor: 2,
    },
    Row {
        case: "a second region, holding more hypervisors than the first",
        region: "eu-central-1",
        gateways: 1,
        hypervisors: 2,
        machines_per_hypervisor: 1,
    },
    Row {
        case: "a third region, holding more gateways than any other",
        region: "ap-southeast-2",
        gateways: 3,
        hypervisors: 1,
        machines_per_hypervisor: 3,
    },
    Row {
        case: "a region holding gateways but no hypervisor and no machine",
        region: "sa-east-1",
        gateways: 1,
        hypervisors: 0,
        machines_per_hypervisor: 0,
    },
    Row {
        case: "a region no record was ever created in",
        region: "me-south-1",
        gateways: 0,
        hypervisors: 0,
        machines_per_hypervisor: 0,
    },
];

/// The ARNs one region's records are stored under, by resource kind.
#[derive(Debug, Default, Clone)]
struct Seeded {
    gateways: BTreeSet<String>,
    hypervisors: BTreeSet<String>,
    machines: BTreeSet<String>,
}

/// Create `row.gateways` gateways, import `row.hypervisors` hypervisors and report
/// `row.machines_per_hypervisor` machines on each, all in `row.region`, and hand back the
/// ARNs the store now holds them under.
async fn seed(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    row: &Row,
) -> Seeded {
    let mut seeded = Seeded::default();
    for index in 0..row.gateways {
        let activation_key = issued_activation_key_for(router, row.region).await;
        let reply = call_in_region(
            router,
            row.region,
            "CreateGateway",
            &create_body(
                &format!("gw-{}-{index}", row.region),
                &activation_key,
                &BTreeMap::new(),
            ),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: CreateGateway must answer 200, got {} {:?}",
            row.case, reply.status, reply.body
        );
        seeded.gateways.insert(reply.gateway_arn().to_string());
    }

    for index in 0..row.hypervisors {
        let reply = call_in_region(
            router,
            row.region,
            "ImportHypervisorConfiguration",
            &json!({
                "Name": format!("hv-{}-{index}", row.region),
                // A host of its own per record: a record stands for the appliance there.
                "Host": format!("198.51.100.{}", seeded.hypervisors.len() + 1),
            }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
            row.case, reply.status, reply.body
        );
        let hypervisor_arn = reply
            .str_at("/HypervisorArn")
            .unwrap_or_else(|| panic!("case {:?}: no HypervisorArn in {:?}", row.case, reply.body))
            .to_string();

        // A virtual machine is not minted by any operation of this model: it reaches the
        // service as part of the inventory of the hypervisor it runs on, which is the
        // channel `record_virtual_machine` stands for — in the hypervisor's own region.
        let state = service_state(&**engine).expect("the in-memory engine must expose this state");
        let mut store = state.lock();
        for machine in 0..row.machines_per_hypervisor {
            let recorded = store
                .record_virtual_machine(
                    row.region,
                    &hypervisor_arn,
                    VirtualMachineInventory {
                        name: format!("vm-{index}-{machine}"),
                        host_name: format!("vm-{index}-{machine}"),
                        path: format!("/Datacenter/vm/{index}/{machine}"),
                        last_backup_date: None,
                        vmware_tags: Vec::new(),
                    },
                    SystemTime::now(),
                )
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the store must record a machine on an imported hypervisor",
                        row.case
                    )
                });
            seeded.machines.insert(recorded.arn);
        }
        seeded.hypervisors.insert(hypervisor_arn);
    }

    seeded
}

/// The ARNs a listing reply carries, read off the member each listing names them in.
fn listed_arns(reply: &Reply, list_member: &str, arn_member: &str) -> Vec<String> {
    reply
        .body
        .get(list_member)
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    record
                        .get(arn_member)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Walk every page of `operation` in `region` at `max_results`, and hand back the ARNs in
/// the order they were handed out.
async fn walk_pages(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    list_member: &str,
    arn_member: &str,
    max_results: Option<i64>,
) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({});
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let reply = call_in_region(router, region, operation, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {region}/{operation} MaxResults={max_results:?} page {pages}: must answer \
             200, got {} {:?}",
            reply.status, reply.body
        );
        seen.extend(listed_arns(&reply, list_member, arn_member));
        token = reply.next_token();
        pages += 1;
        if token.is_none() {
            break;
        }
        assert!(
            pages <= 64,
            "case {region}/{operation} MaxResults={max_results:?}: a listing must terminate, \
             got {pages} pages and {} ARNs",
            seen.len()
        );
    }
    seen
}

/// Every listing of this service reports only the records of the region the request was
/// addressed to — whole, in ARN order, and whichever resource it lists.
#[tokio::test]
async fn a_listing_reports_only_the_records_of_the_region_it_was_addressed_to() {
    let engine = build_engine();
    let router = build_router(&engine);

    // One store, seeded from several regions' endpoints, exactly as a client addressing
    // more than one endpoint leaves it.
    let mut seeded: Vec<(Row, Seeded)> = Vec::new();
    for row in ROWS {
        let records = seed(&engine, &router, &row).await;
        seeded.push((row, records));
    }

    // (listing, the output member holding the records, the ARN member of a record,
    //  the expected ARNs of a region)
    type Expected = fn(&Seeded) -> &BTreeSet<String>;
    let listings: [(&str, &str, &str, Expected); 3] = [
        ("ListGateways", "Gateways", "GatewayArn", |s| &s.gateways),
        (
            "ListHypervisors",
            "Hypervisors",
            "HypervisorArn",
            |s| &s.hypervisors,
        ),
        (
            "ListVirtualMachines",
            "VirtualMachines",
            "ResourceArn",
            |s| &s.machines,
        ),
    ];

    for (row, records) in &seeded {
        for (operation, list_member, arn_member, expected) in listings {
            let expected = expected(records);

            // Unpaged: the whole region, and nothing of any other.
            let reply = call_in_region(&router, row.region, operation, &json!({})).await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: {operation} must answer 200, got {} {:?}",
                row.case, reply.status, reply.body
            );
            let listed: Vec<String> = listed_arns(&reply, list_member, arn_member);
            assert_eq!(
                listed.iter().cloned().collect::<BTreeSet<String>>(),
                *expected,
                "case {:?}: {operation} in {} must report exactly that region's records, \
                 got {listed:?}",
                row.case,
                row.region
            );
            let mut sorted = listed.clone();
            sorted.sort();
            assert_eq!(
                listed, sorted,
                "case {:?}: {operation} must report its records in ARN order, got {listed:?}",
                row.case
            );
            for arn in &listed {
                assert!(
                    arn.contains(&format!(":{}:", row.region)),
                    "case {:?}: {operation} must report only ARNs of {}, got {arn:?}",
                    row.case,
                    row.region
                );
            }
            assert_eq!(
                reply.next_token(),
                None,
                "case {:?}: {operation} must not promise a successor to a complete page, \
                 got {:?}",
                row.case,
                reply.body
            );

            // And paged at one record per page: the region's records, still whole and
            // still only its own.
            let walked = walk_pages(
                &router,
                row.region,
                operation,
                list_member,
                arn_member,
                Some(1),
            )
            .await;
            assert_eq!(
                walked, listed,
                "case {:?}: {operation} paged at MaxResults=1 must hand out the same \
                 records as the unpaged listing, got {walked:?} against {listed:?}",
                row.case
            );
        }
    }

    // The filtered listings really are filtered: the store holds every region's records,
    // so no single region's listing is the whole store.
    let stored_gateways: usize = seeded.iter().map(|(_, records)| records.gateways.len()).sum();
    let widest = seeded
        .iter()
        .map(|(_, records)| records.gateways.len())
        .max()
        .unwrap_or_default();
    assert!(
        stored_gateways > widest,
        "the store must hold gateways of more than one region for this test to mean \
         anything, got {stored_gateways} across the widest region's {widest}"
    );
}

/// `ListVirtualMachines`'s `HypervisorArn` filter and the region scope compose: a machine
/// is reported only when it is in the addressed region AND on the named hypervisor, so a
/// hypervisor of another region selects nothing rather than that region's machines.
#[tokio::test]
async fn the_hypervisor_filter_does_not_reach_across_regions() {
    let engine = build_engine();
    let router = build_router(&engine);

    let mut per_region: Vec<(Row, Seeded)> = Vec::new();
    for row in ROWS.iter().filter(|row| row.hypervisors > 0) {
        let records = seed(&engine, &router, row).await;
        per_region.push((*row, records));
    }

    for (row, records) in &per_region {
        for hypervisor in &records.hypervisors {
            for (other, _) in &per_region {
                let reply = call_in_region(
                    &router,
                    other.region,
                    "ListVirtualMachines",
                    &json!({ "HypervisorArn": hypervisor }),
                )
                .await;
                assert_eq!(
                    reply.status, 200,
                    "case {:?} listed from {}: ListVirtualMachines must answer 200, got {} {:?}",
                    row.case, other.region, reply.status, reply.body
                );
                let listed: BTreeSet<String> =
                    listed_arns(&reply, "VirtualMachines", "ResourceArn")
                        .into_iter()
                        .collect();
                let expects_machines = other.region == row.region;
                assert_eq!(
                    !listed.is_empty(),
                    expects_machines,
                    "case {:?}: a hypervisor of {} listed from {} must report {}, got \
                     {listed:?}",
                    row.case,
                    row.region,
                    other.region,
                    if expects_machines {
                        "its machines"
                    } else {
                        "nothing"
                    }
                );
                assert!(
                    listed.iter().all(|arn| records.machines.contains(arn)),
                    "case {:?}: every reported machine must be one of that hypervisor's \
                     region, got {listed:?}",
                    row.case
                );
            }
        }
    }
}

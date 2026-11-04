// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the page size every listing of this service serves.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`
//! — the harness of `gateway_lifecycle` — so what is asserted is the wire behaviour of
//! the three list operations.
//!
//! The rule under test: a listing of this service serves
//! [`DEFAULT_PAGE_SIZE`](backup_gateway::state::DEFAULT_PAGE_SIZE) records when the
//! request names no `MaxResults`, and otherwise the size it asked for floored at one
//! record and capped at [`MAX_PAGE_SIZE`](backup_gateway::state::MAX_PAGE_SIZE) — one
//! rule for `ListGateways`, `ListHypervisors` and `ListVirtualMachines` alike, because
//! nothing about a page size is specific to the resource being listed. The table below
//! spans the class the rule generalises over: no `MaxResults` at all, the modelled
//! minimum of 1, sizes inside the server's ceiling, the ceiling itself, one past it, a
//! multiple of it and the largest value the modelled integer can carry
//! (`MaxResults` is `@range(min: 1)` with no maximum —
//! `smithy_query(query="operation",operation="ListGateways")`), each driven through all
//! three operations over one store that holds more records than any page may carry.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    page_limit, service_state, Creation, HypervisorConfiguration, VirtualMachineInventory,
    BACKUP_VM, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE,
};

use crate::gateway_lifecycle::{build_engine, build_router, call, Reply, REGION};

/// How many records of each resource type the store holds: more than the largest page any
/// request can ask for, so a clamped page is shorter than the store and every case can
/// tell the limit it was served from the number of records that exist.
const SEEDED_RECORDS: usize = MAX_PAGE_SIZE + 3;

/// The three listings under test, each with the output member that carries its records.
const LISTINGS: [(&str, &str); 3] = [
    ("ListGateways", "Gateways"),
    ("ListHypervisors", "Hypervisors"),
    ("ListVirtualMachines", "VirtualMachines"),
];

/// The page size the rule says a request naming `max_results` is served out of a store of
/// `records` records — restated here, independently of the crate, so the test pins the
/// contract rather than whatever the code happens to compute.
fn expected_page_size(max_results: Option<i32>, records: usize) -> usize {
    let asked = match max_results {
        None => DEFAULT_PAGE_SIZE,
        Some(max) if max < 1 => 1,
        Some(max) => (max as usize).min(MAX_PAGE_SIZE),
    };
    asked.min(records)
}

/// How many records a listing reply carried under `member`.
fn listed_len(reply: &Reply, member: &str) -> usize {
    reply
        .body
        .get(member)
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

/// Fill the store with [`SEEDED_RECORDS`] gateways, hypervisors and virtual machines.
///
/// Through the state API rather than one request per record: what is under test is the
/// page the handlers cut, not the operations that write the records — and a virtual
/// machine reaches the service as part of a hypervisor's inventory, which no operation of
/// this model creates. Everything is dated in the past, so every record is already part
/// of its listing whatever the clock does during the test.
fn seed(engine: &Arc<engine::InMemoryEngine>) {
    let past = SystemTime::now() - Duration::from_secs(3_600);
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();

    // One hypervisor to hang the inventory off, then the records of each type.
    let host_hypervisor = store.import_hypervisor(
        REGION,
        HypervisorConfiguration {
            name: "hv-inventory-host".to_string(),
            host: "hv-inventory-host.example.internal".to_string(),
            kms_key_arn: None,
            tags: BTreeMap::new(),
        },
        Creation {
            by: "SEEDED-CREATOR".to_string(),
            at: past,
        },
    );

    for index in 0..SEEDED_RECORDS {
        store.create_gateway(
            REGION,
            "SEEDED-ACTIVATION-KEY",
            &format!("gw-seeded-{index}"),
            BACKUP_VM,
            BTreeMap::new(),
            Creation {
                by: "SEEDED-CREATOR".to_string(),
                at: past,
            },
        );
        store
            .record_virtual_machine(
                REGION,
                &host_hypervisor.arn,
                VirtualMachineInventory {
                    name: format!("vm-seeded-{index}"),
                    host_name: format!("vm-seeded-{index}"),
                    path: format!("/Datacenter/vm/vm-seeded-{index}"),
                    last_backup_date: None,
                    vmware_tags: Vec::new(),
                },
                past,
            )
            .unwrap_or_else(|| panic!("case seed {index}: the store must record a machine"));
    }
    // The inventory host is one of the hypervisors the listing reports, so only the rest
    // are imported here.
    for index in 1..SEEDED_RECORDS {
        store.import_hypervisor(
            REGION,
            HypervisorConfiguration {
                name: format!("hv-seeded-{index}"),
                host: format!("hv-seeded-{index}.example.internal"),
                kms_key_arn: None,
                tags: BTreeMap::new(),
            },
            Creation {
                by: "SEEDED-CREATOR".to_string(),
                at: past,
            },
        );
    }

    assert_eq!(
        (
            store.gateway_count(),
            store.hypervisor_count(),
            store.virtual_machine_count()
        ),
        (SEEDED_RECORDS, SEEDED_RECORDS, SEEDED_RECORDS),
        "case seed: the store must hold {SEEDED_RECORDS} records of each type, got {} \
         gateways, {} hypervisors and {} machines",
        store.gateway_count(),
        store.hypervisor_count(),
        store.virtual_machine_count()
    );
}

/// Every listing of this service serves the same page size for the same `MaxResults`, and
/// that size is the one the rule states: the default when the request names none, else the
/// requested size capped at the server's maximum.
#[tokio::test]
async fn every_listing_serves_the_same_page_size() {
    let engine = build_engine();
    let router = build_router(&engine);
    seed(&engine);

    // The `MaxResults` each case asks for; `None` is "the request names none". Every
    // `Some` satisfies the modelled minimum of 1, so each reaches the handler rather than
    // the boundary validator.
    let cases: [Option<i32>; 8] = [
        None,
        Some(1),
        Some(2),
        Some(7),
        Some(MAX_PAGE_SIZE as i32 - 1),
        Some(MAX_PAGE_SIZE as i32),
        Some(MAX_PAGE_SIZE as i32 + 1),
        Some(i32::MAX),
    ];

    for case in cases {
        let expected = expected_page_size(case, SEEDED_RECORDS);
        for (operation, member) in LISTINGS {
            let body = match case {
                None => json!({}),
                Some(max) => json!({ "MaxResults": max }),
            };
            let reply = call(&router, operation, &body).await;
            assert_eq!(
                reply.status, 200,
                "case MaxResults={case:?} on {operation}: the listing must answer 200, \
                 got {} {:?}",
                reply.status, reply.body
            );
            let length = listed_len(&reply, member);
            assert_eq!(
                length, expected,
                "case MaxResults={case:?} on {operation}: the page must carry \
                 {expected} records, got {length}"
            );
            assert!(
                reply.next_token().is_some(),
                "case MaxResults={case:?} on {operation}: a page shorter than the store \
                 must carry a NextToken, got {:?}",
                reply.body
            );
        }
    }
}

/// The page size the listings read is a function of `MaxResults` alone, over every value
/// the member can take — including the values no request can reach a handler with,
/// because the modelled `@range(min: 1)` is refused at the boundary first.
#[test]
fn the_page_limit_spans_every_max_results() {
    // (case, MaxResults, the page size the rule states).
    let cases: [(&str, Option<i32>, usize); 9] = [
        ("no MaxResults", None, DEFAULT_PAGE_SIZE),
        ("the modelled minimum", Some(1), 1),
        ("inside the ceiling", Some(13), 13),
        ("one below the ceiling", Some(MAX_PAGE_SIZE as i32 - 1), MAX_PAGE_SIZE - 1),
        ("the ceiling itself", Some(MAX_PAGE_SIZE as i32), MAX_PAGE_SIZE),
        ("one past the ceiling", Some(MAX_PAGE_SIZE as i32 + 1), MAX_PAGE_SIZE),
        ("the largest integer", Some(i32::MAX), MAX_PAGE_SIZE),
        ("below the modelled minimum", Some(0), 1),
        ("the smallest integer", Some(i32::MIN), 1),
    ];

    for (case, max_results, expected) in cases {
        let limit = page_limit(max_results);
        assert_eq!(
            limit, expected,
            "case {case:?} (MaxResults={max_results:?}): the page size must be \
             {expected}, got {limit}"
        );
    }
}

/// A page the listings serve is never empty and never longer than the server's maximum,
/// whatever `MaxResults` asks for: the invariant the pages of every resource share.
#[tokio::test]
async fn a_served_page_is_bounded_at_both_ends() {
    let engine = build_engine();
    let router = build_router(&engine);
    seed(&engine);

    for max_results in [1, 2, 50, MAX_PAGE_SIZE as i32, i32::MAX] {
        for (operation, member) in LISTINGS {
            let reply = call(&router, operation, &json!({ "MaxResults": max_results })).await;
            assert_eq!(
                reply.status, 200,
                "case MaxResults={max_results} on {operation}: the listing must answer \
                 200, got {} {:?}",
                reply.status, reply.body
            );
            let length = listed_len(&reply, member);
            assert!(
                (1..=MAX_PAGE_SIZE).contains(&length),
                "case MaxResults={max_results} on {operation}: a page of a non-empty \
                 store must hold between 1 and {MAX_PAGE_SIZE} records, got {length}"
            );
        }
    }
}

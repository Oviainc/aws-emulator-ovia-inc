// SPDX-License-Identifier: CC0-1.0

//! Tests that everything a metadata sync publishes becomes reportable AT ONE INSTANT.
//!
//! The rule under test: a metadata sync publishes the hypervisor's successful-sync time and
//! status and the machines of the inventory it reports, and every one of them becomes
//! reportable at the one instant that sync falls due — so at every instant the hypervisor
//! reports a successful sync exactly when a listing reports its machines. A sync restarted
//! before the one in flight fell due supersedes it, so the superseded sync publishes none of
//! them at any instant; a sync that had already fallen due keeps what it published, at the
//! instant it fell due.
//!
//! Where the sibling `metadata_sync_restart_supersedes_the_pending_sync` suite observes that
//! over the wire — several replies, each answered at the instant its own handler read the
//! clock at, so what it observes is a window and not an instant — every case here is read AT
//! one chosen instant, through the very readers `GetHypervisor` and `ListVirtualMachines`
//! call with the clock (`StoredHypervisor::latest_metadata_sync_status_as_of`,
//! `StoredHypervisor::last_successful_metadata_sync_time_as_of` and
//! `page_of_virtual_machines`). Nothing here depends on how long a read takes, so the
//! biconditional is asserted exactly where it is sharpest: on either side of each due
//! instant, one millisecond away.
//!
//! The table spans the class rather than one request. The rows are the restart points of the
//! sibling suite (`rows`) — a restart inside `CREATED`, at the `CREATED`/`RUNNING` boundary,
//! one millisecond before the first sync fell due, exactly at its due instant and well after
//! it, beside hypervisors differing in name length and in host form — and each row is
//! observed at every instant either sync's report turns on, and one millisecond either side
//! of each due instant. Each row is also read back over the WIRE, at an instant an hour past
//! every one of its due instants, so the operations are driven end-to-end where no reply can
//! land on the far side of a transition from another's.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    service_state, MAX_PAGE_SIZE, METADATA_SYNC_RUNNING_AFTER, METADATA_SYNC_SUCCEEDS_AFTER,
    SYNCED_VIRTUAL_MACHINES, SYNC_METADATA_CREATED, SYNC_METADATA_RUNNING, SYNC_METADATA_SUCCEEDED,
};

use crate::gateway_lifecycle::{
    build_engine, build_router, call, epoch_secs, INSTANT_TOLERANCE_SECS, REGION,
};
use crate::hypervisor_property_mappings::import;
use crate::metadata_sync_restart_supersedes_the_pending_sync::{rows, start_sync_at};

/// How long before the wire reads below each row's second sync is started, so every instant
/// of either sync's span is in the past by then and what those reads report cannot depend on
/// how long a read takes.
const SETTLED: Duration = Duration::from_secs(3_600);

/// The step the table takes either side of a due instant.
const A_MILLISECOND: Duration = Duration::from_millis(1);

/// What the store reports about a hypervisor and its inventory AT one instant.
#[derive(Debug)]
struct ObservedAt {
    status: Option<String>,
    success_time: Option<SystemTime>,
    listed: Vec<String>,
}

/// Read the hypervisor and its inventory AT `at`, through the readers the `GetHypervisor`
/// and `ListVirtualMachines` handlers call with the current instant — so one observation is
/// one instant, which no pair of wire calls can be.
fn observe_at(
    engine: &Arc<engine::InMemoryEngine>,
    hypervisor_arn: &str,
    at: SystemTime,
) -> ObservedAt {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let store = state.lock();
    let record = store
        .hypervisor(hypervisor_arn)
        .unwrap_or_else(|| panic!("no hypervisor stored under {hypervisor_arn}"));
    ObservedAt {
        status: record.latest_metadata_sync_status_as_of(at),
        success_time: record.last_successful_metadata_sync_time_as_of(at),
        listed: store
            .page_of_virtual_machines(REGION, Some(hypervisor_arn), None, MAX_PAGE_SIZE, at)
            .items
            .iter()
            .map(|machine| machine.arn.clone())
            .collect(),
    }
}

/// The instants one row is observed at: every instant a report of either sync turns on, and
/// one millisecond either side of each due instant.
///
/// None of them is before `second`, the instant the restart was written: the store did not
/// hold that sync any earlier, so an earlier instant is not one any client could have read
/// this record at.
fn observation_instants(
    first: SystemTime,
    second: SystemTime,
) -> Vec<(&'static str, SystemTime)> {
    let first_due = first + METADATA_SYNC_SUCCEEDS_AFTER;
    let second_due = second + METADATA_SYNC_SUCCEEDS_AFTER;
    [
        ("the instant the restart was started at", second),
        (
            "the instant the restart turns RUNNING",
            second + METADATA_SYNC_RUNNING_AFTER,
        ),
        (
            "a millisecond before the first sync's due instant",
            first_due - A_MILLISECOND,
        ),
        ("the first sync's due instant", first_due),
        (
            "a millisecond after the first sync's due instant",
            first_due + A_MILLISECOND,
        ),
        (
            "a millisecond before the restart's due instant",
            second_due - A_MILLISECOND,
        ),
        ("the restart's due instant", second_due),
        (
            "a millisecond after the restart's due instant",
            second_due + A_MILLISECOND,
        ),
        ("an hour after the restart's due instant", second_due + SETTLED),
    ]
    .into_iter()
    .filter(|(_, at)| *at >= second)
    .collect()
}

/// Every fact a sync publishes becomes reportable at the instant that sync falls due, and a
/// sync superseded before its own due instant publishes none of them at any instant.
#[tokio::test]
async fn every_fact_a_sync_publishes_becomes_reportable_at_the_instant_it_falls_due() {
    let engine = build_engine();
    let router = build_router(&engine);

    for row in rows() {
        let hypervisor = import(&router, row.name, row.host).await;

        // Both syncs are started AT chosen instants, the restart long enough ago that every
        // instant of either span is past by the time the wire reads below land.
        let second = SystemTime::now() - SETTLED;
        let first = second - row.gap;
        start_sync_at(&engine, &hypervisor, first);
        start_sync_at(&engine, &hypervisor, second);

        let first_due = first + METADATA_SYNC_SUCCEEDS_AFTER;
        let second_due = second + METADATA_SYNC_SUCCEEDS_AFTER;
        // The first sync stands only if it had already fallen due when the restart was
        // started; otherwise the restart supersedes it, and what it would have published is
        // published at the restart's due instant instead.
        let stood = second >= first_due;
        let published_at = if stood { first_due } else { second_due };

        for (when, at) in observation_instants(first, second) {
            let case = format!("{} / {when}", row.case);
            let observed = observe_at(&engine, &hypervisor, at);

            let expected_success = if at >= second_due {
                Some(second_due)
            } else if stood {
                Some(first_due)
            } else {
                None
            };
            assert_eq!(
                observed.success_time, expected_success,
                "case {case:?}: the successful-sync time reported at {at:?} must be the due \
                 instant of the last sync that completed, got {observed:?}"
            );

            let expected_listed = if at >= published_at {
                SYNCED_VIRTUAL_MACHINES
            } else {
                0
            };
            assert_eq!(
                observed.listed.len(),
                expected_listed,
                "case {case:?}: the inventory reported at {at:?} must be the one the sync \
                 that published it reports, got {observed:?}"
            );

            // The two are halves of one fact, so neither is ever reportable without the
            // other — the biconditional, at this one instant.
            assert_eq!(
                !observed.listed.is_empty(),
                observed.success_time.is_some(),
                "case {case:?}: a listing reports machines exactly when the hypervisor \
                 reports a successful metadata sync, got {observed:?}"
            );

            let expected_status = if at >= second_due {
                SYNC_METADATA_SUCCEEDED
            } else if at >= second + METADATA_SYNC_RUNNING_AFTER {
                SYNC_METADATA_RUNNING
            } else {
                SYNC_METADATA_CREATED
            };
            assert_eq!(
                observed.status.as_deref(),
                Some(expected_status),
                "case {case:?}: the status reported at {at:?} must be the point of the sync \
                 in flight the clock has reached, got {observed:?}"
            );
        }

        // ... and that is what a client reads over the wire, at an instant every one of this
        // row's due instants is an hour past — so no reply here can land on the far side of
        // a transition from another's.
        let case = row.case;
        let read = call(
            &router,
            "GetHypervisor",
            &json!({ "HypervisorArn": hypervisor }),
        )
        .await;
        assert_eq!(
            (
                read.status,
                read.str_at("/Hypervisor/LatestMetadataSyncStatus")
            ),
            (200, Some(SYNC_METADATA_SUCCEEDED)),
            "case {case:?}: a sync that fell due an hour ago must read back as SUCCEEDED, \
             got {} {:?}",
            read.status,
            read.body
        );
        let reported = read
            .body
            .pointer("/Hypervisor/LastSuccessfulMetadataSyncTime")
            .and_then(Value::as_f64);
        let due = epoch_secs(second_due);
        assert!(
            reported.is_some_and(|secs| (secs - due).abs() < INSTANT_TOLERANCE_SECS),
            "case {case:?}: the wire must date the success at the instant the sync in flight \
             fell due ({due}), got {reported:?} in {:?}",
            read.body
        );

        let listing = call(
            &router,
            "ListVirtualMachines",
            &json!({ "HypervisorArn": hypervisor }),
        )
        .await;
        let listed: Vec<String> = listing
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
        assert_eq!(
            (listing.status, listed.len()),
            (200, SYNCED_VIRTUAL_MACHINES),
            "case {case:?}: the listing must report the machines the sync published, got {} \
             {:?}",
            listing.status,
            listing.body
        );
        assert_eq!(
            listed,
            observe_at(&engine, &hypervisor, SystemTime::now()).listed,
            "case {case:?}: the listing must report exactly the machines the store reports \
             at the instant it was read"
        );

        // Each of them reads back on its own ARN, so the inventory the listing hands a
        // client is one every other read of this service resolves too.
        for arn in &listed {
            let machine = call(
                &router,
                "GetVirtualMachine",
                &json!({ "ResourceArn": arn }),
            )
            .await;
            assert_eq!(
                (
                    machine.status,
                    machine.str_at("/VirtualMachine/ResourceArn")
                ),
                (200, Some(arn.as_str())),
                "case {case:?}: a listed machine must read back on its own ARN, got {} {:?}",
                machine.status,
                machine.body
            );
        }
    }
}

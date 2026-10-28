// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a metadata sync RESTARTED before the previous one fell due.
//!
//! Every case reads the service over the wire — a real `POST /` awsJson1_0 request through
//! the `MultiServiceRouter`, reusing the harness of `gateway_lifecycle` and the helpers of
//! the sibling `metadata_sync` suites — so what is asserted is what a client observes.
//!
//! The rule under test: starting a sync SUPERSEDES one the clock has not yet made due, so
//! everything that sync would have published is published at the due instant of the sync
//! that is in flight and not at the abandoned one's — the hypervisor's successful-sync
//! time and status, and the machines of the inventory the sync reports, become true
//! together at one instant. A fact an earlier sync already published stands: an inventory a
//! listing already reported keeps the instant it became visible at, and the time of a sync
//! that really completed never disappears because another one was started.
//!
//! What a client can observe of that is the biconditional every case below asserts at every
//! observation: `ListVirtualMachines` reports machines for a hypervisor exactly when
//! `GetHypervisor` reports a `LastSuccessfulMetadataSyncTime` for it, and every machine the
//! listing reports is readable through `GetVirtualMachine` while every machine the store
//! holds but the listing does not report is refused by it.
//!
//! The table spans the class rather than one request: the restart lands at a point of the
//! first sync's span in each row — early in `CREATED`, at the `CREATED`/`RUNNING` boundary,
//! inside `RUNNING`, one millisecond before the first sync fell due, exactly at the instant
//! it fell due, and well after it — beside hypervisors differing in name length and in host
//! form (IPv4, DNS, IPv6). The first three rows are restarts of a sync that had NOT fallen
//! due (the superseded case), the last two restarts of one that HAD (the standing case).
//!
//! Where a case needs the first sync to have started a known span ago, the sync is started
//! AT a chosen instant through the store helper the handler calls
//! (`state::start_metadata_sync`, which takes that instant) instead of sleeping: a sync's
//! progress is a function of that instant and the clock, so a sync started earlier and one
//! observed later are the same observation. The last case here drives both starts over the
//! wire with a real wait between them, so the handler's own path is covered too.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    service_state, MAX_PAGE_SIZE, METADATA_SYNC_RUNNING_AFTER, METADATA_SYNC_SUCCEEDS_AFTER,
    SYNCED_VIRTUAL_MACHINES, SYNC_METADATA_CREATED, SYNC_METADATA_RUNNING, SYNC_METADATA_SUCCEEDED,
};

use crate::gateway_lifecycle::{build_engine, build_router, call, Reply, REGION};
use crate::hypervisor_property_mappings::import;
use crate::metadata_sync_over_the_wire::start_sync;

/// The tolerance a reported instant is compared with: the wire carries the modelled `Time`
/// as epoch seconds, so a millisecond of rounding is not a difference.
const INSTANT_TOLERANCE_SECS: f64 = 0.002;

/// Start a metadata sync on `arn` AT `at`, through the store helper the
/// `StartVirtualMachinesMetadataSync` handler calls with the current instant.
fn start_sync_at(engine: &Arc<engine::InMemoryEngine>, arn: &str, at: SystemTime) {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    store
        .start_metadata_sync(arn, at)
        .unwrap_or_else(|| panic!("no hypervisor stored under {arn}"));
}

/// Every virtual machine the STORE holds for `hypervisor_arn`, whatever the clock has
/// reached — read with an instant far in the future, so a machine no read reports yet is in
/// the list too.
fn stored_machine_arns(engine: &Arc<engine::InMemoryEngine>, hypervisor_arn: &str) -> Vec<String> {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let store = state.lock();
    store
        .page_of_virtual_machines(
            REGION,
            Some(hypervisor_arn),
            None,
            MAX_PAGE_SIZE,
            SystemTime::now() + Duration::from_secs(3_600),
        )
        .items
        .iter()
        .map(|machine| machine.arn.clone())
        .collect()
}

/// `time` as epoch seconds, the form the modelled `Time` timestamp takes on the wire.
fn epoch_secs(time: SystemTime) -> f64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test clock is past the epoch")
        .as_secs_f64()
}

/// What one observation of a hypervisor saw: the sync members `GetHypervisor` reported and
/// the machines `ListVirtualMachines` reported for it.
#[derive(Debug)]
struct Observed {
    status: Option<String>,
    success_time: Option<f64>,
    listed: Vec<String>,
}

/// Read a hypervisor and its inventory at one instant, and assert the two agree: a listing
/// reports machines exactly when the hypervisor reports a successful sync, and the listing
/// and `GetVirtualMachine` resolve the same machines.
async fn observe(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    hypervisor_arn: &str,
) -> Observed {
    let read = call(
        router,
        "GetHypervisor",
        &json!({ "HypervisorArn": hypervisor_arn }),
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: GetHypervisor must answer 200, got {} {:?}",
        read.status, read.body
    );
    let listing = call(
        router,
        "ListVirtualMachines",
        &json!({ "HypervisorArn": hypervisor_arn }),
    )
    .await;
    assert_eq!(
        listing.status, 200,
        "case {case:?}: ListVirtualMachines must answer 200, got {} {:?}",
        listing.status, listing.body
    );

    let observed = Observed {
        status: read
            .str_at("/Hypervisor/LatestMetadataSyncStatus")
            .map(str::to_string),
        success_time: read
            .body
            .pointer("/Hypervisor/LastSuccessfulMetadataSyncTime")
            .and_then(Value::as_f64),
        listed: listing
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
            .unwrap_or_default(),
    };

    // The one fact the two reads are halves of: an inventory this service reports is an
    // inventory a sync of it has successfully produced.
    assert_eq!(
        !observed.listed.is_empty(),
        observed.success_time.is_some(),
        "case {case:?}: a listing reports machines exactly when the hypervisor reports a \
         successful metadata sync, got {observed:?}"
    );

    // ... and a machine is listed exactly when it is readable on its own.
    for machine_arn in stored_machine_arns(engine, hypervisor_arn) {
        let reply = read_machine(router, &machine_arn).await;
        let expected: (u16, Option<&str>) = if observed.listed.contains(&machine_arn) {
            (200, None)
        } else {
            (404, Some("ResourceNotFoundException"))
        };
        assert_eq!(
            (reply.status, reply.code.as_deref()),
            expected,
            "case {case:?}: GetVirtualMachine must resolve exactly the machines the listing \
             reports ({:?}), got {} {:?} for {machine_arn}",
            observed.listed,
            reply.status,
            reply.body
        );
    }

    observed
}

async fn read_machine(router: &MultiServiceRouter, resource_arn: &str) -> Reply {
    call(
        router,
        "GetVirtualMachine",
        &json!({ "ResourceArn": resource_arn }),
    )
    .await
}

/// One hypervisor whose sync is restarted at a chosen point of the first sync's span.
struct Row {
    case: &'static str,
    name: &'static str,
    host: &'static str,
    /// How long after the first sync started the second one is started.
    gap: Duration,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "restarted early, while the first sync was still CREATED",
            name: "hv-restart-created",
            host: "192.0.2.140",
            gap: Duration::from_millis(200),
        },
        Row {
            case: "restarted as the first sync turned RUNNING",
            name: "h",
            host: "hv-restart-running.example.com",
            gap: METADATA_SYNC_RUNNING_AFTER,
        },
        Row {
            case: "restarted one millisecond before the first sync fell due",
            name: "hv-restart-almost-due",
            host: "2001:db8::140",
            gap: METADATA_SYNC_SUCCEEDS_AFTER - Duration::from_millis(1),
        },
        Row {
            case: "restarted exactly at the instant the first sync fell due",
            name: "hv-restart-at-the-due-instant",
            host: "198.51.100.140",
            gap: METADATA_SYNC_SUCCEEDS_AFTER,
        },
        Row {
            case: "restarted well after the first sync had fallen due",
            name: "hv-restart-after-the-due-instant",
            host: "203.0.113.140",
            gap: METADATA_SYNC_SUCCEEDS_AFTER + Duration::from_millis(250),
        },
    ]
}

/// A restart supersedes a sync the clock had not yet made due: in the window after the
/// superseded sync's due instant and before the new one's, the hypervisor reports no
/// successful sync and the listing reports no machine — and once the sync in flight falls
/// due, both become true at its instant. A sync that HAD fallen due before the restart
/// stands instead: its inventory keeps being reported and its success keeps being reported,
/// at the instant it fell due.
#[tokio::test]
async fn a_restart_supersedes_a_sync_that_had_not_fallen_due_and_keeps_one_that_had() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows = rows();
    let mut hypervisors: Vec<(&Row, String)> = Vec::new();
    for row in &rows {
        hypervisors.push((row, import(&router, row.name, row.host).await));
    }

    // Every row is arranged against one anchor: the instant the FIRST sync of a superseded
    // row falls due. So the observation below lands after that instant for every row, and
    // before the instant the sync in flight falls due.
    let anchor = SystemTime::now();
    let mut scenes: Vec<(&Row, String, SystemTime, SystemTime)> = Vec::new();
    for (row, hypervisor) in hypervisors {
        let first = anchor - row.gap.max(METADATA_SYNC_SUCCEEDS_AFTER);
        let second = first + row.gap;
        start_sync_at(&engine, &hypervisor, first);
        start_sync_at(&engine, &hypervisor, second);
        scenes.push((row, hypervisor, first, second));
    }

    for (row, hypervisor, first, _) in &scenes {
        let observed = observe(&router, &engine, row.case, hypervisor).await;
        let restarted_before_due = row.gap < METADATA_SYNC_SUCCEEDS_AFTER;
        // The sync in flight has not fallen due at this observation, whichever row it is,
        // so it has published nothing of its own yet; which point of ITS span the read
        // lands in is the clock's answer, so both are accepted.
        let status = observed.status.as_deref();
        assert!(
            status == Some(SYNC_METADATA_CREATED) || status == Some(SYNC_METADATA_RUNNING),
            "case {:?}: the sync in flight must read back as CREATED or RUNNING, got \
             {observed:?}",
            row.case
        );
        if restarted_before_due {
            assert_eq!(
                (observed.success_time, observed.listed.as_slice()),
                (None, [].as_slice()),
                "case {:?}: a sync superseded before it fell due must publish neither a \
                 successful sync nor an inventory, got {observed:?}",
                row.case
            );
        } else {
            // The sync that really completed keeps its success, dated at the instant it
            // fell due, and the inventory it published stays published.
            let first_due = epoch_secs(*first + METADATA_SYNC_SUCCEEDS_AFTER);
            assert!(
                observed
                    .success_time
                    .is_some_and(|secs| (secs - first_due).abs() < INSTANT_TOLERANCE_SECS)
                    && observed.listed.len() == SYNCED_VIRTUAL_MACHINES,
                "case {:?}: a sync that had fallen due before the restart must keep its \
                 success (around {first_due}) and its inventory ({SYNCED_VIRTUAL_MACHINES} \
                 machines), got {observed:?}",
                row.case
            );
        }
    }

    // Nothing but the clock carries the syncs in flight along. Wait past the latest instant
    // one of them falls due.
    let last_due = scenes
        .iter()
        .map(|(_, _, _, second)| *second + METADATA_SYNC_SUCCEEDS_AFTER)
        .max()
        .expect("the table has rows");
    std::thread::sleep(
        last_due
            .duration_since(SystemTime::now())
            .unwrap_or_default()
            + Duration::from_millis(150),
    );

    for (row, hypervisor, _, second) in &scenes {
        let observed = observe(&router, &engine, row.case, hypervisor).await;
        let due = epoch_secs(*second + METADATA_SYNC_SUCCEEDS_AFTER);
        assert_eq!(
            observed.status.as_deref(),
            Some(SYNC_METADATA_SUCCEEDED),
            "case {:?}: the sync in flight must have succeeded, got {observed:?}",
            row.case
        );
        assert!(
            observed
                .success_time
                .is_some_and(|secs| (secs - due).abs() < INSTANT_TOLERANCE_SECS),
            "case {:?}: the succeeded sync must be dated at the instant it fell due \
             ({due}), got {observed:?}",
            row.case
        );
        assert_eq!(
            observed.listed.len(),
            SYNCED_VIRTUAL_MACHINES,
            "case {:?}: the succeeded sync must report the hypervisor's machines, got \
             {observed:?}",
            row.case
        );
        // The two starts synced one hypervisor, so the store holds exactly the machines
        // that hypervisor reports — no second copy of one, re-dated or not.
        assert_eq!(
            stored_machine_arns(&engine, hypervisor),
            observed.listed,
            "case {:?}: the store must hold exactly the machines the listing reports",
            row.case
        );
    }
}

/// The same rule through the handler: two `StartVirtualMachinesMetadataSync` calls over the
/// wire, the second one while the first sync is still in flight, and the inventory and the
/// successful sync a client can read stay one fact at every instant in between.
#[tokio::test]
async fn a_restart_over_the_wire_keeps_the_inventory_and_the_successful_sync_together() {
    let engine = build_engine();
    let router = build_router(&engine);

    let hypervisor = import(&router, "hv-restart-over-the-wire", "192.0.2.141").await;

    let first = start_sync(&router, &hypervisor).await;
    assert_eq!(
        first.status, 200,
        "the first start must answer 200, got {} {:?}",
        first.status, first.body
    );

    // A quarter of the sync's span: long enough that the window the superseded sync would
    // have published its inventory in is observable, short enough that the first sync is
    // certainly still in flight when the second start lands.
    std::thread::sleep(METADATA_SYNC_SUCCEEDS_AFTER / 4);
    let second = start_sync(&router, &hypervisor).await;
    assert_eq!(
        second.status, 200,
        "the second start must answer 200, got {} {:?}",
        second.status, second.body
    );

    let restarted = observe(&router, &engine, "just after the restart", &hypervisor).await;
    assert_eq!(
        (restarted.success_time, restarted.listed.as_slice()),
        (None, [].as_slice()),
        "the first sync must still have been in flight when the second start landed, so \
         nothing is published yet, got {restarted:?}"
    );

    // Poll across the whole span of both syncs: at every instant the listing and the
    // hypervisor agree ([`observe`] asserts it), whichever side of either due instant the
    // poll lands on.
    let deadline = SystemTime::now() + 2 * METADATA_SYNC_SUCCEEDS_AFTER;
    let mut succeeded = None;
    while SystemTime::now() < deadline {
        let observed = observe(&router, &engine, "polled across both spans", &hypervisor).await;
        if observed.status.as_deref() == Some(SYNC_METADATA_SUCCEEDED) {
            succeeded = Some(observed);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let finished = succeeded.unwrap_or_else(|| {
        panic!("the restarted sync must fall due inside twice its own span");
    });
    assert_eq!(
        finished.listed.len(),
        SYNCED_VIRTUAL_MACHINES,
        "the sync that fell due must report the hypervisor's machines, got {finished:?}"
    );
    assert!(
        finished.success_time.is_some(),
        "the sync that fell due must report the instant it succeeded at, got {finished:?}"
    );
}

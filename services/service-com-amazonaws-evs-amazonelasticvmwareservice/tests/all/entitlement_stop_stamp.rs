// SPDX-License-Identifier: CC0-1.0

//! When `DeleteEntitlement` dates the stop it reports, over awsJson1_0.
//!
//! The rule under test: `stoppedAt` is "The date and time that the entitlement stopped"
//! and `startedAt` "The date and time that the entitlement started"
//! (`smithy_query(query="doc",shape="VmEntitlement",member="stoppedAt",model_path="services/service-com-amazonaws-evs-amazonelasticvmwareservice/smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json")`,
//! and the same sentence for `startedAt`), so the stop is dated at the instant it was
//! APPLIED to this service's store — not the instant the request arrived — which makes it
//! ONE instant per call, carried by every record that call stops, and never earlier than
//! the `startedAt` the same call's materialisation of an owed validation wrote onto a
//! record it is about to stop. Tracking cannot stop before it started.
//!
//! Each row drives the ordering the rule exists for, and drives it deterministically: the
//! test takes this service's store lock itself, issues the delete while holding it, keeps
//! it until past the instant the validation in flight is due, and only then releases it.
//! The delete therefore arrives BEFORE that validation is due and is applied AFTER it,
//! which is the window in which an instant read on the request's way in would be earlier
//! than the `startedAt` the delete itself wrote.
//!
//! What the rule generalises over, and so what the table spans: HOW MANY records one call
//! stops (one, two, three), whether each record was already tracking when the call arrived
//! or had its validation still in flight, a MIX of the two in one call, a record the call
//! leaves behind, a `vmId` named twice, and a `vmId` the environment holds no record for.
//!
//! `tests/all/child_record_create_stamp.rs` owns the same rule for the calls that ADD a
//! record an environment holds, and `tests/all/record_stamp_backwards_guard.rs` for the
//! calls that mutate one that already exists; this suite owns it for `stoppedAt` — the
//! member only `DeleteEntitlement` and the deletion of a whole environment write — over a
//! SET of records stopped by one call.
//!
//! Every test drives the service the way a client does, through the same
//! `MultiServiceRouter` the production router mounts, and reads each record back through
//! its own lister.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::delete_entitlement::{
    assert_stopped, delete_request, entitlements, entitle, list_all, vms,
};
use crate::entitlement_connector::environment_with_vcenter_connector;
use crate::environment_lifecycle::{call, emulator, error_type, Emulator, PAST_THE_TRANSITION};
use crate::record_stamp_backwards_guard::{environment_stamp, held_for, member, wire_ms};

/// The status a record whose validation has come due is reported in
/// (`smithy_query(query="shape",shape="EntitlementStatus")` declares it).
const CREATED: &str = "CREATED";

/// One row of the table: the records the environment holds when the delete arrives, and
/// the virtual machines that delete names.
struct Row {
    case: &'static str,
    /// Entitled first and left until the validation the clock owes them has come due, so
    /// each is tracking — and carries a `startedAt` — before the call under test arrives.
    tracking: &'static [&'static str],
    /// Entitled immediately before the call under test, so the validation the clock owes
    /// each is still in flight as that call arrives and comes due while it waits for the
    /// store lock. These are the records whose `startedAt` the delete itself writes.
    being_validated: &'static [&'static str],
    /// The `vmIds` the delete names.
    named: &'static [&'static str],
}

/// The `vmId`s of a list of reported entitlements, in the order they were reported.
fn reported_vm_ids(case: &str, entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| member(case, entry, "vmId"))
        .collect()
}

/// The records a row's delete removes, in the order the request names their virtual
/// machines: the DISTINCT names it carries that the environment holds a record for.
fn stopped_by(row: &Row) -> Vec<String> {
    let mut expected: Vec<String> = Vec::new();
    for vm_id in row.named {
        let held = row.tracking.contains(vm_id) || row.being_validated.contains(vm_id);
        if held && !expected.iter().any(|already| already == vm_id) {
            expected.push((*vm_id).to_string());
        }
    }
    expected
}

/// The records a row's delete leaves behind: the ones the environment holds that it does
/// not name.
fn left_by(row: &Row) -> Vec<String> {
    let mut left: Vec<String> = Vec::new();
    for vm_id in row.tracking.iter().chain(row.being_validated) {
        if !row.named.contains(vm_id) && !left.iter().any(|already| already == vm_id) {
            left.push((*vm_id).to_string());
        }
    }
    left
}

/// The instant the validation the clock still owes each named entitlement comes due, as
/// this service's store holds it — the instant the materialisation inside the call under
/// test dates that record's `startedAt` at.
fn pending_due_instants(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    vm_ids: &[&str],
) -> Vec<(String, i64)> {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    let environment = store
        .environments
        .get(environment_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {environment_id}"));
    vm_ids
        .iter()
        .map(|vm_id| {
            let entitlement = environment
                .entitlements
                .values()
                .find(|entitlement| entitlement.vm_id == **vm_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the environment holds an entitlement of {vm_id}")
                });
            let pending = entitlement.pending.as_ref().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the entitlement of {vm_id} must still owe its validation, \
                     got status {}",
                    entitlement.status
                )
            });
            ((*vm_id).to_string(), pending.due_at_ms)
        })
        .collect()
}

/// Entitle a row's virtual machines and answer what the setup established: the
/// `startedAt` each record carries once it is tracking, keyed by `vmId`, and the instants
/// the validations still in flight are due at.
///
/// The records that are tracking before the call under test have their instant read off
/// the lister; the records whose validation that call materialises have theirs off the
/// store's own due instant, which is what the materialisation writes.
async fn prepare(
    router: &Emulator,
    row: &Row,
    environment: &str,
    connector: &str,
) -> (BTreeMap<String, i64>, Vec<(String, i64)>) {
    let case = row.case;
    let mut expected: BTreeMap<String, i64> = BTreeMap::new();
    if !row.tracking.is_empty() {
        entitle(router, case, environment, connector, &vms(row.tracking)).await;
        // The span this service owes a validation in, slept through rather than settled
        // by hand, because this suite asserts the instants themselves.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        for entry in list_all(router, case, environment, connector).await {
            let vm_id = member(case, &entry, "vmId");
            assert_eq!(
                entry["status"],
                json!(CREATED),
                "case {case:?}: the validation of {vm_id} must have come due, got {entry}"
            );
            let label = format!("the entitlement of {vm_id}");
            expected.insert(vm_id, wire_ms(case, &label, &entry["startedAt"]));
        }
    }
    let mut due_instants: Vec<(String, i64)> = Vec::new();
    if !row.being_validated.is_empty() {
        entitle(router, case, environment, connector, &vms(row.being_validated)).await;
        due_instants = pending_due_instants(router, case, environment, row.being_validated);
        for (vm_id, due_at_ms) in &due_instants {
            expected.insert(vm_id.clone(), *due_at_ms);
        }
    }
    (expected, due_instants)
}

/// The `startedAt` expected for one record, as the setup established it.
fn started_of(case: &str, expected: &BTreeMap<String, i64>, vm_id: &str) -> i64 {
    *expected
        .get(vm_id)
        .unwrap_or_else(|| panic!("case {case:?}: the setup recorded no startedAt for {vm_id}"))
}

// Two worker threads: the delete under test BLOCKS one of them on this service's store
// lock while the test holds it, so a second is needed to run the test's own task on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_delete_stops_every_record_it_names_at_the_instant_it_was_applied() {
    let rows: [Row; 5] = [
        Row {
            case: "one record whose validation the delete itself materialises",
            tracking: &[],
            being_validated: &["vm-1"],
            named: &["vm-1"],
        },
        Row {
            case: "one record that was already tracking",
            tracking: &["vm-2"],
            being_validated: &[],
            named: &["vm-2"],
        },
        Row {
            case: "three records whose validation the delete itself materialises",
            tracking: &[],
            being_validated: &["vm-10", "vm-11", "vm-12"],
            named: &["vm-12", "vm-10", "vm-11"],
        },
        Row {
            case: "a tracking record and a validating one, with a third left behind",
            tracking: &["vm-20", "vm-21"],
            being_validated: &["vm-22"],
            named: &["vm-20", "vm-22"],
        },
        Row {
            case: "a vmId named twice beside one the environment never entitled",
            tracking: &["vm-30"],
            being_validated: &["vm-31"],
            named: &["vm-31", "vm-31", "vm-30", "vm-99"],
        },
    ];

    for row in rows {
        let case = row.case;
        let router = Arc::new(emulator());
        let (environment, connector) = environment_with_vcenter_connector(&router, case).await;
        // `due_instants` are the instants the validations still in flight are due at: the
        // delete has to arrive before them, and be applied after them, for the window to
        // be the one the rule is about.
        let (expected_started, due_instants) =
            prepare(&router, &row, &environment, &connector).await;

        // This service's store lock, taken and HELD past the instant those validations are
        // due, on a thread of its own so the delete — issued below while it is held —
        // cannot be applied until it is released. The thread SIGNALS once it holds the
        // lock, so the delete is issued against a lock that is provably held rather than
        // after a span assumed to be long enough. Answers the instant it was released at.
        let (held, taken) = ::tokio::sync::oneshot::channel();
        let holding = tokio::task::spawn_blocking({
            let engine = Arc::clone(&router.engine);
            move || {
                let state = evs::state::service_state(&*engine)
                    .expect("the in-memory engine carries this service's store");
                let store = state.lock();
                let _ = held.send(());
                std::thread::sleep(held_for());
                let released_at_ms = evs::state::now_ms();
                drop(store);
                released_at_ms
            }
        });
        taken.await.expect("the store lock was taken");

        let arrived_at_ms = evs::state::now_ms();
        for (vm_id, due_at_ms) in &due_instants {
            assert!(
                arrived_at_ms < *due_at_ms,
                "case {case:?}: the delete must arrive before the validation of {vm_id} is due \
                 ({due_at_ms}), got {arrived_at_ms}"
            );
        }

        let waiting = tokio::spawn({
            let router = Arc::clone(&router);
            let body = delete_request(&environment, &connector, &vms(row.named));
            async move { call(&router, "DeleteEntitlement", &body).await }
        });
        let released_at_ms = holding.await.expect("the lock was held and released");
        let (status, answered) = waiting.await.expect("the delete ran to completion");
        assert_eq!(
            status, 200,
            "case {case:?}: the delete expected 200, got {status} ({}) {answered}",
            error_type(&answered)
        );
        for (vm_id, due_at_ms) in &due_instants {
            assert!(
                released_at_ms >= *due_at_ms,
                "case {case:?}: the lock must be released after the validation of {vm_id} came \
                 due ({due_at_ms}), got {released_at_ms}"
            );
        }

        // Exactly the records the request named that the environment held, in the order it
        // named their virtual machines.
        let reported = entitlements(case, &answered);
        assert_eq!(
            reported_vm_ids(case, &reported),
            stopped_by(&row),
            "case {case:?}: the delete must report exactly the records it removed, got {answered}"
        );

        // ONE instant for the whole call, dated when the stop was applied — which cannot
        // be before the lock was released, and so is not the instant the request arrived
        // at.
        let mut stopped_instants: Vec<i64> = Vec::new();
        for entry in &reported {
            assert_stopped(case, entry, &environment, &connector);
            let vm_id = member(case, entry, "vmId");
            let stopped_ms = wire_ms(case, &format!("the stopped {vm_id}"), &entry["stoppedAt"]);
            assert!(
                stopped_ms >= released_at_ms,
                "case {case:?}: {vm_id} must be stopped at the instant the stop was applied, \
                 which is no earlier than {released_at_ms} (the request arrived at \
                 {arrived_at_ms}), got {stopped_ms}"
            );
            // And so at or after the instant tracking started — the one the validation
            // this very call materialised came due at.
            let started_ms = wire_ms(case, &format!("the stopped {vm_id}"), &entry["startedAt"]);
            assert_eq!(
                started_ms,
                started_of(case, &expected_started, &vm_id),
                "case {case:?}: {vm_id} must report the instant its validation came due as \
                 startedAt, got {entry}"
            );
            assert!(
                stopped_ms >= started_ms,
                "case {case:?}: tracking of {vm_id} cannot stop before it started \
                 ({started_ms}), got {stopped_ms}"
            );
            stopped_instants.push(stopped_ms);
        }
        let first = stopped_instants.first().copied().unwrap_or_default();
        assert!(
            stopped_instants.iter().all(|stopped| *stopped == first),
            "case {case:?}: every record one call stops carries the same stoppedAt ({first}), \
             got {stopped_instants:?}"
        );

        // The environment's inventory changed, so the environment is re-stamped at that
        // very instant.
        let environment_ms = environment_stamp(&router, case, &environment);
        assert_eq!(
            environment_ms, first,
            "case {case:?}: removing a record must re-stamp the environment at the instant the \
             stop was applied ({first}), got {environment_ms}"
        );

        // Persisted: the lister reports the records the delete left behind, each still
        // tracking from the instant it started and stopped by nothing.
        let left = list_all(&router, case, &environment, &connector).await;
        assert_eq!(
            reported_vm_ids(case, &left),
            left_by(&row),
            "case {case:?}: the lister must report exactly the records the delete left, got \
             {left:?}"
        );
        for entry in &left {
            let vm_id = member(case, entry, "vmId");
            assert_eq!(
                entry["stoppedAt"],
                Value::Null,
                "case {case:?}: {vm_id} was not named, so nothing stopped it, got {entry}"
            );
            let started_ms = wire_ms(case, &format!("the surviving {vm_id}"), &entry["startedAt"]);
            assert_eq!(
                started_ms,
                started_of(case, &expected_started, &vm_id),
                "case {case:?}: {vm_id} must keep the instant its validation came due as \
                 startedAt, got {entry}"
            );
        }
    }
}

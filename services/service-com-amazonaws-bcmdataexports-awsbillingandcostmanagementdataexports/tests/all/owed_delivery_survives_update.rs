// SPDX-License-Identifier: CC0-1.0

//! What the clock already owed an export when `UpdateExport` reached the store, over the
//! awsJson1_1 wire: `CreateExport` -> `UpdateExport` -> `ListExecutions` / `GetExecution` /
//! `GetExport`.
//!
//! The rule under test: a transition the CLOCK owes is owed from the instant STORED on the
//! resource, so a handler that both materialises what the clock owes and then replaces the
//! work that was owed must judge it against the store it is holding — against an instant no
//! earlier than the one it takes the store at — and every transition due by then is
//! recorded rather than discarded by the replacement. A delivery whose stored instant has
//! passed by the time `UpdateExport` takes the store is therefore ALWAYS recorded as an
//! execution of the definition it was owed for, whether it fell due long before the request,
//! a moment before it, or while the request was being read; only a delivery still ahead of
//! that instant is superseded by the one the update owes.
//!
//! It is asserted over a TABLE whose rows differ along what the rule generalises over —
//! WHERE the stored instant sits relative to the request's span (long past, a moment past,
//! inside the request's own span, still ahead of it) — each row on a different export
//! definition from `export_lifecycle::cases`, because the rule is about the instant and not
//! about any one definition.
//!
//! Nothing here sleeps for a due instant and nothing is faked: the instant is state the
//! service itself reads, so a row MOVES it — and moves it with the store held against an
//! `UpdateExport` that is already in flight, which is how the "fell due while the request
//! was in flight" row is deterministic without racing the handler.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    DELIVERY_SETTLES_AFTER, EXECUTION_STATUS_INITIATION_IN_PROCESS,
};
use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState as State;

use crate::delivery_executions::hold_delivery;
use crate::execution_lookup::client_and_engine;
use crate::export_lifecycle::{cases, timestamp, Client};

/// How long a row gives the `UpdateExport` it spawned to reach the store it is held at.
///
/// Only a floor: the row holds the store for this long and re-dates the delivery while
/// holding it, so the instant it writes cannot be later than the instant the handler goes
/// on to take the store at — however long the handler takes to get there.
const IN_FLIGHT: Duration = Duration::from_millis(250);

/// How far ahead an instant is moved when a row wants it NOT to be due: long enough that no
/// call this suite makes can reach it.
const HOLD_FOR: Duration = Duration::from_secs(3_600);

/// Where the stored instant of the delivery the service already owed sits relative to the
/// span of the `UpdateExport` that replaces the definition that delivery would have run.
#[derive(Debug, Clone, Copy)]
enum FellDue {
    /// Before the update was ever called.
    Before(Duration),
    /// While the update was in flight: the row reads the clock with the store held against
    /// the handler, so this instant is later than the one the request started at and
    /// earlier than the one the handler takes the store at.
    InFlight,
    /// Not by the time the handler takes the store, and so not at all: the update's own
    /// delivery supersedes it.
    NotYet(Duration),
}

impl FellDue {
    /// The instant to store on the pending delivery, where `now` is read while the update
    /// is held at the store.
    fn initiate_at(self, now: SystemTime) -> SystemTime {
        match self {
            Self::Before(ago) => now - ago,
            Self::InFlight => now,
            Self::NotYet(ahead) => now + ahead,
        }
    }

    /// Whether the clock owed that delivery by the instant the update took the store.
    fn was_owed(self) -> bool {
        !matches!(self, Self::NotYet(_))
    }
}

/// Re-date the delivery `export_arn` is owed, through a store the caller ALREADY holds:
/// initiated at `initiate_at`, with its terminal status held well beyond this suite, so the
/// execution it records stays in the non-terminal status a started delivery carries.
///
/// Both instants are state the service reads, so writing them is how a row puts a
/// clock-owed transition where it wants it without sleeping for one.
fn reschedule(store: &mut State, case: &str, export_arn: &str, initiate_at: SystemTime) {
    let delivery = store
        .exports
        .get_mut(export_arn)
        .unwrap_or_else(|| panic!("case {case:?}: the export was created through CreateExport"))
        .pending_delivery
        .as_mut()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the service must owe this export a delivery")
        });
    delivery.initiate_at = initiate_at;
    delivery.complete_at = initiate_at + DELIVERY_SETTLES_AFTER + HOLD_FOR;
}

/// `at` as the whole epoch second this service reports every instant at.
fn epoch_second(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs() as i64
}

/// The `Executions` one `ListExecutions` reports for `export_arn`. An empty page may elide
/// the member, so an absent one reads as the empty list it stands for.
async fn listed(client: &Client, case: &str, export_arn: &str) -> Vec<Value> {
    let body = client
        .ok("ListExecutions", &json!({ "ExportArn": export_arn }))
        .await;
    match body.get("Executions") {
        Some(Value::Array(references)) => references.clone(),
        None => Vec::new(),
        other => panic!("case {case:?}: Executions must be a list, got {other:?}"),
    }
}

/// The definition each reported execution of `export_arn` runs, read back through
/// `GetExecution`.
async fn snapshots(client: &Client, case: &str, export_arn: &str) -> Vec<Value> {
    let mut snapshots = Vec::new();
    for reference in listed(client, case, export_arn).await {
        let id = reference["ExecutionId"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: a reference carries its ExecutionId, got {reference}")
        });
        let described = client
            .ok(
                "GetExecution",
                &json!({ "ExportArn": export_arn, "ExecutionId": id }),
            )
            .await;
        snapshots.push(described["Export"].clone());
    }
    snapshots
}

/// RULE: every delivery the clock owed by the instant `UpdateExport` took the store is
/// recorded as an execution of the definition it was owed for; the replacement supersedes
/// only a delivery that was not due by then, and the update reports no instant from before
/// the store it answered for.
#[tokio::test]
async fn a_delivery_due_by_the_time_the_update_takes_the_store_is_recorded() {
    let rows = [
        (
            "due long before the update",
            FellDue::Before(Duration::from_secs(2)),
        ),
        (
            "due a moment before the update took the store",
            FellDue::Before(Duration::from_millis(1)),
        ),
        ("fell due while the update was in flight", FellDue::InFlight),
        ("not due when the update took the store", FellDue::NotYet(HOLD_FOR)),
    ];

    for (index, (label, fell_due)) in rows.into_iter().enumerate() {
        let (client, engine) = client_and_engine();
        // A different definition per row, so the rule is not asserted on one shape of
        // export: the leanest definition the model allows, a fully populated one, and
        // queries on two further catalog tables.
        let mut case = cases().remove(index);
        case.set_name(&format!("owed_delivery_{index:02}"));
        let arn = client.ok("CreateExport", &case.create_request()).await["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: CreateExport returns an ExportArn"))
            .to_string();
        // The delivery the create owed is held while the row sets up, so nothing
        // materialises it before the row decides where its instant sits.
        hold_delivery(&engine, &arn);
        let first = client.ok("GetExport", &json!({ "ExportArn": &arn })).await["Export"].clone();

        // The replacement differs from the stored definition, so the snapshot each
        // execution reports says which definition its delivery was owed for.
        let mut replacement = case.update_export();
        replacement
            .as_object_mut()
            .unwrap_or_else(|| panic!("case {label:?}: the Export structure is a JSON object"))
            .insert(
                "Description".to_string(),
                json!(format!("the replacement definition of row {index:02}")),
            );
        let update = json!({ "ExportArn": &arn, "Export": replacement });

        // The store is held against the update: it is taken BEFORE the request is made, so
        // the handler cannot be past its first read of the store, and it is released only
        // after this row has written the instant it wants. The request runs on a thread of
        // its own because it blocks on this very lock.
        let state = engine.state::<State>();
        let (in_flight, initiate_at) = {
            let mut store = state.lock();
            let in_flight = {
                let engine = Arc::clone(&engine);
                std::thread::spawn(move || {
                    let runtime = ::tokio::runtime::Builder::new_current_thread()
                        .build()
                        .expect("a current-thread runtime");
                    let client = Client::with_engine(engine);
                    runtime.block_on(async move { client.call("UpdateExport", &update).await })
                })
            };
            std::thread::sleep(IN_FLIGHT);
            let initiate_at = fell_due.initiate_at(SystemTime::now());
            reschedule(&mut store, label, &arn, initiate_at);
            (in_flight, initiate_at)
        };

        let (status, body) = in_flight.join().expect("the update answered");
        assert_eq!(
            status, 200,
            "case {label:?}: expected the update to be accepted, got {status} with {body}"
        );
        // The delivery the UPDATE owes is held in turn, so what the row reads next is what
        // the update did and not a later delivery falling due under it.
        hold_delivery(&engine, &arn);

        let after = client.ok("GetExport", &json!({ "ExportArn": &arn })).await;
        let second = after["Export"].clone();
        assert_ne!(
            second, first,
            "case {label:?}: the update must have replaced the stored definition, got {second}"
        );

        let owed = listed(&client, label, &arn).await;
        if fell_due.was_owed() {
            assert_eq!(
                owed.len(),
                1,
                "case {label:?}: the delivery the clock owed by the instant the update took the store must be recorded, got {owed:?}"
            );
            let status = owed[0]["ExecutionStatus"].clone();
            assert_eq!(
                status["StatusCode"].as_str(),
                Some(EXECUTION_STATUS_INITIATION_IN_PROCESS),
                "case {label:?}: a delivery that has started and not finished is in its non-terminal status, got {status}"
            );
            assert_eq!(
                timestamp(label, &status, "CreatedAt").secs(),
                epoch_second(initiate_at),
                "case {label:?}: expected the execution dated at the instant the delivery fell due ({}), got {status}",
                epoch_second(initiate_at)
            );
            assert_eq!(
                snapshots(&client, label, &arn).await,
                vec![first.clone()],
                "case {label:?}: the delivery owed for the FIRST definition must report it as its snapshot"
            );
            // The instant the update reports is the one it answered the store at, so it is
            // never earlier than a transition it had to apply.
            let last_updated = timestamp(label, &after["ExportStatus"], "LastUpdatedAt").secs();
            assert!(
                last_updated >= epoch_second(initiate_at),
                "case {label:?}: expected LastUpdatedAt at or after the instant the delivery it materialised fell due ({}), got {last_updated}",
                epoch_second(initiate_at)
            );
        } else {
            assert!(
                owed.is_empty(),
                "case {label:?}: a delivery that was not due when the update took the store is superseded, not recorded, got {owed:?}"
            );
        }

        // The delivery the update itself owes is of the NEW definition: settling it adds
        // exactly one execution, and that one reports the replacement.
        {
            let mut store = state.lock();
            reschedule(
                &mut store,
                label,
                &arn,
                SystemTime::now() - Duration::from_secs(1),
            );
        }
        let reported = snapshots(&client, label, &arn).await;
        let mut expected = vec![second.clone()];
        if fell_due.was_owed() {
            expected.insert(0, first.clone());
        }
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {label:?}: expected {} delivery(ies), got {reported:?}",
            expected.len()
        );
        for definition in &expected {
            assert!(
                reported.contains(definition),
                "case {label:?}: a delivery of this definition must report it as its snapshot, got {reported:?}"
            );
        }
    }
}

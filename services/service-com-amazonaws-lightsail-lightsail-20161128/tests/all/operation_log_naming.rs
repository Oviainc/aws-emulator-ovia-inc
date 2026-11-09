// SPDX-License-Identifier: CC0-1.0

//! What every operation-log entry says about the call that wrote it, driven over
//! the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: every entry of this account's operation log reports an
//! `operationType` naming the call that wrote it — the modelled `OperationType`
//! enum's own member where it declares one for that call, and otherwise the name
//! of the call itself, because the member is a string on the wire and a Smithy
//! enum is open there. One rule for the whole log: no entry `GetOperations` or
//! `GetOperationsForResource` reports is silent about what ran, whether or not
//! the enum (which lags this API) declares a member for the call.
//!
//! The rows of the table differ along what the rule generalises over: WHICH call
//! wrote the entry, and in particular whether the modelled enum declares a member
//! for it — the table spans both groups, and which group a row is in is read off
//! the model's own value list rather than written out here — plus the KIND of
//! resource the entry is about (an instance, a disk, both kinds of snapshot, a
//! snapshot copy, a key pair, an export record, a stack record, the Region's
//! VPC), whether the entry is terminal when the call returns or left for the
//! clock to settle, and whether the call answers one `operation` or a list of
//! `operations`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The model's own list of `OperationType` members, generated into this crate
// from the Smithy model
// (`smithy_query(query="shape", shape="OperationType")`), so which calls the
// enum declares is read off the model rather than copied into this suite.
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::OPERATION_TYPE_VALUES;
// The store this service answers every read from, for the one fixture an
// implemented call cannot produce as this suite needs it: a key pair the account
// holds under a chosen id, instant and fingerprint (seeded exactly as
// `key_pair_delete.rs` seeds one).
use service_com_amazonaws_lightsail_lightsail_20161128::state as lightsail_state;

// The harness, the minted-id check, the settled-state waits and the log readers
// this test target already exposes: a sibling suite drives the service through
// the same front door and reads an answered entry the same way rather than
// keeping second copies of either.
use crate::auto_snapshots::{auto_snapshot_request, wait_for_success};
use crate::instance_lifecycle::{is_uuid_shaped, wait_for_state, Emulator};
use crate::operations_for_resource::{answered_entries, ids, operation_types};
// The two paged log walks `operations_log.rs` already drives, which check the
// paging invariants on the way: this suite reads the log through them rather
// than keeping a third copy of the walk.
use crate::operations_log::{walk, walk_one_resource};
use crate::snapshot_copy::{wait_for_snapshot_state, Kind};

/// The zone every resource in this suite lives in, and the Region that zone is
/// in — which is also the Region every call here is made in, so a request's
/// credential scope always matches the resource it names.
const ZONE: &str = "us-east-1a";
/// See [`ZONE`].
const REGION: &str = "us-east-1";

/// A Linux/Unix blueprint and the smallest plan: nothing about the operation log
/// turns on either.
const BLUEPRINT: &str = "amazon_linux_2023";
/// See [`BLUEPRINT`].
const BUNDLE: &str = "nano_3_0";

/// How long a case waits for a transition the clock owes.
const WAIT: Duration = Duration::from_secs(5);

/// The resources this suite's calls act on.
const INSTANCE: &str = "log-instance";
/// See [`INSTANCE`].
const DISK: &str = "log-disk";
/// See [`INSTANCE`].
const DISK_SIZE_IN_GB: i64 = 8;
/// See [`INSTANCE`].
const INSTANCE_SNAPSHOT: &str = "log-instance-snapshot";
/// See [`INSTANCE`].
const DISK_SNAPSHOT: &str = "log-disk-snapshot";
/// See [`INSTANCE`].
const INSTANCE_SNAPSHOT_COPY: &str = "log-instance-snapshot-copy";
/// See [`INSTANCE`].
const DISK_SNAPSHOT_COPY: &str = "log_disk_snapshot_copy_1";
/// See [`INSTANCE`].
const KEY_PAIR: &str = "log-key-pair";

/// The tag one row puts on the instance and the next row takes off again.
const TAG_KEY: &str = "stage";
/// See [`TAG_KEY`].
const TAG_VALUE: &str = "production";

/// The Amazon EC2 instance type and port configuration the stack row asks for,
/// from the modelled `InstanceEntry$instanceType` and `PortInfoSourceType`.
const EC2_INSTANCE_TYPE: &str = "t2.micro";
/// See [`EC2_INSTANCE_TYPE`].
const PORT_INFO_SOURCE: &str = "DEFAULT";

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One call this suite drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    /// Create the instance every later row acts on, with the automatic snapshot
    /// add-on enabled so a copy of it exists to delete.
    CreateInstances,
    /// Take the manual instance snapshot the copy row copies.
    CreateInstanceSnapshot,
    /// Create the disk and take the manual disk snapshot the copy, export and
    /// stack rows use.
    CreateDiskSnapshot,
    /// Copy that instance snapshot.
    CopyInstanceSnapshot,
    /// Copy that disk snapshot.
    CopyDiskSnapshot,
    /// Put a tag on the instance.
    TagResource,
    /// Take it off again.
    UntagResource,
    /// Delete the one automatic snapshot the add-on has taken of the instance.
    DeleteAutoSnapshot,
    /// Export the disk snapshot to Amazon EC2.
    ExportSnapshot,
    /// Create a stack from the record that export appended.
    CreateCloudFormationStack,
    /// Delete a key pair this row creates first.
    DeleteKeyPair,
    /// Peer the Region's Lightsail VPC.
    PeerVpc,
    /// Unpeer it again.
    UnpeerVpc,
}

/// One row: the call, and the `operationType` the rule says every entry that
/// call writes reports.
struct Row {
    case: &'static str,
    call: Call,
    operation_type: &'static str,
    /// The resource the entry is about, which `GetOperationsForResource` reports
    /// the entry under.
    resource_name: &'static str,
}

/// The table. Both groups the rule covers are present: calls the modelled
/// `OperationType` enum declares a member for, and calls it declares none for —
/// and the suite asserts, from the model's own value list, that each group has
/// rows.
const ROWS: &[Row] = &[
    Row {
        // `CreateInstances` creates one instance per name and the enum's member
        // is the SINGULAR `CreateInstance`: where the enum declares a value for
        // what the call did, that value is what the entry reports.
        case: "a call the modelled enum declares: creating an instance",
        call: Call::CreateInstances,
        operation_type: "CreateInstance",
        resource_name: INSTANCE,
    },
    Row {
        case: "a call the modelled enum declares: taking a manual instance snapshot",
        call: Call::CreateInstanceSnapshot,
        operation_type: "CreateInstanceSnapshot",
        resource_name: INSTANCE_SNAPSHOT,
    },
    Row {
        case: "a call the modelled enum declares: taking a manual disk snapshot",
        call: Call::CreateDiskSnapshot,
        operation_type: "CreateDiskSnapshot",
        resource_name: DISK_SNAPSHOT,
    },
    Row {
        case: "a call the enum declares no member for: copying an instance snapshot",
        call: Call::CopyInstanceSnapshot,
        operation_type: "CopySnapshot",
        resource_name: INSTANCE_SNAPSHOT_COPY,
    },
    Row {
        case: "a call the enum declares no member for: copying a disk snapshot",
        call: Call::CopyDiskSnapshot,
        operation_type: "CopySnapshot",
        resource_name: DISK_SNAPSHOT_COPY,
    },
    Row {
        case: "a call the enum declares no member for: tagging a resource",
        call: Call::TagResource,
        operation_type: "TagResource",
        resource_name: INSTANCE,
    },
    Row {
        case: "a call the enum declares no member for: untagging it again",
        call: Call::UntagResource,
        operation_type: "UntagResource",
        resource_name: INSTANCE,
    },
    Row {
        case: "a call the enum declares no member for: deleting one automatic snapshot",
        call: Call::DeleteAutoSnapshot,
        operation_type: "DeleteAutoSnapshot",
        resource_name: INSTANCE,
    },
    Row {
        case: "a call the enum declares no member for: exporting a snapshot",
        call: Call::ExportSnapshot,
        operation_type: "ExportSnapshot",
        resource_name: DISK_SNAPSHOT,
    },
    Row {
        case: "a call the enum declares no member for: creating a stack from that export",
        call: Call::CreateCloudFormationStack,
        // The entry is written against the stack record the call appended, whose
        // name the store mints, so this row's resource is read off the entry.
        operation_type: "CreateCloudFormationStack",
        resource_name: "",
    },
    Row {
        case: "a call the enum declares no member for: deleting a key pair",
        call: Call::DeleteKeyPair,
        operation_type: "DeleteKeyPair",
        resource_name: KEY_PAIR,
    },
    Row {
        case: "a call the enum declares no member for: peering the Region's VPC",
        call: Call::PeerVpc,
        operation_type: "PeerVpc",
        resource_name: REGION,
    },
    Row {
        case: "a call the enum declares no member for: unpeering it again",
        call: Call::UnpeerVpc,
        operation_type: "UnpeerVpc",
        resource_name: REGION,
    },
];

// ---------------------------------------------------------------------------
// Driving one row
// ---------------------------------------------------------------------------

/// Poll `GetDisk` until the disk `name` reports `want`.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_disk_state(emulator: &Emulator, name: &str, want: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let reported = emulator.ok("GetDisk", json!({"diskName": name})).await["disk"]["state"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if reported == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within {WAIT:?}, got {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Every export snapshot record `GetExportSnapshotRecords` reports, walking every
/// page.
async fn export_records(emulator: &Emulator) -> Vec<Value> {
    let mut records = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => {
                emulator
                    .ok("GetExportSnapshotRecords", json!({"pageToken": token}))
                    .await
            }
            None => emulator.ok("GetExportSnapshotRecords", json!({})).await,
        };
        records.extend(
            body["exportSnapshotRecords"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break records,
        }
    }
}

/// Poll the export snapshot records until the one taken of `snapshot_name` has
/// succeeded — the state a stack can be created from — answering its name.
async fn succeeded_export_name(emulator: &Emulator, case: &str, snapshot_name: &str) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let reported = export_records(emulator).await;
        let found = reported.iter().find(|record| {
            record["sourceInfo"]["name"].as_str() == Some(snapshot_name)
                && record["state"].as_str() == Some("Succeeded")
        });
        if let Some(record) = found {
            return record["name"].as_str().unwrap_or_default().to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected the export of {snapshot_name} to succeed within {WAIT:?}, \
             got {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Put the key pair this suite deletes in the store under the id, instant and
/// fingerprint the cases below name — none of which a create lets a caller pick
/// (`key_pair_delete.rs` seeds its own the same way).
fn hold_key_pair(emulator: &Emulator) {
    let id = "7f3c1d58-9b24-4e6a-8c05-2f1a6b7d4e90";
    let held = lightsail_state::with_state(emulator.engine(), |store| {
        store.insert_key_pair(lightsail_state::StoredKeyPair {
            name: KEY_PAIR.to_string(),
            arn: lightsail_state::key_pair_arn(REGION, id),
            support_code: lightsail_state::support_code(id),
            created_at_ms: 1_770_000_000_000,
            location: lightsail_state::StoredLocation::regional(REGION),
            tags: Vec::new(),
            fingerprint: "a6:1f:3c:d2:0b:8e:74:59:c1:3d:ef:90:12:ab:cd:34".to_string(),
        });
        store.key_pairs.len()
    });
    assert_eq!(
        held,
        Some(1),
        "case seeding the key pair: expected the store to hold it, got {held:?}"
    );
}

/// Poll `IsVpcPeered` until it reports `want`.
async fn wait_until_peered(emulator: &Emulator, case: &str, want: bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        let body = emulator.ok("IsVpcPeered", json!({})).await;
        let peered = body["isPeered"].as_bool();
        if peered == Some(want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected isPeered {want} within {WAIT:?}, got {body:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Make one row's call, together with whatever it needs to be makeable, and
/// answer the operation-log entries that call reported.
async fn drive(emulator: &Emulator, row: &Row) -> Vec<Value> {
    let case = row.case;
    let answered = match row.call {
        Call::CreateInstances => {
            emulator
                .ok(
                    "CreateInstances",
                    json!({
                        "instanceNames": [INSTANCE],
                        "availabilityZone": ZONE,
                        "blueprintId": BLUEPRINT,
                        "bundleId": BUNDLE,
                        "addOns": [auto_snapshot_request(None)],
                    }),
                )
                .await
        }
        Call::CreateInstanceSnapshot => {
            // A snapshot is taken of a settled instance.
            wait_for_state(emulator, INSTANCE, "running").await;
            emulator
                .ok(
                    "CreateInstanceSnapshot",
                    json!({
                        "instanceSnapshotName": INSTANCE_SNAPSHOT,
                        "instanceName": INSTANCE,
                    }),
                )
                .await
        }
        Call::CreateDiskSnapshot => {
            emulator
                .ok(
                    "CreateDisk",
                    json!({
                        "diskName": DISK,
                        "availabilityZone": ZONE,
                        "sizeInGb": DISK_SIZE_IN_GB,
                    }),
                )
                .await;
            wait_for_disk_state(emulator, DISK, "available").await;
            emulator
                .ok(
                    "CreateDiskSnapshot",
                    json!({"diskSnapshotName": DISK_SNAPSHOT, "diskName": DISK}),
                )
                .await
        }
        Call::CopyInstanceSnapshot => {
            // Only a snapshot the service has finished taking can be copied.
            wait_for_snapshot_state(
                emulator,
                Kind::InstanceSnapshot,
                INSTANCE_SNAPSHOT,
                Kind::InstanceSnapshot.settled(),
            )
            .await;
            emulator
                .ok(
                    "CopySnapshot",
                    json!({
                        "sourceSnapshotName": INSTANCE_SNAPSHOT,
                        "targetSnapshotName": INSTANCE_SNAPSHOT_COPY,
                        "sourceRegion": REGION,
                    }),
                )
                .await
        }
        Call::CopyDiskSnapshot => {
            wait_for_snapshot_state(
                emulator,
                Kind::DiskSnapshot,
                DISK_SNAPSHOT,
                Kind::DiskSnapshot.settled(),
            )
            .await;
            emulator
                .ok(
                    "CopySnapshot",
                    json!({
                        "sourceSnapshotName": DISK_SNAPSHOT,
                        "targetSnapshotName": DISK_SNAPSHOT_COPY,
                        "sourceRegion": REGION,
                    }),
                )
                .await
        }
        Call::TagResource => {
            emulator
                .ok(
                    "TagResource",
                    json!({
                        "resourceName": INSTANCE,
                        "tags": [{"key": TAG_KEY, "value": TAG_VALUE}],
                    }),
                )
                .await
        }
        Call::UntagResource => {
            emulator
                .ok(
                    "UntagResource",
                    json!({"resourceName": INSTANCE, "tagKeys": [TAG_KEY]}),
                )
                .await
        }
        Call::DeleteAutoSnapshot => {
            // The add-on takes the copy; only the clock makes it due.
            let copy = wait_for_success(emulator, case, INSTANCE).await;
            let date = copy["date"].as_str().unwrap_or_else(|| {
                panic!("case {case}: expected the copy to be dated, got {copy:?}")
            });
            emulator
                .ok(
                    "DeleteAutoSnapshot",
                    json!({"resourceName": INSTANCE, "date": date}),
                )
                .await
        }
        Call::ExportSnapshot => {
            emulator
                .ok(
                    "ExportSnapshot",
                    json!({"sourceSnapshotName": DISK_SNAPSHOT}),
                )
                .await
        }
        Call::CreateCloudFormationStack => {
            // The stack creates its Amazon EC2 instance from an exported
            // INSTANCE snapshot, so this row exports one of its own (the
            // `ExportSnapshot` row above exports the disk snapshot).
            emulator
                .ok(
                    "ExportSnapshot",
                    json!({"sourceSnapshotName": INSTANCE_SNAPSHOT}),
                )
                .await;
            let source_name = succeeded_export_name(emulator, case, INSTANCE_SNAPSHOT).await;
            emulator
                .ok(
                    "CreateCloudFormationStack",
                    json!({
                        "instances": [{
                            "sourceName": source_name,
                            "instanceType": EC2_INSTANCE_TYPE,
                            "portInfoSource": PORT_INFO_SOURCE,
                            "availabilityZone": ZONE,
                        }],
                    }),
                )
                .await
        }
        Call::DeleteKeyPair => {
            hold_key_pair(emulator);
            emulator
                .ok("DeleteKeyPair", json!({"keyPairName": KEY_PAIR}))
                .await
        }
        Call::PeerVpc => emulator.ok("PeerVpc", json!({})).await,
        Call::UnpeerVpc => {
            wait_until_peered(emulator, case, true).await;
            emulator.ok("UnpeerVpc", json!({})).await
        }
    };
    let entries = answered_entries(case, &answered);
    assert!(
        !entries.is_empty(),
        "case {case}: expected the call to answer at least one operation-log entry, got \
         {answered:?}"
    );
    entries
}

// ---------------------------------------------------------------------------
// Reading the log back
// ---------------------------------------------------------------------------

/// Whether the modelled `OperationType` enum declares `value`, read off the
/// model's own member list.
fn declared_by_the_model(value: &str) -> bool {
    OPERATION_TYPE_VALUES.contains(&value)
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: every operation-log entry reports an `operationType` naming the call
/// that wrote it — the modelled enum's member where it declares one, the call's
/// own name where it declares none — in the answer the call gave, in
/// `GetOperationsForResource` and in `GetOperations`.
#[tokio::test]
async fn every_entry_names_the_call_that_wrote_it() {
    let emulator = Emulator::new();
    // What each entry this suite's calls answered must report, by entry id, so
    // the account-wide read is checked against the answers rather than against a
    // second expectation.
    let mut expected_by_id: BTreeMap<String, String> = BTreeMap::new();
    let mut covered_declared: BTreeSet<&str> = BTreeSet::new();
    let mut covered_undeclared: BTreeSet<&str> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let entries = drive(&emulator, row).await;

        // The answer itself: every entry names the call, and nothing is left
        // silent about what ran.
        for (entry, reported) in entries.iter().zip(operation_types(&entries)) {
            assert_eq!(
                reported, row.operation_type,
                "case {case}: expected the entry to report operationType {}, got {entry:?}",
                row.operation_type
            );
        }
        for (id, reported) in ids(&entries).into_iter().zip(operation_types(&entries)) {
            assert!(
                is_uuid_shaped(&id),
                "case {case}: expected a uuid-shaped operation id, got {id:?}"
            );
            expected_by_id.insert(id, reported);
        }
        if declared_by_the_model(row.operation_type) {
            covered_declared.insert(row.operation_type);
        } else {
            covered_undeclared.insert(row.operation_type);
        }

        // The same entry, read back through the per-resource reader: an entry
        // reports what it reports however it is read.
        let resource_name = if row.resource_name.is_empty() {
            entries[0]["resourceName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        } else {
            row.resource_name.to_string()
        };
        let read = walk_one_resource(&emulator, case, &resource_name).await;
        let read_types = operation_types(&read);
        assert!(
            read_types.iter().any(|kind| kind == row.operation_type),
            "case {case}: expected {} among the types reported for {resource_name}, got \
             {read_types:?}",
            row.operation_type
        );
    }

    // The table spans both groups the rule covers, which is read off the model's
    // own member list rather than asserted by hand.
    assert!(
        covered_declared.len() >= 2,
        "case the table's coverage: expected rows for calls the modelled enum declares, got \
         {covered_declared:?}"
    );
    assert!(
        covered_undeclared.len() >= 2,
        "case the table's coverage: expected rows for calls the modelled enum declares no \
         member for, got {covered_undeclared:?}"
    );

    // The whole log, as `GetOperations` reports it: EVERY entry it holds names a
    // call — including the ones written by calls no row made — and each entry a
    // row's answer named reports there exactly what the answer said.
    let (logged, _page_sizes) = walk(&emulator, "reading the whole log").await;
    let logged_types = operation_types(&logged);
    for (entry, reported) in logged.iter().zip(&logged_types) {
        assert!(
            !reported.is_empty(),
            "case reading the whole log: expected every entry to name its call, got {entry:?}"
        );
    }
    for (id, reported) in ids(&logged).into_iter().zip(logged_types) {
        if let Some(answered) = expected_by_id.get(&id) {
            assert_eq!(
                &reported, answered,
                "case reading the whole log: expected entry {id} to keep the operationType its \
                 call answered, got {reported:?}"
            );
        }
    }
    for row in ROWS {
        assert!(
            expected_by_id
                .values()
                .any(|kind| kind == row.operation_type),
            "case {}: expected its entry's type to have been recorded, got {expected_by_id:?}",
            row.case
        );
    }
}

/// RULE, on the one comparison the log makes easiest to see: two calls in the
/// IDENTICAL position — the modelled enum declares a member for neither — acting
/// on the same snapshot are reported the same way, each naming itself.
#[tokio::test]
async fn an_export_and_a_copy_of_one_snapshot_are_reported_the_same_way() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": DISK,
                "availabilityZone": ZONE,
                "sizeInGb": DISK_SIZE_IN_GB,
            }),
        )
        .await;
    wait_for_disk_state(&emulator, DISK, "available").await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskSnapshotName": DISK_SNAPSHOT, "diskName": DISK}),
        )
        .await;
    wait_for_snapshot_state(
        &emulator,
        Kind::DiskSnapshot,
        DISK_SNAPSHOT,
        Kind::DiskSnapshot.settled(),
    )
    .await;

    emulator
        .ok(
            "ExportSnapshot",
            json!({"sourceSnapshotName": DISK_SNAPSHOT}),
        )
        .await;
    emulator
        .ok(
            "CopySnapshot",
            json!({
                "sourceSnapshotName": DISK_SNAPSHOT,
                "targetSnapshotName": DISK_SNAPSHOT_COPY,
                "sourceRegion": REGION,
            }),
        )
        .await;

    // Neither name is a member of the modelled enum, and both are reported:
    // whichever of the two calls a reader asks about, the log says which ran.
    for name in ["ExportSnapshot", "CopySnapshot"] {
        assert!(
            !declared_by_the_model(name),
            "case {name}: this suite's comparison is about a call the modelled enum declares no \
             member for, got a declared one"
        );
    }
    let exported = operation_types(
        &walk_one_resource(&emulator, "the exported snapshot's log", DISK_SNAPSHOT).await,
    );
    assert!(
        exported.iter().any(|kind| kind == "ExportSnapshot"),
        "case the exported snapshot's log: expected the export to name itself, got {exported:?}"
    );
    let copied = operation_types(
        &walk_one_resource(&emulator, "the copy's log", DISK_SNAPSHOT_COPY).await,
    );
    assert_eq!(
        copied,
        vec!["CopySnapshot".to_string()],
        "case the copy's log: expected the copy to name itself, got {copied:?}"
    );
}

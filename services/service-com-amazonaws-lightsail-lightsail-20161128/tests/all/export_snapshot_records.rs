// SPDX-License-Identifier: CC0-1.0

//! Lightsail's export of a snapshot to Amazon EC2 — `ExportSnapshot` — and the
//! records it leaves behind — `GetExportSnapshotRecords` — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: each `ExportSnapshot` call appends exactly ONE export
//! snapshot record for the snapshot the request names — describing that snapshot,
//! the resource the snapshot was taken of and the Amazon EC2 resource the export
//! creates, every member taken from the stored snapshot record plus this store's
//! own allocators — and `GetExportSnapshotRecords` reports exactly the records
//! the store holds, a page at a time, with a `nextPageToken` only while records
//! remain. A record is created `Started` and the clock, not a reader, settles it
//! into `Succeeded`, which is when it names the resource created at the
//! destination; a request naming a snapshot the account does not hold is refused
//! with the service's `DoesNotExist` envelope and records nothing.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front door
// rather than standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN
/// carries.
const ACCOUNT: &str = "000000000000";

/// The prefix the service puts before the id in an export snapshot record's name
/// (`crate::state::EXPORT_SNAPSHOT_RECORD_NAME_PREFIX`).
const RECORD_NAME_PREFIX: &str = "ExportSnapshot-";

/// How many records one `GetExportSnapshotRecords` page carries
/// (`crate::state::EXPORT_SNAPSHOT_RECORD_PAGE_SIZE`).
const PAGE_SIZE: usize = 10;

/// Every export snapshot record `GetExportSnapshotRecords` reports, walking every
/// page.
///
/// Answers the records in page order together with the size of each page, so a
/// caller can assert both the contents and the paging.
async fn all_records(emulator: &Emulator) -> (Vec<Value>, Vec<usize>) {
    let mut records = Vec::new();
    let mut page_sizes = Vec::new();
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
        let page = body["exportSnapshotRecords"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case listing export snapshot records: expected an exportSnapshotRecords array, got {body:?}")
            })
            .clone();
        page_sizes.push(page.len());
        records.extend(page);
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            page_sizes.len() < 100,
            "case listing export snapshot records: the walk did not terminate after {} pages",
            page_sizes.len()
        );
    }
    (records, page_sizes)
}

/// The record names `GetExportSnapshotRecords` reports, as a set.
async fn record_names(emulator: &Emulator) -> BTreeSet<String> {
    all_records(emulator)
        .await
        .0
        .into_iter()
        .map(|record| {
            record["name"]
                .as_str()
                .expect("every reported record carries a name")
                .to_string()
        })
        .collect()
}

/// The reported record `name` names.
async fn record(emulator: &Emulator, name: &str) -> Value {
    all_records(emulator)
        .await
        .0
        .into_iter()
        .find(|record| record["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("case reading {name}: expected the record to be reported"))
}

/// Poll `GetExportSnapshotRecords` until the record `name` names reports `want`,
/// answering it.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let record = record(emulator, name).await;
        if record["state"].as_str() == Some(want) {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            record["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Whether `value` is a long-form Amazon EC2 resource id under `prefix`:
/// `<prefix>` plus 17 lowercase hexadecimal characters
/// (`crate::state::ec2_destination_id`).
fn is_ec2_resource_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|hex| {
        hex.len() == 17
            && hex
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    })
}

// ---------------------------------------------------------------------------
// The export table
// ---------------------------------------------------------------------------

/// What one row's snapshot was taken of: the two sources `CreateDiskSnapshot`
/// admits, because `ExportSnapshot`'s record reports "the snapshot's source
/// instance or disk".
#[derive(Debug)]
enum Source {
    /// A block storage disk of this size in GB, created through `CreateDisk`.
    Disk { size_in_gb: i64 },
    /// An instance's system volume: the instance is created from this blueprint
    /// and bundle, and the exported snapshot's size is the size of the system
    /// disk `GetInstance` reports for it.
    Instance {
        blueprint_id: &'static str,
        bundle_id: &'static str,
    },
}

/// One snapshot to export, and how many times to export it.
///
/// The rows differ along everything the rule generalises over: the KIND of
/// resource the snapshot was taken of (a block storage disk, an instance's system
/// volume), the snapshotted volume's size — including the smallest disk Lightsail
/// creates and a large one — the Availability Zone (and so the region in the ARN
/// and the location), the form of the snapshot name, and the NUMBER of exports of
/// the one snapshot, because each export is a record of its own.
struct Row {
    case: &'static str,
    source_name: &'static str,
    snapshot_name: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    source: Source,
    exports: usize,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "a disk snapshot exported once",
        source_name: "Disk-Virginia-1",
        snapshot_name: "my-disk-snapshot",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        source: Source::Disk { size_in_gb: 32 },
        exports: 1,
    },
    Row {
        case: "the smallest disk Lightsail creates, in another region, exported twice",
        source_name: "small-disk-8",
        snapshot_name: "small_disk_snapshot_1",
        availability_zone: "eu-west-1c",
        region: "eu-west-1",
        source: Source::Disk { size_in_gb: 8 },
        exports: 2,
    },
    Row {
        case: "a large disk's snapshot",
        source_name: "data-disk-4096",
        snapshot_name: "data-disk-4096-backup",
        availability_zone: "ap-southeast-2b",
        region: "ap-southeast-2",
        source: Source::Disk { size_in_gb: 4096 },
        exports: 1,
    },
    Row {
        case: "an instance's system volume, the smallest bundle",
        source_name: "nano-instance",
        snapshot_name: "nano-system-volume-snapshot",
        availability_zone: "us-west-2a",
        region: "us-west-2",
        source: Source::Instance {
            blueprint_id: "amazon_linux_2023",
            bundle_id: "nano_3_0",
        },
        exports: 1,
    },
    Row {
        case: "an instance's system volume, a larger bundle, exported three times",
        source_name: "medium_instance_2",
        snapshot_name: "medium-system-volume-snapshot",
        availability_zone: "ca-central-1a",
        region: "ca-central-1",
        source: Source::Instance {
            blueprint_id: "debian_12",
            bundle_id: "medium_3_0",
        },
        exports: 3,
    },
];

/// What one row's snapshot is a copy of, as the store itself reports it: the
/// source resource's ARN and the size in GB of the volume that was snapshotted.
async fn create_source(emulator: &Emulator, row: &Row) -> (String, i64) {
    match row.source {
        Source::Disk { size_in_gb } => {
            emulator
                .ok(
                    "CreateDisk",
                    json!({
                        "diskName": row.source_name,
                        "availabilityZone": row.availability_zone,
                        "sizeInGb": size_in_gb,
                    }),
                )
                .await;
            let disk = emulator
                .ok("GetDisk", json!({"diskName": row.source_name}))
                .await["disk"]
                .clone();
            (
                disk["arn"].as_str().unwrap_or_default().to_string(),
                size_in_gb,
            )
        }
        Source::Instance {
            blueprint_id,
            bundle_id,
        } => {
            emulator
                .ok(
                    "CreateInstances",
                    json!({
                        "instanceNames": [row.source_name],
                        "availabilityZone": row.availability_zone,
                        "blueprintId": blueprint_id,
                        "bundleId": bundle_id,
                    }),
                )
                .await;
            let instance = emulator
                .ok("GetInstance", json!({"instanceName": row.source_name}))
                .await["instance"]
                .clone();
            let system_disk = instance["hardware"]["disks"][0].clone();
            assert_eq!(
                system_disk["isSystemDisk"].as_bool(),
                Some(true),
                "case {}: expected the instance's first disk to be its system disk, got \
                 {system_disk:?}",
                row.case
            );
            (
                instance["arn"].as_str().unwrap_or_default().to_string(),
                system_disk["sizeInGb"].as_i64().unwrap_or_default(),
            )
        }
    }
}

/// Create `row`'s source resource and the disk snapshot of it this suite
/// exports, answering the snapshot as `GetDiskSnapshot` reports it, the source
/// resource's ARN and the snapshotted volume's size.
async fn create_snapshot(emulator: &Emulator, row: &Row) -> (Value, String, i64) {
    let (source_arn, size_in_gb) = create_source(emulator, row).await;
    let mut body = json!({"diskSnapshotName": row.snapshot_name});
    match row.source {
        Source::Disk { .. } => body["diskName"] = json!(row.source_name),
        Source::Instance { .. } => body["instanceName"] = json!(row.source_name),
    }
    emulator.ok("CreateDiskSnapshot", body).await;
    let snapshot = emulator
        .ok(
            "GetDiskSnapshot",
            json!({"diskSnapshotName": row.snapshot_name}),
        )
        .await["diskSnapshot"]
        .clone();
    (snapshot, source_arn, size_in_gb)
}

/// RULE: every export appends one record, and every member
/// `GetExportSnapshotRecords` reports on it comes from the stored snapshot plus
/// the store's own allocators.
#[tokio::test]
async fn every_export_appends_one_record_describing_its_snapshot() {
    let emulator = Emulator::new();
    let mut record_arns: BTreeSet<String> = BTreeSet::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();
    let mut expected_records = 0usize;

    for row in ROWS {
        let case = row.case;
        let (snapshot, source_arn, size_in_gb) = create_snapshot(&emulator, row).await;
        assert!(
            size_in_gb > 0,
            "case {case}: expected the snapshotted volume to have a size, got {size_in_gb}"
        );

        for export in 1..=row.exports {
            let before = record_names(&emulator).await;

            // The export answers ONE operation-log entry — it exports one
            // snapshot — against the SNAPSHOT, and it is not terminal yet,
            // because the export is still running when the call returns.
            let exported = emulator
                .ok(
                    "ExportSnapshot",
                    json!({"sourceSnapshotName": row.snapshot_name}),
                )
                .await;
            let operations = exported["operations"].as_array().unwrap_or_else(|| {
                panic!("case {case} export {export}: expected an operations array, got {exported:?}")
            }).clone();
            assert_eq!(
                operations.len(),
                1,
                "case {case} export {export}: expected exactly one operation, got {operations:?}"
            );
            let operation = operations[0].clone();
            assert_eq!(
                operation["operationType"].as_str(),
                Some("ExportSnapshot"),
                "case {case} export {export}: expected the ExportSnapshot operation type, got \
                 {operation:?}"
            );
            assert_eq!(
                operation["resourceType"].as_str(),
                Some("DiskSnapshot"),
                "case {case} export {export}: expected the exported snapshot's resource type, \
                 got {operation:?}"
            );
            assert_eq!(
                operation["resourceName"].as_str(),
                Some(row.snapshot_name),
                "case {case} export {export}: expected the operation to name the snapshot, got \
                 {operation:?}"
            );
            assert_eq!(
                operation["status"].as_str(),
                Some("Started"),
                "case {case} export {export}: expected a Started operation, got {operation:?}"
            );
            assert_eq!(
                operation["isTerminal"].as_bool(),
                Some(false),
                "case {case} export {export}: expected a non-terminal operation, got \
                 {operation:?}"
            );
            assert_eq!(
                operation["location"]["availabilityZone"].as_str(),
                Some(row.availability_zone),
                "case {case} export {export}: expected the operation to be located where the \
                 snapshot is, got {operation:?}"
            );
            let operation_id = operation["id"].as_str().unwrap_or_default().to_string();
            assert!(
                is_uuid_shaped(&operation_id),
                "case {case} export {export}: expected a uuid-shaped operation id, got \
                 {operation_id:?}"
            );
            assert!(
                operation_ids.insert(operation_id),
                "case {case} export {export}: expected a fresh operation id, got an already-used \
                 one"
            );

            // Exactly one record appeared, and it is the record of THIS export.
            let after = record_names(&emulator).await;
            expected_records += 1;
            assert_eq!(
                after.len(),
                expected_records,
                "case {case} export {export}: expected one more record than before, got {after:?}"
            );
            let new_names: Vec<String> = after.difference(&before).cloned().collect();
            assert_eq!(
                new_names.len(),
                1,
                "case {case} export {export}: expected exactly one new record, got {new_names:?}"
            );
            let name = new_names[0].clone();
            let record = record(&emulator, &name).await;

            // The record's own identity: a minted name whose id is the id its
            // ARN carries, in the snapshot's own region and under the emulator
            // account.
            let id = name
                .strip_prefix(RECORD_NAME_PREFIX)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case} export {export}: expected a '{RECORD_NAME_PREFIX}' record \
                         name, got {name:?}"
                    )
                })
                .to_string();
            assert!(
                is_uuid_shaped(&id),
                "case {case} export {export}: expected a uuid-shaped record id, got {id:?}"
            );
            let want_arn = format!(
                "arn:aws:lightsail:{}:{ACCOUNT}:ExportSnapshotRecord/{id}",
                row.region
            );
            assert_eq!(
                record["arn"].as_str(),
                Some(want_arn.as_str()),
                "case {case} export {export}: expected the record's own ARN, got {record:?}"
            );
            assert!(
                record_arns.insert(want_arn),
                "case {case} export {export}: expected a fresh record ARN, got an already-used one"
            );
            assert_eq!(
                record["resourceType"].as_str(),
                Some("ExportSnapshotRecord"),
                "case {case} export {export}: expected the ExportSnapshotRecord resource type, \
                 got {record:?}"
            );
            assert!(
                record["createdAt"].is_number(),
                "case {case} export {export}: expected an epoch-seconds createdAt, got {:?}",
                record["createdAt"]
            );
            assert_eq!(
                record["location"]["availabilityZone"].as_str(),
                Some(row.availability_zone),
                "case {case} export {export}: expected the snapshot's own zone, got {:?}",
                record["location"]
            );
            assert_eq!(
                record["location"]["regionName"].as_str(),
                Some(row.region),
                "case {case} export {export}: expected the snapshot's own region, got {:?}",
                record["location"]
            );

            // The export has only just started, so nothing exists at the
            // destination yet.
            assert_eq!(
                record["state"].as_str(),
                Some("Started"),
                "case {case} export {export}: expected a Started record, got {record:?}"
            );
            assert!(
                record["destinationInfo"].is_null(),
                "case {case} export {export}: a Started export has created nothing at the \
                 destination, got {:?}",
                record["destinationInfo"]
            );

            // What the record says about the snapshot it exported: the stored
            // snapshot's own members, and the resource THAT snapshot was taken
            // of.
            let source_info = record["sourceInfo"].clone();
            assert_eq!(
                source_info["resourceType"].as_str(),
                Some("DiskSnapshot"),
                "case {case} export {export}: expected a DiskSnapshot source, got {source_info:?}"
            );
            assert_eq!(
                source_info["name"].as_str(),
                Some(row.snapshot_name),
                "case {case} export {export}: expected the exported snapshot's name, got \
                 {source_info:?}"
            );
            assert_eq!(
                source_info["arn"], snapshot["arn"],
                "case {case} export {export}: expected the exported snapshot's own ARN, got \
                 {source_info:?}"
            );
            assert_eq!(
                source_info["createdAt"], snapshot["createdAt"],
                "case {case} export {export}: expected the exported snapshot's own creation \
                 time, got {source_info:?}"
            );
            assert_eq!(
                source_info["fromResourceName"].as_str(),
                Some(row.source_name),
                "case {case} export {export}: expected the snapshot's source resource, got \
                 {source_info:?}"
            );
            assert_eq!(
                source_info["fromResourceArn"].as_str(),
                Some(source_arn.as_str()),
                "case {case} export {export}: expected the snapshot's source resource ARN, got \
                 {source_info:?}"
            );
            assert_eq!(
                source_info["diskSnapshotInfo"]["sizeInGb"].as_i64(),
                Some(size_in_gb),
                "case {case} export {export}: expected the snapshotted volume's size, got \
                 {source_info:?}"
            );
            assert!(
                source_info["instanceSnapshotInfo"].is_null(),
                "case {case} export {export}: the export of a disk snapshot describes no \
                 instance snapshot, got {source_info:?}"
            );
        }
    }

    // Every export of the table is a record of its own, and each settles into
    // `Succeeded` naming a destination resource no other export created.
    let (records, page_sizes) = all_records(&emulator).await;
    let want_records: usize = ROWS.iter().map(|row| row.exports).sum();
    assert_eq!(
        records.len(),
        want_records,
        "case the whole table: expected one record per export, got {page_sizes:?} pages of \
         {} records",
        records.len()
    );
    let mut destination_ids: BTreeSet<String> = BTreeSet::new();
    for record in &records {
        let name = record["name"].as_str().unwrap_or_default().to_string();
        let settled = wait_for_state(&emulator, &name, "Succeeded").await;
        let destination = settled["destinationInfo"].clone();
        assert_eq!(
            destination["service"].as_str(),
            Some("ec2"),
            "case {name}: expected Amazon EC2 as the destination service, got {destination:?}"
        );
        let destination_id = destination["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_ec2_resource_id(&destination_id, "vol-"),
            "case {name}: expected the EBS volume an exported disk snapshot becomes, got \
             {destination_id:?}"
        );
        assert!(
            destination_ids.insert(destination_id),
            "case {name}: expected a destination resource no other export created, got an \
             already-used id"
        );
        // Settling changes the state and nothing else the record reported.
        assert_eq!(
            settled["sourceInfo"], record["sourceInfo"],
            "case {name}: expected the exported snapshot's description to be unchanged, got {:?}",
            settled["sourceInfo"]
        );
        assert_eq!(
            settled["arn"], record["arn"],
            "case {name}: expected the record's ARN to be unchanged, got {:?}",
            settled["arn"]
        );
    }
}

/// RULE: the clock, not a reader, settles an export — and reading the records
/// leaves them exactly as they were.
#[tokio::test]
async fn an_export_settles_on_the_clock_and_reads_leave_it_alone() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_snapshot(&emulator, row).await;

    let exported = emulator
        .ok(
            "ExportSnapshot",
            json!({"sourceSnapshotName": row.snapshot_name}),
        )
        .await;
    let operation_id = exported["operations"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Reading the records repeatedly does not move the export along: it is
    // `Started` on every read until the instant the clock owed it.
    let first = all_records(&emulator).await.0;
    let second = all_records(&emulator).await.0;
    assert_eq!(
        first, second,
        "case repeated reads: expected a read to change nothing a later reader sees, got \
         {second:?}"
    );
    assert_eq!(
        first.len(),
        1,
        "case repeated reads: expected the one record this export appended, got {first:?}"
    );
    assert_eq!(
        first[0]["state"].as_str(),
        Some("Started"),
        "case repeated reads: expected the export to still be Started, got {:?}",
        first[0]
    );
    let operation = emulator
        .ok("GetOperation", json!({"operationId": operation_id}))
        .await["operation"]
        .clone();
    assert_eq!(
        operation["status"].as_str(),
        Some("Started"),
        "case the operation log: expected the export's entry to still be Started, got \
         {operation:?}"
    );

    // Then the clock settles both the record and the operation it answered.
    let name = first[0]["name"].as_str().unwrap_or_default().to_string();
    let settled = wait_for_state(&emulator, &name, "Succeeded").await;
    assert!(
        is_ec2_resource_id(
            settled["destinationInfo"]["id"].as_str().unwrap_or_default(),
            "vol-"
        ),
        "case the settled export: expected a destination resource id, got {:?}",
        settled["destinationInfo"]
    );
    let operation = emulator
        .ok("GetOperation", json!({"operationId": operation_id}))
        .await["operation"]
        .clone();
    assert_eq!(
        operation["status"].as_str(),
        Some("Succeeded"),
        "case the operation log: expected the settled export's entry to have Succeeded, got \
         {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case the operation log: expected a terminal entry, got {operation:?}"
    );
}

/// RULE: `GetExportSnapshotRecords` pages the records it holds — at most
/// `PAGE_SIZE` per page, each page resumable from the token the one before it
/// answered, and a `nextPageToken` only while records remain.
#[tokio::test]
async fn the_records_are_paged_and_every_page_is_resumable() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_snapshot(&emulator, row).await;

    // More than two full pages, so the walk has a first, a middle and a last
    // page.
    let exports = PAGE_SIZE * 2 + 3;
    for export in 1..=exports {
        let exported = emulator
            .ok(
                "ExportSnapshot",
                json!({"sourceSnapshotName": row.snapshot_name}),
            )
            .await;
        assert_eq!(
            exported["operations"].as_array().map(Vec::len),
            Some(1),
            "case export {export}: expected one operation per export, got {exported:?}"
        );
    }

    let (records, page_sizes) = all_records(&emulator).await;
    assert_eq!(
        page_sizes,
        vec![PAGE_SIZE, PAGE_SIZE, exports - 2 * PAGE_SIZE],
        "case the page walk: expected full pages until the records run out, got {page_sizes:?}"
    );
    let names: Vec<String> = records
        .iter()
        .map(|record| record["name"].as_str().unwrap_or_default().to_string())
        .collect();
    let unique: BTreeSet<&String> = names.iter().collect();
    assert_eq!(
        unique.len(),
        exports,
        "case the page walk: expected every export reported exactly once, got {names:?}"
    );

    // The first page stops before the record the second page starts with, and
    // that token is what resumes the walk.
    let first = emulator.ok("GetExportSnapshotRecords", json!({})).await;
    let token = first["nextPageToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case the first page: expected a next page token, got {first:?}"))
        .to_string();
    assert_eq!(
        token, names[PAGE_SIZE],
        "case the first page: expected the token to name the next record, got {token:?}"
    );
    let resumed = emulator
        .ok("GetExportSnapshotRecords", json!({"pageToken": token}))
        .await;
    let resumed_names: Vec<String> = resumed["exportSnapshotRecords"]
        .as_array()
        .unwrap_or_else(|| panic!("case the resumed page: expected an array, got {resumed:?}"))
        .iter()
        .map(|record| record["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        resumed_names,
        names[PAGE_SIZE..2 * PAGE_SIZE].to_vec(),
        "case the resumed page: expected the records from the token onward, got {resumed_names:?}"
    );

    // The last page carries no token: "A next page token is not returned if
    // there are no more results to display."
    let last = emulator
        .ok(
            "GetExportSnapshotRecords",
            json!({"pageToken": names[2 * PAGE_SIZE]}),
        )
        .await;
    assert!(
        last["nextPageToken"].is_null(),
        "case the last page: expected no next page token, got {:?}",
        last["nextPageToken"]
    );
}

/// RULE: only a snapshot this account holds can be exported; anything else is
/// refused with the `DoesNotExist` envelope and appends no record.
#[tokio::test]
async fn an_export_of_something_that_is_not_a_snapshot_is_refused() {
    let emulator = Emulator::new();
    let row = &ROWS[0];

    // An account that has exported nothing reports an empty list, not an absent
    // one, and no token.
    let empty = emulator.ok("GetExportSnapshotRecords", json!({})).await;
    assert_eq!(
        empty["exportSnapshotRecords"].as_array().map(Vec::len),
        Some(0),
        "case an account with no exports: expected an empty list, got {empty:?}"
    );
    assert!(
        empty["nextPageToken"].is_null(),
        "case an account with no exports: expected no next page token, got {empty:?}"
    );

    create_snapshot(&emulator, row).await;
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["export-refusal-instance"],
                "availabilityZone": row.availability_zone,
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            }),
        )
        .await;

    // Each name below is admitted by the modelled `ResourceName` pattern and
    // names no snapshot: one that was never created, one differing from the
    // stored snapshot's name only in case, the DISK the snapshot was taken of,
    // and an instance.
    let refused = [
        ("a name that was never created", "no-such-snapshot"),
        ("the stored name in another case", "MY-DISK-SNAPSHOT"),
        ("the disk the snapshot was taken of", row.source_name),
        ("an instance", "export-refusal-instance"),
    ];
    for (case, source_snapshot_name) in refused {
        let (status, body) = emulator
            .call(
                "ExportSnapshot",
                json!({"sourceSnapshotName": source_snapshot_name}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the DoesNotExist refusal's status, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the NotFoundException wire error, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist body code, got {body:?}"
        );
        let (records, _) = all_records(&emulator).await;
        assert!(
            records.is_empty(),
            "case {case}: expected a refused export to append no record, got {records:?}"
        );
    }

    // The snapshot itself still exports, so the refusals rejected the request
    // rather than the operation.
    emulator
        .ok(
            "ExportSnapshot",
            json!({"sourceSnapshotName": row.snapshot_name}),
        )
        .await;
    let (records, _) = all_records(&emulator).await;
    assert_eq!(
        records.len(),
        1,
        "case the snapshot itself: expected the one record that export appended, got {records:?}"
    );
}

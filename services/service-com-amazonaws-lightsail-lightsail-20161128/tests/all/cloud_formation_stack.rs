// SPDX-License-Identifier: CC0-1.0

//! Lightsail's CloudFormation stack family — `CreateCloudFormationStack` and the
//! records it leaves behind, `GetCloudFormationStackRecords` — driven over the
//! wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: each `CreateCloudFormationStack` call takes exactly one
//! instance entry, creates ONE AWS CloudFormation stack from the export snapshot
//! record the entry names, and appends exactly ONE CloudFormation stack record
//! that tracks it — every member of that record taken from the stored export
//! snapshot record and the entry's own Availability Zone plus this store's
//! allocators, and `GetCloudFormationStackRecords` reports exactly the records
//! the store holds, a page at a time, with a `nextPageToken` only while records
//! remain. A record is created `Started` and the clock, not a reader, settles it
//! into `Succeeded`; a request outside the service's own rules is refused and
//! appends nothing.

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

/// The prefix the service puts before the id in a CloudFormation stack record's
/// name (`crate::state::CLOUD_FORMATION_STACK_RECORD_NAME_PREFIX`), from the
/// modelled `CloudFormationStackRecord$name` documentation: "It starts with
/// `CloudFormationStackRecord` followed by a GUID."
const RECORD_NAME_PREFIX: &str = "CloudFormationStackRecord-";

/// The prefix the name of the created AWS CloudFormation stack carries
/// (`crate::state::CLOUD_FORMATION_STACK_NAME_PREFIX`).
const STACK_NAME_PREFIX: &str = "Lightsail-";

/// How many records one `GetCloudFormationStackRecords` page carries
/// (`crate::state::CLOUD_FORMATION_STACK_RECORD_PAGE_SIZE`).
const PAGE_SIZE: usize = 10;

/// The most bytes of `userData` one instance entry may carry
/// (`crate::state::MAX_USER_DATA_BYTES`): Amazon EC2's own 16 KB cap on an
/// instance's user data.
const MAX_USER_DATA_BYTES: usize = 16 * 1024;

/// Every `PortInfoSourceType` member, from
/// `smithy_query(query="shape",shape="PortInfoSourceType")` (generated in this
/// crate as `PORT_INFO_SOURCE_TYPE_VALUES`).
const PORT_INFO_SOURCES: &[&str] = &["DEFAULT", "INSTANCE", "NONE", "CLOSED"];

// ---------------------------------------------------------------------------
// Reading the records
// ---------------------------------------------------------------------------

/// Every CloudFormation stack record `GetCloudFormationStackRecords` reports,
/// walking every page.
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
                    .ok(
                        "GetCloudFormationStackRecords",
                        json!({"pageToken": token}),
                    )
                    .await
            }
            None => {
                emulator
                    .ok("GetCloudFormationStackRecords", json!({}))
                    .await
            }
        };
        let page = body["cloudFormationStackRecords"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case listing stack records: expected a cloudFormationStackRecords array, \
                     got {body:?}"
                )
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
            "case listing stack records: the walk did not terminate after {} pages",
            page_sizes.len()
        );
    }
    (records, page_sizes)
}

/// The record names `GetCloudFormationStackRecords` reports, as a set.
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

/// The reported stack record `name` names.
async fn record(emulator: &Emulator, name: &str) -> Value {
    all_records(emulator)
        .await
        .0
        .into_iter()
        .find(|record| record["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("case reading {name}: expected the record to be reported"))
}

/// Poll `GetCloudFormationStackRecords` until the record `name` names reports
/// `want`, answering it.
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

/// Every export snapshot record `GetExportSnapshotRecords` reports, walking every
/// page.
async fn all_export_records(emulator: &Emulator) -> Vec<Value> {
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
                .unwrap_or_else(|| {
                    panic!("case listing export records: expected an array, got {body:?}")
                })
                .clone(),
        );
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break records,
        }
    }
}

/// The export snapshot record `name` names, as `GetExportSnapshotRecords` reports
/// it.
async fn export_record(emulator: &Emulator, name: &str) -> Value {
    all_export_records(emulator)
        .await
        .into_iter()
        .find(|record| record["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("case reading {name}: expected the export record to be reported"))
}

// ---------------------------------------------------------------------------
// The stack table
// ---------------------------------------------------------------------------

/// What one row's exported snapshot was taken of: the two sources
/// `CreateDiskSnapshot` admits in this emulator.
#[derive(Debug)]
enum Source {
    /// A block storage disk of this size in GB, created through `CreateDisk`.
    Disk { size_in_gb: i64 },
    /// An instance's system volume: the instance is created from this blueprint
    /// and bundle.
    Instance {
        blueprint_id: &'static str,
        bundle_id: &'static str,
    },
}

/// The launch script one row's instance entry carries.
#[derive(Debug)]
enum UserData {
    /// The entry carries none — the member is optional.
    None,
    /// This script verbatim.
    Script(&'static str),
    /// The largest user data Amazon EC2 accepts: exactly
    /// [`MAX_USER_DATA_BYTES`] bytes, the published bound itself.
    LargestAccepted,
}

impl UserData {
    /// This row's script, as the request carries it.
    fn script(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Script(script) => Some((*script).to_string()),
            Self::LargestAccepted => Some("a".repeat(MAX_USER_DATA_BYTES)),
        }
    }
}

/// One stack to create, and how many stacks to create from the one export
/// snapshot record.
///
/// The rows differ along everything the rule generalises over: the KIND of
/// resource the exported snapshot was taken of, the Availability Zone (and so the
/// region in every ARN and in the location), the Amazon EC2 instance type's form
/// (family and size, a family carrying a letter suffix, a numbered size), the
/// port configuration — every member of the modelled `PortInfoSourceType` enum
/// appears across the table — the launch script (absent, an ASCII script, a
/// multi-byte one, and exactly the largest Amazon EC2 accepts) and the NUMBER of
/// stacks created from one export snapshot record, because each stack is a record
/// of its own. The instance types span the forms Amazon EC2 names: a plain
/// family, a family with a letter suffix, a HYPHENATED family (`mac2-m2pro`,
/// `c7i-flex` and `u7i-12tb` are real EC2 families), a non-numeric size and a
/// numbered one.
struct Row {
    case: &'static str,
    source_name: &'static str,
    snapshot_name: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    source: Source,
    instance_type: &'static str,
    port_info_source: &'static str,
    user_data: UserData,
    stacks: usize,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "a disk snapshot's export, the default firewall, no launch script",
        source_name: "Disk-Virginia-1",
        snapshot_name: "my-disk-snapshot",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        source: Source::Disk { size_in_gb: 32 },
        instance_type: "t2.micro",
        port_info_source: "DEFAULT",
        user_data: UserData::None,
        stacks: 1,
    },
    Row {
        case: "another region, the source instance's firewall, a launch script, two stacks",
        source_name: "small-disk-8",
        snapshot_name: "small_disk_snapshot_1",
        availability_zone: "eu-west-1c",
        region: "eu-west-1",
        source: Source::Disk { size_in_gb: 8 },
        instance_type: "m5d.2xlarge",
        port_info_source: "INSTANCE",
        user_data: UserData::Script("#!/bin/sh\napt-get -y update\n"),
        stacks: 2,
    },
    Row {
        case: "all ports closed, a multi-byte launch script",
        source_name: "data-disk-4096",
        snapshot_name: "data-disk-4096-backup",
        availability_zone: "ap-southeast-2b",
        region: "ap-southeast-2",
        source: Source::Disk { size_in_gb: 4096 },
        instance_type: "mac2-m2pro.metal",
        port_info_source: "CLOSED",
        user_data: UserData::Script("#!/bin/sh\necho 'café ☕ für alle'\n"),
        stacks: 1,
    },
    Row {
        case: "an instance's system volume, the default EC2 security group",
        source_name: "nano-instance",
        snapshot_name: "nano-system-volume-snapshot",
        availability_zone: "us-west-2a",
        region: "us-west-2",
        source: Source::Instance {
            blueprint_id: "amazon_linux_2023",
            bundle_id: "nano_3_0",
        },
        instance_type: "t3a.nano",
        port_info_source: "NONE",
        user_data: UserData::None,
        stacks: 1,
    },
    Row {
        case: "the largest launch script Amazon EC2 accepts",
        source_name: "medium_instance_2",
        snapshot_name: "medium-system-volume-snapshot",
        availability_zone: "ca-central-1a",
        region: "ca-central-1",
        source: Source::Instance {
            blueprint_id: "debian_12",
            bundle_id: "medium_3_0",
        },
        instance_type: "m6i.32xlarge",
        port_info_source: "DEFAULT",
        user_data: UserData::LargestAccepted,
        stacks: 1,
    },
];

/// Create the resource one row's snapshot is taken of.
async fn create_source(emulator: &Emulator, row: &Row) {
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
        }
    }
}

/// Create one row's snapshot, from the resource it is taken of.
async fn create_snapshot(emulator: &Emulator, row: &Row) {
    create_source(emulator, row).await;
    let mut body = json!({"diskSnapshotName": row.snapshot_name});
    match row.source {
        Source::Disk { .. } => body["diskName"] = json!(row.source_name),
        Source::Instance { .. } => body["instanceName"] = json!(row.source_name),
    }
    emulator.ok("CreateDiskSnapshot", body).await;
}

/// Export `snapshot_name` and answer the one export snapshot record that export
/// appended, once the clock has settled it into `Succeeded` — which is the state
/// a stack can be created from.
async fn succeeded_export(emulator: &Emulator, snapshot_name: &str) -> Value {
    let before: BTreeSet<String> = all_export_records(emulator)
        .await
        .into_iter()
        .map(|record| record["name"].as_str().unwrap_or_default().to_string())
        .collect();
    emulator
        .ok("ExportSnapshot", json!({"sourceSnapshotName": snapshot_name}))
        .await;
    let after: BTreeSet<String> = all_export_records(emulator)
        .await
        .into_iter()
        .map(|record| record["name"].as_str().unwrap_or_default().to_string())
        .collect();
    let new: Vec<String> = after.difference(&before).cloned().collect();
    assert_eq!(
        new.len(),
        1,
        "case exporting {snapshot_name}: expected exactly one new export record, got {new:?}"
    );
    let name = new[0].clone();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let record = export_record(emulator, &name).await;
        if record["state"].as_str() == Some("Succeeded") {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "case exporting {snapshot_name}: expected the export to succeed within 5s, got {:?}",
            record["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The `instances` entry one row asks for, against the export snapshot record
/// `source_name`.
fn entry(row: &Row, source_name: &str) -> Value {
    let mut entry = json!({
        "sourceName": source_name,
        "instanceType": row.instance_type,
        "portInfoSource": row.port_info_source,
        "availabilityZone": row.availability_zone,
    });
    if let Some(script) = row.user_data.script() {
        entry["userData"] = json!(script);
    }
    entry
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: every `CreateCloudFormationStack` call appends one stack record, and
/// every member `GetCloudFormationStackRecords` reports on it comes from the
/// stored export snapshot record and the entry's zone plus the store's own
/// allocators.
#[tokio::test]
async fn every_call_appends_one_record_describing_its_stack() {
    let emulator = Emulator::new();
    let mut record_arns: BTreeSet<String> = BTreeSet::new();
    let mut stack_arns: BTreeSet<String> = BTreeSet::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();
    let mut expected_records = 0usize;
    let mut covered_port_info_sources: BTreeSet<&str> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        create_snapshot(&emulator, row).await;
        covered_port_info_sources.insert(row.port_info_source);

        for stack in 1..=row.stacks {
            // Each stack of a row is built from an export snapshot record of its
            // own, because a second stack may only be created from the same
            // record once the first has finished — pinned on its own below.
            let export = succeeded_export(&emulator, row.snapshot_name).await;
            let export_name = export["name"].as_str().unwrap_or_default().to_string();
            let before = record_names(&emulator).await;

            // The call answers ONE operation-log entry — it creates one stack —
            // against the RECORD it appended, and it is not terminal yet,
            // because the stack is still creating the instance when the call
            // returns.
            let created = emulator
                .ok(
                    "CreateCloudFormationStack",
                    json!({"instances": [entry(row, &export_name)]}),
                )
                .await;
            let operations = created["operations"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case} stack {stack}: expected an operations array, got {created:?}")
                })
                .clone();
            assert_eq!(
                operations.len(),
                1,
                "case {case} stack {stack}: expected exactly one operation, got {operations:?}"
            );
            let operation = operations[0].clone();
            assert_eq!(
                operation["operationType"].as_str(),
                Some("CreateCloudFormationStack"),
                "case {case} stack {stack}: expected the CreateCloudFormationStack operation \
                 type, got {operation:?}"
            );
            assert_eq!(
                operation["resourceType"].as_str(),
                Some("CloudFormationStackRecord"),
                "case {case} stack {stack}: expected the created record's resource type, got \
                 {operation:?}"
            );
            assert_eq!(
                operation["status"].as_str(),
                Some("Started"),
                "case {case} stack {stack}: expected a Started operation, got {operation:?}"
            );
            assert_eq!(
                operation["isTerminal"].as_bool(),
                Some(false),
                "case {case} stack {stack}: expected a non-terminal operation, got {operation:?}"
            );
            assert_eq!(
                operation["location"]["availabilityZone"].as_str(),
                Some(row.availability_zone),
                "case {case} stack {stack}: expected the operation to be located where the new \
                 instance is, got {operation:?}"
            );
            assert_eq!(
                operation["location"]["regionName"].as_str(),
                Some(row.region),
                "case {case} stack {stack}: expected the operation's own region, got {operation:?}"
            );
            let operation_id = operation["id"].as_str().unwrap_or_default().to_string();
            assert!(
                is_uuid_shaped(&operation_id),
                "case {case} stack {stack}: expected a uuid-shaped operation id, got \
                 {operation_id:?}"
            );
            assert!(
                operation_ids.insert(operation_id.clone()),
                "case {case} stack {stack}: expected a fresh operation id, got an already-used one"
            );

            // Exactly one record appeared, and it is the record of THIS call.
            let after = record_names(&emulator).await;
            expected_records += 1;
            assert_eq!(
                after.len(),
                expected_records,
                "case {case} stack {stack}: expected one more record than before, got {after:?}"
            );
            let new_names: Vec<String> = after.difference(&before).cloned().collect();
            assert_eq!(
                new_names.len(),
                1,
                "case {case} stack {stack}: expected exactly one new record, got {new_names:?}"
            );
            let name = new_names[0].clone();
            assert_eq!(
                operation["resourceName"].as_str(),
                Some(name.as_str()),
                "case {case} stack {stack}: expected the operation to name the record it \
                 created, got {operation:?}"
            );
            let record = record(&emulator, &name).await;

            // The record's own identity: a minted name whose id is the id its
            // ARN carries, in the new instance's region and under the emulator
            // account.
            let id = name
                .strip_prefix(RECORD_NAME_PREFIX)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case} stack {stack}: expected a '{RECORD_NAME_PREFIX}' record \
                         name, got {name:?}"
                    )
                })
                .to_string();
            assert!(
                is_uuid_shaped(&id),
                "case {case} stack {stack}: expected a uuid-shaped record id, got {id:?}"
            );
            let want_arn = format!(
                "arn:aws:lightsail:{}:{ACCOUNT}:CloudFormationStackRecord/{id}",
                row.region
            );
            assert_eq!(
                record["arn"].as_str(),
                Some(want_arn.as_str()),
                "case {case} stack {stack}: expected the record's own ARN, got {record:?}"
            );
            assert!(
                record_arns.insert(want_arn),
                "case {case} stack {stack}: expected a fresh record ARN, got an already-used one"
            );
            assert_eq!(
                record["resourceType"].as_str(),
                Some("CloudFormationStackRecord"),
                "case {case} stack {stack}: expected the CloudFormationStackRecord resource \
                 type, got {record:?}"
            );
            assert!(
                record["createdAt"].is_number(),
                "case {case} stack {stack}: expected an epoch-seconds createdAt, got {:?}",
                record["createdAt"]
            );
            assert_eq!(
                record["location"]["availabilityZone"].as_str(),
                Some(row.availability_zone),
                "case {case} stack {stack}: expected the new instance's own zone, got {:?}",
                record["location"]
            );
            assert_eq!(
                record["location"]["regionName"].as_str(),
                Some(row.region),
                "case {case} stack {stack}: expected the new instance's own region, got {:?}",
                record["location"]
            );

            // The stack has only just been created, so the record is still
            // `Started` — and it names the stack it tracks from the start,
            // because that stack is what this call created.
            assert_eq!(
                record["state"].as_str(),
                Some("Started"),
                "case {case} stack {stack}: expected a Started record, got {record:?}"
            );
            let destination = record["destinationInfo"].clone();
            assert_eq!(
                destination["service"].as_str(),
                Some("cloudformation"),
                "case {case} stack {stack}: expected AWS CloudFormation as the destination \
                 service, got {destination:?}"
            );
            let stack_arn = destination["id"].as_str().unwrap_or_default().to_string();
            let want_stack_prefix = format!(
                "arn:aws:cloudformation:{}:{ACCOUNT}:stack/{STACK_NAME_PREFIX}{id}/",
                row.region
            );
            let stack_id = stack_arn
                .strip_prefix(&want_stack_prefix)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case} stack {stack}: expected the created stack's ARN under \
                         '{want_stack_prefix}', got {stack_arn:?}"
                    )
                });
            assert!(
                is_uuid_shaped(stack_id),
                "case {case} stack {stack}: expected the stack's own uuid-shaped id, got \
                 {stack_id:?}"
            );
            assert!(
                stack_arns.insert(stack_arn.clone()),
                "case {case} stack {stack}: expected a stack no other call created, got an \
                 already-used ARN"
            );

            // What the record says about its source: the export snapshot record
            // the entry named, exactly as the store holds it.
            let source_info = record["sourceInfo"].as_array().unwrap_or_else(|| {
                panic!("case {case} stack {stack}: expected a sourceInfo array, got {record:?}")
            });
            assert_eq!(
                source_info.len(),
                1,
                "case {case} stack {stack}: expected the one export record this stack was built \
                 from, got {source_info:?}"
            );
            let source = source_info[0].clone();
            assert_eq!(
                source["resourceType"].as_str(),
                Some("ExportSnapshotRecord"),
                "case {case} stack {stack}: expected an ExportSnapshotRecord source, got \
                 {source:?}"
            );
            assert_eq!(
                source["name"].as_str(),
                Some(export_name.as_str()),
                "case {case} stack {stack}: expected the named export record, got {source:?}"
            );
            assert_eq!(
                source["arn"], export["arn"],
                "case {case} stack {stack}: expected the export record's own ARN, got {source:?}"
            );

            // Then the clock settles the record and the operation it answered,
            // and settling changes nothing else the record reported.
            let settled = wait_for_state(&emulator, &name, "Succeeded").await;
            assert_eq!(
                settled["sourceInfo"], record["sourceInfo"],
                "case {case} stack {stack}: expected the source description to be unchanged, got \
                 {:?}",
                settled["sourceInfo"]
            );
            assert_eq!(
                settled["destinationInfo"], record["destinationInfo"],
                "case {case} stack {stack}: expected the tracked stack to be unchanged, got {:?}",
                settled["destinationInfo"]
            );
            assert_eq!(
                settled["arn"], record["arn"],
                "case {case} stack {stack}: expected the record's ARN to be unchanged, got {:?}",
                settled["arn"]
            );
            let settled_operation = emulator
                .ok("GetOperation", json!({"operationId": operation_id}))
                .await["operation"]
                .clone();
            assert_eq!(
                settled_operation["status"].as_str(),
                Some("Succeeded"),
                "case {case} stack {stack}: expected the settled call's entry to have Succeeded, \
                 got {settled_operation:?}"
            );
            assert_eq!(
                settled_operation["isTerminal"].as_bool(),
                Some(true),
                "case {case} stack {stack}: expected a terminal entry, got {settled_operation:?}"
            );
        }
    }

    // The table covers every member of the modelled `PortInfoSourceType` enum.
    assert_eq!(
        covered_port_info_sources,
        PORT_INFO_SOURCES.iter().copied().collect::<BTreeSet<&str>>(),
        "case the whole table: expected every port configuration to be exercised, got \
         {covered_port_info_sources:?}"
    );
    let (records, page_sizes) = all_records(&emulator).await;
    let want_records: usize = ROWS.iter().map(|row| row.stacks).sum();
    assert_eq!(
        records.len(),
        want_records,
        "case the whole table: expected one record per call, got {page_sizes:?} pages of {} \
         records",
        records.len()
    );
}

/// RULE: the clock, not a reader, settles a stack record — and reading the
/// records leaves them exactly as they were.
#[tokio::test]
async fn a_stack_settles_on_the_clock_and_reads_leave_it_alone() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_snapshot(&emulator, row).await;
    let export = succeeded_export(&emulator, row.snapshot_name).await;
    let export_name = export["name"].as_str().unwrap_or_default().to_string();

    // An account that has created no stack reports an empty list, not an absent
    // one, and no token.
    let empty = emulator
        .ok("GetCloudFormationStackRecords", json!({}))
        .await;
    assert_eq!(
        empty["cloudFormationStackRecords"].as_array().map(Vec::len),
        Some(0),
        "case an account with no stacks: expected an empty list, got {empty:?}"
    );
    assert!(
        empty["nextPageToken"].is_null(),
        "case an account with no stacks: expected no next page token, got {empty:?}"
    );

    emulator
        .ok(
            "CreateCloudFormationStack",
            json!({"instances": [entry(row, &export_name)]}),
        )
        .await;

    // Reading the records repeatedly does not move the stack along: it is
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
        "case repeated reads: expected the one record this call appended, got {first:?}"
    );
    assert_eq!(
        first[0]["state"].as_str(),
        Some("Started"),
        "case repeated reads: expected the stack to still be Started, got {:?}",
        first[0]
    );

    let name = first[0]["name"].as_str().unwrap_or_default().to_string();
    let settled = wait_for_state(&emulator, &name, "Succeeded").await;
    assert_eq!(
        settled["name"], first[0]["name"],
        "case the settled stack: expected the same record, got {settled:?}"
    );
}

/// RULE: a second stack may only be built from an export snapshot record once the
/// stack an earlier call started has finished creating its instance.
#[tokio::test]
async fn a_second_stack_from_one_export_waits_for_the_first() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_snapshot(&emulator, row).await;
    let export = succeeded_export(&emulator, row.snapshot_name).await;
    let export_name = export["name"].as_str().unwrap_or_default().to_string();

    let created = emulator
        .ok(
            "CreateCloudFormationStack",
            json!({"instances": [entry(row, &export_name)]}),
        )
        .await;
    let first_name = created["operations"][0]["resourceName"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // While the first stack is still creating the instance, a second from the
    // same record is refused and appends nothing.
    let (status, body) = emulator
        .call(
            "CreateCloudFormationStack",
            json!({"instances": [entry(row, &export_name)]}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a stack still in progress: expected the invalid-input status, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case a stack still in progress: expected the InvalidInputException wire error, got \
         {body:?}"
    );
    let (records, _) = all_records(&emulator).await;
    assert_eq!(
        records.len(),
        1,
        "case a stack still in progress: expected a refused call to append no record, got \
         {records:?}"
    );

    // Once that stack has finished, the same export snapshot record builds
    // another one — a record of its own.
    wait_for_state(&emulator, &first_name, "Succeeded").await;
    emulator
        .ok(
            "CreateCloudFormationStack",
            json!({"instances": [entry(row, &export_name)]}),
        )
        .await;
    let (records, _) = all_records(&emulator).await;
    assert_eq!(
        records.len(),
        2,
        "case after the first stack finished: expected a second record, got {records:?}"
    );
    let names: BTreeSet<&str> = records
        .iter()
        .map(|record| record["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names.len(),
        2,
        "case after the first stack finished: expected two distinct records, got {names:?}"
    );
}

/// RULE: `GetCloudFormationStackRecords` pages the records it holds — at most
/// `PAGE_SIZE` per page, each page resumable from the token the one before it
/// answered, and a `nextPageToken` only while records remain.
#[tokio::test]
async fn the_records_are_paged_and_every_page_is_resumable() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_snapshot(&emulator, row).await;

    // More than two full pages, so the walk has a first, a middle and a last
    // page. Each stack is built from an export of its own, because a stack still
    // in progress holds its own export record.
    let stacks = PAGE_SIZE * 2 + 3;
    let mut exports = Vec::new();
    for _ in 0..stacks {
        exports.push(
            succeeded_export(&emulator, row.snapshot_name).await["name"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    for (stack, export_name) in exports.iter().enumerate() {
        let created = emulator
            .ok(
                "CreateCloudFormationStack",
                json!({"instances": [entry(row, export_name)]}),
            )
            .await;
        assert_eq!(
            created["operations"].as_array().map(Vec::len),
            Some(1),
            "case stack {stack}: expected one operation per call, got {created:?}"
        );
    }

    let (records, page_sizes) = all_records(&emulator).await;
    assert_eq!(
        page_sizes,
        vec![PAGE_SIZE, PAGE_SIZE, stacks - 2 * PAGE_SIZE],
        "case the page walk: expected full pages until the records run out, got {page_sizes:?}"
    );
    let names: Vec<String> = records
        .iter()
        .map(|record| record["name"].as_str().unwrap_or_default().to_string())
        .collect();
    let unique: BTreeSet<&String> = names.iter().collect();
    assert_eq!(
        unique.len(),
        stacks,
        "case the page walk: expected every stack reported exactly once, got {names:?}"
    );

    // The first page stops before the record the second page starts with, and
    // that token is what resumes the walk.
    let first = emulator
        .ok("GetCloudFormationStackRecords", json!({}))
        .await;
    let token = first["nextPageToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case the first page: expected a next page token, got {first:?}"))
        .to_string();
    assert_eq!(
        token, names[PAGE_SIZE],
        "case the first page: expected the token to name the next record, got {token:?}"
    );
    let resumed = emulator
        .ok(
            "GetCloudFormationStackRecords",
            json!({"pageToken": token}),
        )
        .await;
    let resumed_names: Vec<String> = resumed["cloudFormationStackRecords"]
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
            "GetCloudFormationStackRecords",
            json!({"pageToken": names[2 * PAGE_SIZE]}),
        )
        .await;
    assert!(
        last["nextPageToken"].is_null(),
        "case the last page: expected no next page token, got {:?}",
        last["nextPageToken"]
    );
}

/// RULE: a request the model admits but the service's own rules refuse is
/// answered with the modelled `InvalidInputException` at HTTP 400, and appends no
/// record.
#[tokio::test]
async fn a_request_outside_the_services_rules_is_refused() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_snapshot(&emulator, row).await;
    let export = succeeded_export(&emulator, row.snapshot_name).await;
    let export_name = export["name"].as_str().unwrap_or_default().to_string();
    let one = entry(row, &export_name);

    // A second snapshot exported in ANOTHER region, so one row can ask for a
    // zone the export did not export to.
    let other_row = &ROWS[1];
    create_snapshot(&emulator, other_row).await;
    let other_export = succeeded_export(&emulator, other_row.snapshot_name).await;
    let other_export_name = other_export["name"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // An export whose own export has NOT finished: a stack has nothing to build
    // the instance from yet.
    let started_export = {
        let before: BTreeSet<String> = all_export_records(&emulator)
            .await
            .into_iter()
            .map(|record| record["name"].as_str().unwrap_or_default().to_string())
            .collect();
        emulator
            .ok(
                "ExportSnapshot",
                json!({"sourceSnapshotName": row.snapshot_name}),
            )
            .await;
        let after: BTreeSet<String> = all_export_records(&emulator)
            .await
            .into_iter()
            .map(|record| record["name"].as_str().unwrap_or_default().to_string())
            .collect();
        after
            .difference(&before)
            .next()
            .cloned()
            .expect("the export appended a record")
    };

    let too_long = "a".repeat(MAX_USER_DATA_BYTES + 1);
    let refused: Vec<(&str, Value)> = vec![
        ("no instance entry at all", json!({"instances": []})),
        (
            "two instance entries in one call",
            json!({"instances": [one.clone(), one.clone()]}),
        ),
        (
            "an instance type with no size",
            json!({"instances": [{
                "sourceName": export_name,
                "instanceType": "t2micro",
                "portInfoSource": "DEFAULT",
                "availabilityZone": row.availability_zone,
            }]}),
        ),
        (
            "an instance type in upper case",
            json!({"instances": [{
                "sourceName": export_name,
                "instanceType": "T2.MICRO",
                "portInfoSource": "DEFAULT",
                "availabilityZone": row.availability_zone,
            }]}),
        ),
        (
            "an instance type with an empty size",
            json!({"instances": [{
                "sourceName": export_name,
                "instanceType": "t2.",
                "portInfoSource": "DEFAULT",
                "availabilityZone": row.availability_zone,
            }]}),
        ),
        (
            "an instance type with a third part",
            json!({"instances": [{
                "sourceName": export_name,
                "instanceType": "t2.micro.large",
                "portInfoSource": "DEFAULT",
                "availabilityZone": row.availability_zone,
            }]}),
        ),
        (
            "an instance type whose family ends in a hyphen",
            json!({"instances": [{
                "sourceName": export_name,
                "instanceType": "c7i-.large",
                "portInfoSource": "DEFAULT",
                "availabilityZone": row.availability_zone,
            }]}),
        ),
        (
            "an instance type whose family begins with a hyphen",
            json!({"instances": [{
                "sourceName": export_name,
                "instanceType": "-c7i.large",
                "portInfoSource": "DEFAULT",
                "availabilityZone": row.availability_zone,
            }]}),
        ),
        (
            "an instance type whose size carries a hyphen",
            json!({"instances": [{
                "sourceName": export_name,
                "instanceType": "c7i.large-x",
                "portInfoSource": "DEFAULT",
                "availabilityZone": row.availability_zone,
            }]}),
        ),
        (
            "a launch script one byte past what Amazon EC2 accepts",
            json!({"instances": [{
                "sourceName": export_name,
                "instanceType": "t2.micro",
                "portInfoSource": "DEFAULT",
                "availabilityZone": row.availability_zone,
                "userData": too_long,
            }]}),
        ),
        (
            "a region name where a zone belongs",
            json!({"instances": [{
                "sourceName": export_name,
                "instanceType": "t2.micro",
                "portInfoSource": "DEFAULT",
                "availabilityZone": row.region,
            }]}),
        ),
        (
            "a zone of no region this service serves",
            json!({"instances": [{
                "sourceName": export_name,
                "instanceType": "t2.micro",
                "portInfoSource": "DEFAULT",
                "availabilityZone": "mars-central-1a",
            }]}),
        ),
        (
            "a zone letter in upper case",
            json!({"instances": [{
                "sourceName": export_name,
                "instanceType": "t2.micro",
                "portInfoSource": "DEFAULT",
                "availabilityZone": "us-east-1A",
            }]}),
        ),
        (
            "a zone in another region than the export exported to",
            json!({"instances": [{
                "sourceName": other_export_name,
                "instanceType": "t2.micro",
                "portInfoSource": "DEFAULT",
                "availabilityZone": row.availability_zone,
            }]}),
        ),
        (
            "an export whose own export has not finished",
            json!({"instances": [{
                "sourceName": started_export,
                "instanceType": "t2.micro",
                "portInfoSource": "DEFAULT",
                "availabilityZone": row.availability_zone,
            }]}),
        ),
    ];

    for (case, body) in refused {
        let (status, parsed) = emulator.call("CreateCloudFormationStack", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the invalid-input status, got {status} {parsed:?}"
        );
        assert_eq!(
            error_code(&parsed),
            "InvalidInputException",
            "case {case}: expected the InvalidInputException wire error, got {parsed:?}"
        );
        let (records, _) = all_records(&emulator).await;
        assert!(
            records.is_empty(),
            "case {case}: expected a refused call to append no record, got {records:?}"
        );
    }

    // The entry the table admits still creates a stack, so the refusals rejected
    // the request rather than the operation.
    emulator
        .ok(
            "CreateCloudFormationStack",
            json!({"instances": [entry(row, &export_name)]}),
        )
        .await;
    let (records, _) = all_records(&emulator).await;
    assert_eq!(
        records.len(),
        1,
        "case the admitted entry: expected the one record that call appended, got {records:?}"
    );
}

/// RULE: `portInfoSource` is a MODELLED enum, so a value outside
/// [`PORT_INFO_SOURCES`] is refused by the boundary walk — HTTP 400
/// `ValidationException` — before the handler runs, and appends no record.
#[tokio::test]
async fn a_port_configuration_outside_the_modelled_enum_is_refused() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_snapshot(&emulator, row).await;
    let export = succeeded_export(&emulator, row.snapshot_name).await;
    let export_name = export["name"].as_str().unwrap_or_default().to_string();

    // Each value below is outside the modelled `PortInfoSourceType` enum: one
    // the enum does not carry at all, one of its members in the wrong case, and
    // the empty string.
    for (case, port_info_source) in [
        ("a value the enum does not carry", "OPEN"),
        ("a member in the wrong case", "default"),
        ("the empty string", ""),
    ] {
        let (status, body) = emulator
            .call(
                "CreateCloudFormationStack",
                json!({"instances": [{
                    "sourceName": export_name,
                    "instanceType": "t2.micro",
                    "portInfoSource": port_info_source,
                    "availabilityZone": row.availability_zone,
                }]}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the boundary refusal's status, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected the modelled-constraint refusal, got {body:?}"
        );
        let (records, _) = all_records(&emulator).await;
        assert!(
            records.is_empty(),
            "case {case}: expected a refused call to append no record, got {records:?}"
        );
    }

    // Every member of the enum is accepted, one stack at a time from an export
    // of its own.
    for port_info_source in PORT_INFO_SOURCES {
        let export = succeeded_export(&emulator, row.snapshot_name).await;
        let export_name = export["name"].as_str().unwrap_or_default().to_string();
        emulator
            .ok(
                "CreateCloudFormationStack",
                json!({"instances": [{
                    "sourceName": export_name,
                    "instanceType": "t2.micro",
                    "portInfoSource": port_info_source,
                    "availabilityZone": row.availability_zone,
                }]}),
            )
            .await;
    }
    let (records, _) = all_records(&emulator).await;
    assert_eq!(
        records.len(),
        PORT_INFO_SOURCES.len(),
        "case every modelled member: expected one record per accepted port configuration, got \
         {records:?}"
    );
}

/// RULE: only an export snapshot record this account holds can be built from;
/// anything else is refused with the `DoesNotExist` envelope and appends no
/// record.
#[tokio::test]
async fn a_source_that_is_not_an_export_record_is_refused() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_snapshot(&emulator, row).await;
    let export = succeeded_export(&emulator, row.snapshot_name).await;
    let export_name = export["name"].as_str().unwrap_or_default().to_string();

    // Each name below is admitted by the modelled `ResourceName` pattern and
    // names no export snapshot record: one that was never created, the stored
    // name in another case, the SNAPSHOT that was exported, and the disk the
    // snapshot was taken of.
    let refused = [
        ("a name that was never created", "no-such-export-record".to_string()),
        ("the stored name in upper case", export_name.to_uppercase()),
        ("the snapshot that was exported", row.snapshot_name.to_string()),
        ("the disk the snapshot was taken of", row.source_name.to_string()),
    ];
    for (case, source_name) in refused {
        let (status, body) = emulator
            .call(
                "CreateCloudFormationStack",
                json!({"instances": [{
                    "sourceName": source_name,
                    "instanceType": "t2.micro",
                    "portInfoSource": "DEFAULT",
                    "availabilityZone": row.availability_zone,
                }]}),
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
            "case {case}: expected a refused call to append no record, got {records:?}"
        );
    }
}

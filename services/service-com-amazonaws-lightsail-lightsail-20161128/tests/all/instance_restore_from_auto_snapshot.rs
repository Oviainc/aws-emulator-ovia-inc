// SPDX-License-Identifier: CC0-1.0

//! Restoring Lightsail instances from an AUTOMATIC snapshot of an instance,
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a restore that addresses an automatic snapshot resolves
//! it among the copies the `AutoSnapshot` add-on has taken of the instance
//! `sourceInstanceName` names — the copy dated `restoreDate`, or the newest copy
//! the service has FINISHED taking where `useLatestRestorableAutoSnapshot` asks
//! for the latest restorable one — and creates one instance per requested name
//! running the image that instance runs, on the plan the request names and never
//! one whose SSD is smaller than the system volume the copy was taken of, with
//! one block storage disk per `attachedDiskMapping` entry sized by the disk THAT
//! COPY captured at the named device path, leaving the copy and the instance it
//! was taken of exactly as they were. A request addressing a copy this account
//! does not hold is refused with the service's `DoesNotExist` envelope, one
//! addressing a copy the service has not finished taking with the modelled
//! `OperationFailureException`, and neither stores anything.
//!
//! The manual source of the same operation — `instanceSnapshotName` — is pinned
//! in `instance_restore_from_snapshot.rs`, the same automatic source under
//! `CreateDiskFromSnapshot`'s member names in `disk_restore_from_auto_snapshot.rs`
//! and under `CopySnapshot`'s in `snapshot_copy.rs`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state as lightsail_state;

// The harness, the wire error code, the minted-id check and the fixtures the
// sibling suites of this test target already publish: this suite drives the
// service through the same front door and builds its copies the same way rather
// than keeping second copies of either.
use crate::auto_snapshots::{auto_snapshot_request, copies, utc_date_of, wait_for_success};
use crate::disk_attachment::{create_available_disk, wait_for_instance_state};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The device path an instance's system volume is exposed at
/// (`crate::state::SYSTEM_DISK_PATH`).
const SYSTEM_DISK_PATH: &str = "/dev/xvda";

/// A day, in milliseconds: the add-on takes one copy a day, so two consecutive
/// copies of one instance are dated a day apart.
const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

/// The modelled `AutoSnapshotStatus` values a copy reports: the one a copy the
/// service is still taking is in, and the one it settles into.
const IN_PROGRESS: &str = "InProgress";
/// See [`IN_PROGRESS`].
const SUCCESS: &str = "Success";

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// The stored instance `name`, as `GetInstance` reports it.
async fn instance(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetInstance", json!({"instanceName": name})).await["instance"].clone()
}

/// The stored disk `name`, as `GetDisk` reports it.
async fn disk(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetDisk", json!({"diskName": name})).await["disk"].clone()
}

/// The published plan `bundle_id`, as the service's own `GetBundles` catalog
/// reports it, so a case's expectation of the plan's SSD is the service's own
/// account of it.
async fn bundle(emulator: &Emulator, bundle_id: &str) -> Value {
    emulator.ok("GetBundles", json!({})).await["bundles"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading bundle {bundle_id}: expected an array"))
        .iter()
        .find(|entry| entry["bundleId"].as_str() == Some(bundle_id))
        .unwrap_or_else(|| panic!("case reading bundle {bundle_id}: the catalog does not hold it"))
        .clone()
}

/// A `TagList` body member from `(key, value)` pairs.
fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// The restore table
// ---------------------------------------------------------------------------

/// How a row's request says WHICH automatic snapshot it restores: the two
/// mutually exclusive members the modelled input admits for that.
#[derive(Debug, Clone, Copy)]
enum Addressed {
    /// `restoreDate`: the UTC date the copy was taken on.
    ByDate,
    /// `useLatestRestorableAutoSnapshot`: the latest copy the service has
    /// finished taking.
    Latest,
}

/// One restore from an automatic snapshot the service must serve.
///
/// The rows differ along everything the rule generalises over: WHICH member
/// addresses the copy, the IMAGE the copied instance runs (a Linux blueprint, an
/// application one, a Windows one) and so the default firewall the restore
/// inherits, the plan the source ran on and the plan it is restored onto (the
/// same one, a larger one, a much larger one), the Region and zone the source
/// lives in and whether the instances are restored into that zone or another of
/// the same Region, HOW MANY instances one call creates (one, two, three), how
/// many disks the copy captured and how many of them the mapping restores (none,
/// one, both), the hour the add-on carries, the number of tags, the IP address
/// type, and the form of the new names.
#[derive(Debug)]
struct Row {
    case: &'static str,
    source_instance: &'static str,
    source_zone: &'static str,
    /// The zone the instances are restored into — in the source instance's own
    /// Region.
    restore_zone: &'static str,
    region: &'static str,
    /// A blueprint id this service publishes (`GetBlueprints`).
    blueprint_id: &'static str,
    /// The plan the source instance runs on, whose SSD is the system volume the
    /// copy was taken of.
    source_bundle_id: &'static str,
    /// The plan the restore creates the instances on, which the request names.
    restore_bundle_id: &'static str,
    /// The block storage disks attached to the source BEFORE the add-on is
    /// enabled, so the copy captures them: name, device path, size in GB.
    source_disks: &'static [(&'static str, &'static str, i64)],
    restored_names: &'static [&'static str],
    /// `attachedDiskMapping` entries: the index into `restored_names` of the
    /// instance they belong to, the captured device path, and the name to
    /// restore that disk under.
    mapping: &'static [(usize, &'static str, &'static str)],
    addressed: Addressed,
    /// The `snapshotTimeOfDay` the source's add-on carries, or `None` to let the
    /// service pick the hour.
    hour: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    ip_address_type: Option<&'static str>,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "a copy of a Linux instance, restored by date onto its own plan in its own zone",
        source_instance: "auto-linux-source",
        source_zone: "us-east-1a",
        restore_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "amazon_linux_2023",
        source_bundle_id: "nano_3_0",
        restore_bundle_id: "nano_3_0",
        source_disks: &[],
        restored_names: &["auto-restored-linux"],
        mapping: &[],
        addressed: Addressed::ByDate,
        hour: None,
        tags: &[],
        ip_address_type: None,
    },
    Row {
        case: "the latest restorable copy of an application instance, two instances onto a \
               larger plan in another zone of the Region, each taking its own copy of the one \
               captured disk",
        source_instance: "auto_wordpress_source",
        source_zone: "eu-west-1c",
        restore_zone: "eu-west-1a",
        region: "eu-west-1",
        blueprint_id: "wordpress",
        source_bundle_id: "small_3_0",
        restore_bundle_id: "large_3_0",
        source_disks: &[("auto-wp-data", "/dev/xvdf", 24)],
        restored_names: &["auto_wp_1", "auto_wp_2"],
        mapping: &[
            (0, "/dev/xvdf", "auto-wp-data-1"),
            (1, "/dev/xvdf", "auto_wp_data_2"),
        ],
        addressed: Addressed::Latest,
        hour: Some("06:00"),
        tags: &[("tier", "edge")],
        ip_address_type: Some("ipv4"),
    },
    Row {
        case: "a copy of a Windows instance capturing two disks, both restored, by date, two \
               tags",
        source_instance: "auto-windows-source",
        source_zone: "ca-central-1a",
        restore_zone: "ca-central-1a",
        region: "ca-central-1",
        blueprint_id: "windows_server_2022",
        source_bundle_id: "medium_3_0",
        restore_bundle_id: "medium_3_0",
        source_disks: &[("auto-win-a", "/dev/xvdh", 100), ("auto-win-b", "/dev/xvdi", 8)],
        restored_names: &["auto-win-restore"],
        mapping: &[
            (0, "/dev/xvdh", "auto-win-restore-a"),
            (0, "/dev/xvdi", "AutoWinRestoreB"),
        ],
        addressed: Addressed::ByDate,
        hour: Some("23:00"),
        tags: &[("stack", "prod"), ("owner", "storage-team")],
        ip_address_type: Some("dualstack"),
    },
    Row {
        case: "the latest restorable copy, three instances onto a much larger plan, mapping \
               none of the captured disks",
        source_instance: "auto-nodejs-source",
        source_zone: "us-west-2a",
        restore_zone: "us-west-2b",
        region: "us-west-2",
        blueprint_id: "nodejs",
        source_bundle_id: "nano_3_0",
        restore_bundle_id: "2xlarge_3_0",
        source_disks: &[("auto-node-data", "/dev/xvdf", 16)],
        restored_names: &["auto-node-1", "auto-node-2", "auto-node-3"],
        mapping: &[],
        addressed: Addressed::Latest,
        hour: None,
        tags: &[("keep", "yes")],
        ip_address_type: Some("ipv6"),
    },
];

/// Stand up `row`'s source instance with every disk the row attaches, enable the
/// add-on that takes the copies and wait for the copy the CLOCK then owes it,
/// answering that copy's date.
///
/// The disks are attached BEFORE the add-on is enabled, so the copy the schedule
/// then owes captures them.
async fn with_one_copy(emulator: &Emulator, row: &Row) -> String {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [row.source_instance],
                "availabilityZone": row.source_zone,
                "blueprintId": row.blueprint_id,
                "bundleId": row.source_bundle_id,
            }),
        )
        .await;
    wait_for_instance_state(emulator, row.source_instance, "running").await;
    for (disk_name, disk_path, size_in_gb) in row.source_disks {
        create_available_disk(emulator, disk_name, row.source_zone, *size_in_gb).await;
        emulator
            .ok(
                "AttachDisk",
                json!({
                    "diskName": disk_name,
                    "instanceName": row.source_instance,
                    "diskPath": disk_path,
                }),
            )
            .await;
    }
    emulator
        .ok(
            "EnableAddOn",
            json!({
                "resourceName": row.source_instance,
                "addOnRequest": auto_snapshot_request(row.hour),
            }),
        )
        .await;
    let copy = wait_for_success(emulator, row.case, row.source_instance).await;
    copy["date"]
        .as_str()
        .unwrap_or_else(|| panic!("case {}: expected the copy's date, got {copy:?}", row.case))
        .to_string()
}

/// The `CreateInstancesFromSnapshot` body for `row`, restoring the copy dated
/// `date`.
fn restore_body(row: &Row, date: &str) -> Value {
    let mut body = json!({
        "instanceNames": row.restored_names,
        "availabilityZone": row.restore_zone,
        "bundleId": row.restore_bundle_id,
        "sourceInstanceName": row.source_instance,
    });
    match row.addressed {
        Addressed::ByDate => body["restoreDate"] = json!(date),
        Addressed::Latest => body["useLatestRestorableAutoSnapshot"] = json!(true),
    }
    if !row.mapping.is_empty() {
        let mut mapping = serde_json::Map::new();
        for (index, original_path, new_name) in row.mapping {
            let entry = json!({"originalDiskPath": original_path, "newDiskName": new_name});
            mapping
                .entry(row.restored_names[*index].to_string())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .expect("a mapping value is an array")
                .push(entry);
        }
        body["attachedDiskMapping"] = Value::Object(mapping);
    }
    if !row.tags.is_empty() {
        body["tags"] = tag_list(row.tags);
    }
    if let Some(ip_address_type) = row.ip_address_type {
        body["ipAddressType"] = json!(ip_address_type);
    }
    body
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: an instance restored from an automatic snapshot of an instance runs the
/// image that instance runs, on the plan the request names, in the zone it
/// names, with one block storage disk per mapping entry sized by the disk the
/// copy captured at that device path; every member `GetInstance` and `GetDisk`
/// report of it comes from the ONE record the restore stored, and the copy it was
/// restored from — and the instance that copy was taken of — are left exactly as
/// they were.
#[tokio::test]
async fn instances_restored_from_an_automatic_snapshot_copy_the_instance_it_was_taken_of() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let date = with_one_copy(&emulator, row).await;
        let plan = bundle(&emulator, row.restore_bundle_id).await;
        let system_disk_size = plan["diskSizeInGb"].as_i64().unwrap_or_default();

        // What the restore must leave untouched, read from the service's own
        // records before it runs.
        let source_before = instance(&emulator, row.source_instance).await;
        let copies_before = copies(&emulator, row.source_instance).await;

        let answered = emulator
            .ok("CreateInstancesFromSnapshot", restore_body(row, &date))
            .await;

        // A restored instance is not serving yet: the clock, not a reader,
        // settles it, so a caller reading straight back still sees `pending`.
        // Read first, before the rest of the assertions spend that span.
        for name in row.restored_names {
            let fresh = instance(&emulator, name).await;
            assert_eq!(
                fresh["state"]["name"].as_str(),
                Some("pending"),
                "case {case}: expected a pending instance, got {fresh:?}"
            );
        }

        // One operation-log entry per instance the call creates, none of them
        // terminal yet.
        let operations = answered["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {answered:?}"))
            .clone();
        assert_eq!(
            operations.len(),
            row.restored_names.len(),
            "case {case}: expected one operation per requested instance, got {operations:?}"
        );
        for (operation, name) in operations.iter().zip(row.restored_names) {
            assert_eq!(
                (
                    operation["operationType"].as_str(),
                    operation["resourceType"].as_str(),
                    operation["resourceName"].as_str(),
                    operation["isTerminal"].as_bool(),
                ),
                (
                    Some("CreateInstancesFromSnapshot"),
                    Some("Instance"),
                    Some(*name),
                    Some(false),
                ),
                "case {case}: expected a non-terminal entry naming the new instance, got \
                 {operation:?}"
            );
            let id = operation["id"].as_str().unwrap_or_default().to_string();
            assert!(
                is_uuid_shaped(&id),
                "case {case}: expected a uuid-shaped operation id, got {id:?}"
            );
            assert!(
                operation_ids.insert(id.clone()),
                "case {case}: expected a fresh operation id, got the already-used {id:?}"
            );
        }

        // The stored records, read back through the implemented readers.
        for name in row.restored_names {
            let restored = instance(&emulator, name).await;
            // The IMAGE is the copied instance's: this input models no
            // blueprint.
            assert_eq!(
                restored["blueprintId"].as_str(),
                Some(row.blueprint_id),
                "case {case}: expected the copied instance's blueprint, got {restored:?}"
            );
            // The PLAN is the request's.
            assert_eq!(
                restored["bundleId"].as_str(),
                Some(row.restore_bundle_id),
                "case {case}: expected the requested bundle, got {restored:?}"
            );
            assert_eq!(
                (
                    restored["location"]["availabilityZone"].as_str(),
                    restored["location"]["regionName"].as_str(),
                ),
                (Some(row.restore_zone), Some(row.region)),
                "case {case}: expected the requested zone and its Region, got {:?}",
                restored["location"]
            );
            assert_eq!(
                restored["ipAddressType"].as_str(),
                Some(row.ip_address_type.unwrap_or("dualstack")),
                "case {case}: expected the requested IP address type, got {restored:?}"
            );
            assert_eq!(
                restored["tags"],
                tag_list(row.tags),
                "case {case}: expected the requested tags, got {:?}",
                restored["tags"]
            );
            // A restored instance carries only the add-ons the request asked
            // for: the copied instance's own are not carried over.
            assert_eq!(
                restored["addOns"].as_array().map(Vec::len),
                Some(0),
                "case {case}: expected no add-ons, got {:?}",
                restored["addOns"]
            );
            assert!(
                restored["networking"]["ports"]
                    .as_array()
                    .is_some_and(|ports| !ports.is_empty()),
                "case {case}: expected the platform's default firewall, got {:?}",
                restored["networking"]
            );

            // The system disk first, then one disk per mapping entry for this
            // instance, in the order the mapping listed them.
            let mapped: Vec<&(usize, &str, &str)> = row
                .mapping
                .iter()
                .filter(|(index, _, _)| row.restored_names[*index] == *name)
                .collect();
            let disks = restored["hardware"]["disks"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected a disks array, got {restored:?}"))
                .clone();
            assert_eq!(
                disks.len(),
                1 + mapped.len(),
                "case {case}: expected the system disk and every mapped disk, got {disks:?}"
            );
            assert_eq!(
                (
                    disks[0]["isSystemDisk"].as_bool(),
                    disks[0]["path"].as_str(),
                    disks[0]["sizeInGb"].as_i64(),
                ),
                (Some(true), Some(SYSTEM_DISK_PATH), Some(system_disk_size)),
                "case {case}: expected the restore plan's system volume first, got {:?}",
                disks[0]
            );

            for (_, original_path, new_disk_name) in mapped.iter() {
                // The size the COPY captured for that device path, read off the
                // disk the suite attached before the copy was taken.
                let captured_size = row
                    .source_disks
                    .iter()
                    .find(|(_, path, _)| path == original_path)
                    .map(|(_, _, size)| *size)
                    .unwrap_or_else(|| {
                        panic!("case {case}: the row maps a path its source never attached")
                    });
                let restored_disk = disk(&emulator, new_disk_name).await;
                assert_eq!(
                    (
                        restored_disk["sizeInGb"].as_i64(),
                        restored_disk["path"].as_str(),
                        restored_disk["attachedTo"].as_str(),
                        restored_disk["state"].as_str(),
                    ),
                    (
                        Some(captured_size),
                        Some(*original_path),
                        Some(*name),
                        Some("in-use"),
                    ),
                    "case {case}: expected the captured volume attached to the new instance, \
                     got {restored_disk:?}"
                );
                assert_eq!(
                    restored_disk["location"]["availabilityZone"].as_str(),
                    Some(row.restore_zone),
                    "case {case}: expected the disk where the instance was restored, got \
                     {restored_disk:?}"
                );
                let disk_arn = restored_disk["arn"].as_str().unwrap_or_default().to_string();
                let want_disk_prefix = format!("arn:aws:lightsail:{}:{ACCOUNT}:Disk/", row.region);
                assert!(
                    disk_arn.starts_with(&want_disk_prefix)
                        && is_uuid_shaped(&disk_arn[want_disk_prefix.len()..]),
                    "case {case}: expected a disk ARN of the restore Region, got {disk_arn:?}"
                );
                assert!(
                    arns.insert(disk_arn.clone()),
                    "case {case}: expected a fresh disk ARN, got the already-used {disk_arn:?}"
                );
            }

            // Exactly the mapped disks hang off the instance beside its system
            // volume — no more and no fewer, whatever order the mapping was
            // keyed in.
            let mut reported_disk_names: Vec<String> = disks
                .iter()
                .filter(|entry| entry["isSystemDisk"].as_bool() != Some(true))
                .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
                .collect();
            reported_disk_names.sort();
            let mut want_disk_names: Vec<String> = mapped
                .iter()
                .map(|(_, _, new_disk_name)| (*new_disk_name).to_string())
                .collect();
            want_disk_names.sort();
            assert_eq!(
                reported_disk_names, want_disk_names,
                "case {case}: expected exactly the mapped disks attached, got {disks:?}"
            );

            // Server-assigned identity: an ARN of the restore Region carrying a
            // minted id, and a support code naming the account.
            let arn = restored["arn"].as_str().unwrap_or_default().to_string();
            let want_prefix = format!("arn:aws:lightsail:{}:{ACCOUNT}:Instance/", row.region);
            assert!(
                arn.starts_with(&want_prefix) && is_uuid_shaped(&arn[want_prefix.len()..]),
                "case {case}: expected an instance ARN of the restore Region, got {arn:?}"
            );
            assert!(
                arns.insert(arn.clone()),
                "case {case}: expected a fresh instance ARN, got the already-used {arn:?}"
            );
            assert!(
                restored["supportCode"]
                    .as_str()
                    .is_some_and(|code| code.contains(ACCOUNT)),
                "case {case}: expected a support code carrying the account, got {restored:?}"
            );

            // The clock settles the restored instance, which the pending read
            // above pinned.
            wait_for_instance_state(&emulator, name, "running").await;
        }

        // A restore READS the copy and the instance it was taken of: neither has
        // moved.
        let copies_after = copies(&emulator, row.source_instance).await;
        assert_eq!(
            copies_after, copies_before,
            "case {case}: expected the copies of {} to be untouched, got {copies_after:?}",
            row.source_instance
        );
        let source_after = instance(&emulator, row.source_instance).await;
        assert_eq!(
            source_after, source_before,
            "case {case}: expected the copied instance to be untouched, got {source_after:?}"
        );
    }
}

/// Give the instance `name` two further copies: one taken the day BEFORE its
/// newest copy, finished, capturing NO attached disk, and one taken the day
/// AFTER it that the service has not finished taking — answering their dates,
/// oldest first.
///
/// An instance holding copies for several dates is one whose add-on has been
/// enabled for several days, and a copy read mid-capture is one read inside the
/// interval the service takes to finish taking it: neither is something a
/// sequence of API calls can produce inside one test. Both are therefore seeded
/// where the store holds them, through the same public state API a handler
/// reads, so the rule is exercised against the ONE persisted schedule — each
/// written exactly as the add-on's own taker writes it, dated by the UTC date of
/// the instant it was taken, and carrying the disks that were attached when it
/// was taken.
fn seed_neighbouring_copies(emulator: &Emulator, name: &str) -> (String, String) {
    lightsail_state::with_state(emulator.engine(), |store| {
        let schedule = store
            .auto_snapshots
            .get_mut(&(lightsail_state::RESOURCE_TYPE_INSTANCE, name.to_string()))
            .expect("the suite enabled the add-on and waited for its first copy");
        let newest = schedule
            .taken
            .last()
            .expect("the suite waited for the add-on's first copy");
        let newest_ms = newest.created_at_ms;
        let captured = newest.from_attached_disks.clone();
        let earlier_ms = newest_ms - DAY_MS;
        let later_ms = newest_ms + DAY_MS;
        let earlier = utc_date_of(earlier_ms as f64 / 1_000.0);
        let later = utc_date_of(later_ms as f64 / 1_000.0);
        schedule.taken.insert(
            0,
            lightsail_state::StoredAutoSnapshot {
                date: earlier.clone(),
                created_at_ms: earlier_ms,
                status: lightsail_state::AUTO_SNAPSHOT_STATUS_SUCCESS,
                due: None,
                // Nothing was attached to the instance a day earlier.
                from_attached_disks: Some(Vec::new()),
            },
        );
        schedule.taken.push(lightsail_state::StoredAutoSnapshot {
            date: later.clone(),
            created_at_ms: later_ms,
            status: lightsail_state::AUTO_SNAPSHOT_STATUS_IN_PROGRESS,
            due: None,
            from_attached_disks: captured,
        });
        (earlier, later)
    })
    .expect("the suite's engine is the shared in-memory one")
}

/// RULE: a date addresses exactly one of an instance's copies and the latest
/// restorable copy is the newest one the service has FINISHED taking, so an
/// instance holding several copies restores the one the request addresses —
/// including the disks THAT copy captured — while a copy still being taken is
/// refused with the modelled `OperationFailureException` and a date no copy was
/// taken on with the `DoesNotExist` envelope.
#[tokio::test]
async fn the_copy_a_restore_uses_is_the_one_the_request_addresses() {
    let emulator = Emulator::new();
    let case = "an instance holding three copies";
    let source = "staged-instance";
    let zone = "us-east-1a";
    let bundle_id = "nano_3_0";
    let attached_path = "/dev/xvdf";
    let attached_gb = 48;
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [source],
                "availabilityZone": zone,
                "blueprintId": "amazon_linux_2023",
                "bundleId": bundle_id,
            }),
        )
        .await;
    wait_for_instance_state(&emulator, source, "running").await;
    create_available_disk(&emulator, "staged-data", zone, attached_gb).await;
    emulator
        .ok(
            "AttachDisk",
            json!({
                "diskName": "staged-data",
                "instanceName": source,
                "diskPath": attached_path,
            }),
        )
        .await;
    emulator
        .ok(
            "EnableAddOn",
            json!({
                "resourceName": source,
                "addOnRequest": auto_snapshot_request(Some("06:00")),
            }),
        )
        .await;
    let finished = wait_for_success(&emulator, case, source).await;
    let finished_date = finished["date"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected the copy's date, got {finished:?}"))
        .to_string();
    let (earlier_date, unfinished_date) = seed_neighbouring_copies(&emulator, source);

    // The precondition the cases are about, read from the service's own record
    // rather than assumed: three copies, oldest first, the newest unfinished.
    let reported = copies(&emulator, source).await;
    let seen: Vec<(Option<&str>, Option<&str>)> = reported
        .iter()
        .map(|copy| (copy["date"].as_str(), copy["status"].as_str()))
        .collect();
    assert_eq!(
        seen,
        vec![
            (Some(earlier_date.as_str()), Some(SUCCESS)),
            (Some(finished_date.as_str()), Some(SUCCESS)),
            (Some(unfinished_date.as_str()), Some(IN_PROGRESS)),
        ],
        "case {case}: expected the three copies oldest first, only the newest unfinished, got \
         {reported:?}"
    );

    // The copy still being taken holds no complete instance to restore from.
    let (status, answered) = emulator
        .call(
            "CreateInstancesFromSnapshot",
            json!({
                "instanceNames": ["from-unfinished"],
                "availabilityZone": zone,
                "bundleId": bundle_id,
                "sourceInstanceName": source,
                "restoreDate": unfinished_date,
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "OperationFailureException"),
        "case {case} restoring the unfinished copy: expected the modelled \
         OperationFailureException, got {status} {answered:?}"
    );
    let message = answered["message"].as_str().unwrap_or_default().to_string();
    assert!(
        message.contains(source)
            && message.contains(&unfinished_date)
            && message.contains(IN_PROGRESS)
            && message.contains(SUCCESS),
        "case {case} restoring the unfinished copy: expected the refusal to name the instance, \
         the copy's date and the state it is in, got {message:?}"
    );

    // A date no copy of this instance was taken on addresses no copy at all.
    let absent_date = "2020-01-01";
    let (status, answered) = emulator
        .call(
            "CreateInstancesFromSnapshot",
            json!({
                "instanceNames": ["from-absent-date"],
                "availabilityZone": zone,
                "bundleId": bundle_id,
                "sourceInstanceName": source,
                "restoreDate": absent_date,
            }),
        )
        .await;
    assert_eq!(
        (
            status,
            error_code(&answered).as_str(),
            answered["code"].as_str()
        ),
        (400, "NotFoundException", Some("DoesNotExist")),
        "case {case} restoring a date no copy was taken on: expected the DoesNotExist envelope, \
         got {status} {answered:?}"
    );
    assert_eq!(
        answered["message"].as_str(),
        Some(
            format!("The automatic snapshot of the Instance {source} does not exist: {absent_date}")
                .as_str()
        ),
        "case {case} restoring a date no copy was taken on: expected the resource/date message, \
         got {answered:?}"
    );

    // Neither request stored anything.
    for name in ["from-unfinished", "from-absent-date"] {
        let (read_status, read) = emulator
            .call("GetInstance", json!({"instanceName": name}))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused restore stores nothing, got {read_status} {read:?}"
        );
    }

    // The disks a mapping may address are the ones THAT copy captured: the copy
    // taken a day earlier captured none, so the same mapping that the newest
    // finished copy serves is refused against it.
    let mapping = json!({"from-earlier": [{
        "originalDiskPath": attached_path,
        "newDiskName": "from-earlier-data",
    }]});
    let (status, answered) = emulator
        .call(
            "CreateInstancesFromSnapshot",
            json!({
                "instanceNames": ["from-earlier"],
                "availabilityZone": zone,
                "bundleId": bundle_id,
                "sourceInstanceName": source,
                "restoreDate": earlier_date,
                "attachedDiskMapping": mapping,
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "InvalidInputException"),
        "case {case} mapping a disk the addressed copy never captured: expected the modelled \
         InvalidInputException, got {status} {answered:?}"
    );
    let message = answered["message"].as_str().unwrap_or_default().to_string();
    assert!(
        message.contains("attachedDiskMapping") && message.contains(&earlier_date),
        "case {case} mapping a disk the addressed copy never captured: expected the refusal to \
         name the member and the copy, got {message:?}"
    );

    // Each copy the request CAN address is restored, and the mapping is served
    // by the copy that did capture the disk.
    let served: &[(&str, &str, Value, bool)] = &[
        (
            "the older finished copy, by its date, mapping nothing",
            "from-earlier-plain",
            json!({"restoreDate": earlier_date}),
            false,
        ),
        (
            "the newest finished copy, as the latest restorable one, mapping its captured disk",
            "from-latest",
            json!({"useLatestRestorableAutoSnapshot": true}),
            true,
        ),
    ];
    for (served_case, restored_name, addressed, with_mapping) in served {
        let mut body = json!({
            "instanceNames": [restored_name],
            "availabilityZone": zone,
            "bundleId": bundle_id,
            "sourceInstanceName": source,
        });
        for (member, value) in addressed.as_object().expect("the source names members") {
            body[member] = value.clone();
        }
        let restored_disk_name = format!("{restored_name}-data");
        if *with_mapping {
            let mut mapping = serde_json::Map::new();
            mapping.insert(
                (*restored_name).to_string(),
                json!([{
                    "originalDiskPath": attached_path,
                    "newDiskName": restored_disk_name,
                }]),
            );
            body["attachedDiskMapping"] = Value::Object(mapping);
        }
        emulator.ok("CreateInstancesFromSnapshot", body).await;
        let restored = instance(&emulator, restored_name).await;
        assert_eq!(
            restored["blueprintId"].as_str(),
            Some("amazon_linux_2023"),
            "case {served_case}: expected the copied instance's image, got {restored:?}"
        );
        if *with_mapping {
            let restored_disk = disk(&emulator, &restored_disk_name).await;
            assert_eq!(
                restored_disk["sizeInGb"].as_i64(),
                Some(attached_gb),
                "case {served_case}: expected the volume the copy captured, got {restored_disk:?}"
            );
        }
    }
}

/// What one refused row must be answered with.
#[derive(Debug)]
enum Refusal {
    /// Lightsail's `DoesNotExist`: HTTP 400, the modelled `NotFoundException`,
    /// body `code` `DoesNotExist` and exactly this message.
    DoesNotExist(&'static str),
    /// The modelled `InvalidInputException`, whose message names this member.
    Invalid(&'static str),
}

/// RULE: a restore is served only from a copy this account holds, of an instance
/// of the Region the new instances are created in, onto a plan whose SSD is at
/// least the system volume that copy was taken of; a request outside that is
/// refused and stores nothing.
#[tokio::test]
async fn a_restore_the_account_cannot_serve_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let zone = "us-east-1a";
    // One instance whose add-on has taken a copy, on a plan whose SSD sits above
    // the smallest the service publishes, and one that carries no add-on at all,
    // so a request addressing a copy of it addresses one of an instance that has
    // never been copied.
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["copied-instance"],
                "availabilityZone": zone,
                "blueprintId": "amazon_linux_2023",
                "bundleId": "medium_3_0",
                "addOns": [auto_snapshot_request(None)],
            }),
        )
        .await;
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["plain-instance"],
                "availabilityZone": zone,
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            }),
        )
        .await;
    let copy = wait_for_success(&emulator, "the fixture", "copied-instance").await;
    let date = copy["date"]
        .as_str()
        .expect("the copy carries a date")
        .to_string();

    // (case, the new instance's name, the zone, the plan, the source members, the refusal)
    let cases: &[(&str, &str, &str, &str, Value, Refusal)] = &[
        (
            "the latest restorable copy of an instance whose add-on has never been enabled",
            "latest-of-plain",
            zone,
            "nano_3_0",
            json!({"sourceInstanceName": "plain-instance", "useLatestRestorableAutoSnapshot": true}),
            Refusal::DoesNotExist(
                "The Instance plain-instance has no restorable automatic snapshot to restore",
            ),
        ),
        (
            "a dated copy of an instance whose add-on has never been enabled",
            "dated-of-plain",
            zone,
            "nano_3_0",
            json!({"sourceInstanceName": "plain-instance", "restoreDate": "2026-10-22"}),
            Refusal::DoesNotExist(
                "The automatic snapshot of the Instance plain-instance does not exist: 2026-10-22",
            ),
        ),
        (
            "an instance this account does not hold",
            "latest-of-absent",
            zone,
            "nano_3_0",
            json!({"sourceInstanceName": "absent-instance", "useLatestRestorableAutoSnapshot": true}),
            Refusal::DoesNotExist("The Instance does not exist: absent-instance"),
        ),
        (
            "a plan one size below the system volume the copy was taken of",
            "too-small",
            zone,
            "small_3_0",
            json!({"sourceInstanceName": "copied-instance", "useLatestRestorableAutoSnapshot": true}),
            Refusal::Invalid("bundleId"),
        ),
        (
            "the smallest plan the service publishes, well below that volume",
            "far-too-small",
            zone,
            "nano_3_0",
            json!({"sourceInstanceName": "copied-instance", "useLatestRestorableAutoSnapshot": true}),
            Refusal::Invalid("bundleId"),
        ),
        (
            "a zone of another Region",
            "cross-region",
            "eu-west-1a",
            "medium_3_0",
            json!({"sourceInstanceName": "copied-instance", "useLatestRestorableAutoSnapshot": true}),
            Refusal::Invalid("sourceInstanceName"),
        ),
        (
            "a zone of no Lightsail Region",
            "no-such-region",
            "xx-middle-9z",
            "medium_3_0",
            json!({"sourceInstanceName": "copied-instance", "useLatestRestorableAutoSnapshot": true}),
            Refusal::Invalid("availabilityZone"),
        ),
        (
            "a plan the service does not publish",
            "no-such-plan",
            zone,
            "gigantic_3_0",
            json!({"sourceInstanceName": "copied-instance", "useLatestRestorableAutoSnapshot": true}),
            Refusal::Invalid("bundleId"),
        ),
    ];

    for (case, restored_name, case_zone, bundle_id, source, refusal) in cases {
        let mut body = json!({
            "instanceNames": [restored_name],
            "availabilityZone": case_zone,
            "bundleId": bundle_id,
        });
        for (member, value) in source.as_object().expect("the source names members") {
            body[member] = value.clone();
        }
        let (status, answered) = emulator.call("CreateInstancesFromSnapshot", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {answered:?}"
        );
        match refusal {
            Refusal::DoesNotExist(message) => {
                assert_eq!(
                    (error_code(&answered).as_str(), answered["code"].as_str()),
                    ("NotFoundException", Some("DoesNotExist")),
                    "case {case}: expected the DoesNotExist envelope, got {answered:?}"
                );
                assert_eq!(
                    answered["message"].as_str(),
                    Some(*message),
                    "case {case}: expected the refusal to name what is missing, got {answered:?}"
                );
            }
            Refusal::Invalid(member) => {
                assert_eq!(
                    error_code(&answered),
                    "InvalidInputException",
                    "case {case}: expected the modelled InvalidInputException, got {answered:?}"
                );
                let message = answered["message"].as_str().unwrap_or_default().to_string();
                assert!(
                    message.contains(member),
                    "case {case}: expected the refusal to name {member}, got {message:?}"
                );
            }
        }
        let (read_status, read) = emulator
            .call("GetInstance", json!({"instanceName": restored_name}))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused restore stores nothing, got {read_status} {read:?}"
        );
    }

    // Only those requests were refused: the copy is restorable, which the same
    // request made rightly proves — and the copy and its instance are untouched.
    let copies_before = copies(&emulator, "copied-instance").await;
    emulator
        .ok(
            "CreateInstancesFromSnapshot",
            json!({
                "instanceNames": ["served-restore"],
                "availabilityZone": zone,
                "bundleId": "medium_3_0",
                "sourceInstanceName": "copied-instance",
                "restoreDate": date,
            }),
        )
        .await;
    let restored = instance(&emulator, "served-restore").await;
    assert_eq!(
        restored["bundleId"].as_str(),
        Some("medium_3_0"),
        "case the served restore: expected the requested plan, got {restored:?}"
    );
    let copies_after = copies(&emulator, "copied-instance").await;
    assert_eq!(
        copies_after, copies_before,
        "case the served restore: expected the copies to be untouched, got {copies_after:?}"
    );
}

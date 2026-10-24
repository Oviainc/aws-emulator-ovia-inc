// SPDX-License-Identifier: CC0-1.0

//! Restoring Lightsail instances from an instance snapshot, driven over the
//! wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `CreateInstancesFromSnapshot` persists ONE instance
//! record per name in `instanceNames` as a copy of the instance the ONE source
//! the request names captured — an instance snapshot this account holds
//! (`instanceSnapshotName`) or an automatic snapshot of an instance
//! (`sourceInstanceName` with `restoreDate` or
//! `useLatestRestorableAutoSnapshot`), the two being mutually exclusive — taking
//! the IMAGE from the snapshot record (this input models no blueprint) and the
//! plan, zone, key pair, launch script, IP address type, tags and add-ons from
//! the request verbatim, restoring one block storage disk per entry of
//! `attachedDiskMapping` at the device path the snapshot captured it at, and
//! computing every server-assigned member (ARN, support code, creation time,
//! addresses, hardware, firewall, state) from those plus the store's own
//! allocators; so a later `GetInstance` and `GetDisk` report exactly those
//! records, and the snapshot is left as it was. Each instance is created
//! `pending` and the clock, not a reader, settles it into `running`; and a
//! request outside what Lightsail restores (no source or both sources, an
//! automatic-snapshot member without that source, a snapshot of another region, a
//! snapshot Lightsail is still taking, a bundle smaller than the snapshotted
//! system volume, a name the account already holds, a disk mapping that names an
//! instance this request does not create, an entry missing a member, more entries
//! than this emulator holds disks, or a device path the snapshot never captured)
//! is refused and stores nothing.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The emulator's own per-request and per-account bounds, read from the crate
// rather than re-declared here, so the at-bound/past-bound pairs below stay
// pinned to the implementation if a bound is ever retuned.
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    MAX_DISKS, MAX_INSTANCE_NAMES_PER_REQUEST,
};

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::disk_attachment::wait_for_instance_state;
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The device path an instance's system volume is exposed at
/// (`crate::state::SYSTEM_DISK_PATH`).
const SYSTEM_DISK_PATH: &str = "/dev/xvda";

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

/// The snapshot `name`, as the account's snapshot listing reports it, which must
/// report it exactly once.
async fn snapshot(emulator: &Emulator, name: &str) -> Value {
    let body = emulator.ok("GetInstanceSnapshots", json!({})).await;
    let listed: Vec<&Value> = body["instanceSnapshots"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading snapshot {name}: expected an array, got {body:?}"))
        .iter()
        .filter(|entry| entry["name"].as_str() == Some(name))
        .collect();
    assert_eq!(
        listed.len(),
        1,
        "case reading snapshot {name}: expected exactly one entry, got {listed:?}"
    );
    listed[0].clone()
}

/// Poll the snapshot listing until `name` reports `want`, answering it.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_snapshot_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let found = snapshot(emulator, name).await;
        if found["state"].as_str() == Some(want) {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for snapshot '{want}' on {name}: expected it within 5s, got {:?}",
            found["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The published plan `bundle_id`, as the service's own `GetBundles` catalog
/// reports it.
///
/// Every expectation about a restored instance's hardware is read back out of
/// that catalog rather than copied into this suite, so the plan's SSD size and
/// data transfer are the service's own account of them.
async fn bundle(emulator: &Emulator, bundle_id: &str) -> Value {
    let body = emulator.ok("GetBundles", json!({})).await;
    body["bundles"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading bundle {bundle_id}: expected an array, got {body:?}"))
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

/// Create the disk `name` of `size_in_gb` in `zone` and wait until it is
/// `available`, so it can be attached.
async fn create_available_disk(emulator: &Emulator, name: &str, zone: &str, size_in_gb: i64) {
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": name, "availabilityZone": zone, "sizeInGb": size_in_gb}),
        )
        .await;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let found = disk(emulator, name).await;
        if found["state"].as_str() == Some("available") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for disk {name} to be available: got {:?}",
            found["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The restore table
// ---------------------------------------------------------------------------

/// One `CreateInstancesFromSnapshot` request the service must serve, with the
/// source instance and snapshot it restores from.
///
/// The rows differ along everything the rule generalises over: the snapshotted
/// IMAGE (a Linux blueprint, an application blueprint, a Windows one) and so the
/// user name and the default firewall the restore inherits, the plan the source
/// ran on and the plan it is restored onto (the same one, a larger one, a much
/// larger one), the zone and region the snapshot lives in and the zone of that
/// region it is restored into, HOW MANY instances one call creates (one, two,
/// three), how many block storage disks the snapshot captured and how many of
/// them the mapping restores (none, one, both), the number of tags, the IP
/// address type, whether a key pair and a launch script are named, whether an
/// add-on is asked for, and the form of the new names (hyphens, underscores,
/// digits, mixed case).
struct Row {
    case: &'static str,
    source_name: &'static str,
    source_zone: &'static str,
    /// A blueprint id this service publishes (`GetBlueprints`).
    blueprint_id: &'static str,
    /// A bundle id this service publishes (`GetBundles`).
    source_bundle_id: &'static str,
    snapshot_name: &'static str,
    /// The block storage disks attached to the source before the snapshot is
    /// taken: name, device path, size in GB.
    source_disks: &'static [(&'static str, &'static str, i64)],
    /// The zone the instances are restored into — in the snapshot's own region.
    restore_zone: &'static str,
    region: &'static str,
    /// The plan the restore creates the instances on, which the request names
    /// and the snapshot does not.
    restore_bundle_id: &'static str,
    restored_names: &'static [&'static str],
    /// `attachedDiskMapping` entries: the index into `restored_names` of the
    /// instance they belong to, the captured device path, and the name to
    /// restore that disk under.
    mapping: &'static [(usize, &'static str, &'static str)],
    tags: &'static [(&'static str, &'static str)],
    ip_address_type: Option<&'static str>,
    key_pair_name: Option<&'static str>,
    user_data: Option<&'static str>,
    /// `Some(time)` asks for the `AutoSnapshot` add-on with that
    /// `snapshotTimeOfDay`; `Some("")` asks for it without one, so the service
    /// picks the hour; `None` asks for no add-on at all.
    auto_snapshot_at: Option<&'static str>,
}

/// The table. The modelled `bundleId` documentation's own example plan
/// (`micro_x_x`) is the first row's restore plan.
const ROWS: &[Row] = &[
    Row {
        case: "a Linux instance restored onto a larger plan in its own zone",
        source_name: "linux-source",
        source_zone: "us-east-2a",
        blueprint_id: "amazon_linux_2023",
        source_bundle_id: "nano_3_0",
        snapshot_name: "linux-source-snapshot",
        source_disks: &[],
        restore_zone: "us-east-2a",
        region: "us-east-2",
        restore_bundle_id: "micro_3_0",
        restored_names: &["restored-linux"],
        mapping: &[],
        tags: &[],
        ip_address_type: None,
        key_pair_name: None,
        user_data: None,
        auto_snapshot_at: None,
    },
    Row {
        case: "two instances from one snapshot, into another zone of the same region",
        source_name: "web_source_1",
        source_zone: "eu-west-1c",
        blueprint_id: "wordpress",
        source_bundle_id: "small_3_0",
        snapshot_name: "web_source_1_snapshot",
        source_disks: &[],
        restore_zone: "eu-west-1a",
        region: "eu-west-1",
        restore_bundle_id: "small_3_0",
        restored_names: &["web_restore_1", "web_restore_2"],
        mapping: &[],
        tags: &[("tier", "edge")],
        ip_address_type: Some("ipv4"),
        key_pair_name: Some("fleet-key"),
        user_data: Some("apt-get -y update"),
        auto_snapshot_at: Some("06:00"),
    },
    Row {
        case: "both captured disks restored onto one instance, two tags, an add-on",
        source_name: "DataSource3",
        source_zone: "ap-southeast-2b",
        blueprint_id: "debian_12",
        source_bundle_id: "medium_3_0",
        snapshot_name: "DataSource3-Snapshot",
        source_disks: &[("source-data-a", "/dev/xvdf", 32), ("source-data-b", "/dev/xvdg", 64)],
        restore_zone: "ap-southeast-2b",
        region: "ap-southeast-2",
        restore_bundle_id: "medium_3_0",
        restored_names: &["DataRestore3"],
        mapping: &[
            (0, "/dev/xvdf", "restored-data-a"),
            (0, "/dev/xvdg", "restored_data_b"),
        ],
        tags: &[("stack", "prod"), ("owner", "storage-team")],
        ip_address_type: Some("dualstack"),
        key_pair_name: None,
        user_data: None,
        auto_snapshot_at: Some(""),
    },
    Row {
        case: "a Windows instance restored onto a much larger plan, one of two disks mapped",
        source_name: "windows-source",
        source_zone: "ca-central-1a",
        blueprint_id: "windows_server_2022",
        source_bundle_id: "micro_3_0",
        snapshot_name: "windows_source_snapshot_1",
        source_disks: &[("win-data-a", "/dev/xvdh", 100), ("win-data-b", "/dev/xvdi", 8)],
        restore_zone: "ca-central-1a",
        region: "ca-central-1",
        restore_bundle_id: "2xlarge_3_0",
        restored_names: &["windows-restore-1"],
        mapping: &[(0, "/dev/xvdh", "win-restore-a")],
        tags: &[("os", "windows")],
        ip_address_type: Some("ipv6"),
        key_pair_name: Some("windows_key_1"),
        user_data: None,
        auto_snapshot_at: None,
    },
    Row {
        case: "three instances, each taking its own copy of the one captured disk",
        source_name: "fan-out-source",
        source_zone: "us-west-2a",
        blueprint_id: "nodejs",
        source_bundle_id: "nano_3_0",
        snapshot_name: "fan-out-snapshot",
        source_disks: &[("fan-out-data", "/dev/xvdf", 16)],
        restore_zone: "us-west-2b",
        region: "us-west-2",
        restore_bundle_id: "large_3_0",
        restored_names: &["fan-out-1", "fan-out-2", "fan-out-3"],
        mapping: &[
            (0, "/dev/xvdf", "fan-out-data-1"),
            (1, "/dev/xvdf", "fan-out-data-2"),
            (2, "/dev/xvdf", "fan-out-data-3"),
        ],
        tags: &[],
        ip_address_type: None,
        key_pair_name: None,
        user_data: Some("#!/bin/bash\nyum -y update"),
        auto_snapshot_at: None,
    },
];

/// Stand up `row`'s source instance, attach every disk the row names, and take
/// the snapshot the restore reads — answering once the snapshot is `available`.
async fn create_source_snapshot(emulator: &Emulator, row: &Row) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [row.source_name],
                "availabilityZone": row.source_zone,
                "blueprintId": row.blueprint_id,
                "bundleId": row.source_bundle_id,
            }),
        )
        .await;
    wait_for_instance_state(emulator, row.source_name, "running").await;
    for (disk_name, disk_path, size_in_gb) in row.source_disks {
        create_available_disk(emulator, disk_name, row.source_zone, *size_in_gb).await;
        emulator
            .ok(
                "AttachDisk",
                json!({
                    "diskName": disk_name,
                    "instanceName": row.source_name,
                    "diskPath": disk_path,
                }),
            )
            .await;
    }
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({
                "instanceName": row.source_name,
                "instanceSnapshotName": row.snapshot_name,
            }),
        )
        .await;
    wait_for_snapshot_state(emulator, row.snapshot_name, "available").await;
}

/// The `CreateInstancesFromSnapshot` body `row` describes.
fn restore_body(row: &Row) -> Value {
    let mut body = json!({
        "instanceNames": row.restored_names,
        "instanceSnapshotName": row.snapshot_name,
        "availabilityZone": row.restore_zone,
        "bundleId": row.restore_bundle_id,
    });
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
    if let Some(key_pair_name) = row.key_pair_name {
        body["keyPairName"] = json!(key_pair_name);
    }
    if let Some(user_data) = row.user_data {
        body["userData"] = json!(user_data);
    }
    match row.auto_snapshot_at {
        None => {}
        Some("") => body["addOns"] = json!([{"addOnType": "AutoSnapshot"}]),
        Some(at) => {
            body["addOns"] = json!([{
                "addOnType": "AutoSnapshot",
                "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": at},
            }]);
        }
    }
    body
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: every member `GetInstance` and `GetDisk` report of a restored instance
/// comes from the ONE record this call persisted — the snapshot's image, the
/// request's plan, zone, key pair, launch script, IP address type, tags and
/// add-ons verbatim, one restored disk per mapping entry sized by the volume the
/// snapshot captured, and every server-assigned member computed from those plus
/// the store's allocators — and the snapshot it was restored from is left
/// exactly as it was.
#[tokio::test]
async fn restored_instances_read_back_as_they_were_requested() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();
    let mut addresses: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        create_source_snapshot(&emulator, row).await;
        let snapshot_before = snapshot(&emulator, row.snapshot_name).await;
        let plan = bundle(&emulator, row.restore_bundle_id).await;
        let system_disk_size = plan["diskSizeInGb"].as_i64().unwrap_or_default();

        // One operation-log entry per instance the call creates, none of them
        // terminal yet: the instances are still coming up when the call returns.
        let answered = emulator
            .ok("CreateInstancesFromSnapshot", restore_body(row))
            .await;

        // A restored instance is not serving yet: the clock, not a reader,
        // settles it, so a caller reading straight back still sees `pending`.
        // Read first, before the rest of the assertions spend that span.
        for name in row.restored_names {
            let fresh = instance(&emulator, name).await;
            assert_eq!(
                fresh["state"]["name"].as_str(),
                Some("pending"),
                "case {case}: expected a pending instance, got {:?}",
                fresh["state"]
            );
        }

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
                operation["operationType"].as_str(),
                Some("CreateInstancesFromSnapshot"),
                "case {case}: expected the CreateInstancesFromSnapshot operation type, got \
                 {operation:?}"
            );
            assert_eq!(
                operation["resourceType"].as_str(),
                Some("Instance"),
                "case {case}: expected the Instance resource type, got {operation:?}"
            );
            assert_eq!(
                operation["resourceName"].as_str(),
                Some(*name),
                "case {case}: expected the operation to name the new instance, got {operation:?}"
            );
            assert_eq!(
                operation["status"].as_str(),
                Some("Started"),
                "case {case}: expected a Started operation, got {operation:?}"
            );
            assert_eq!(
                operation["isTerminal"].as_bool(),
                Some(false),
                "case {case}: expected a non-terminal operation, got {operation:?}"
            );
            assert_eq!(
                operation["location"]["availabilityZone"].as_str(),
                Some(row.restore_zone),
                "case {case}: expected the operation to be located where the instance was \
                 restored, got {operation:?}"
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
            assert_eq!(
                restored["name"].as_str(),
                Some(*name),
                "case {case}: expected the requested name, got {restored:?}"
            );
            assert_eq!(
                restored["resourceType"].as_str(),
                Some("Instance"),
                "case {case}: expected the Instance resource type, got {restored:?}"
            );
            // The IMAGE is the snapshot's: this input models no blueprint.
            assert_eq!(
                restored["blueprintId"].as_str(),
                Some(row.blueprint_id),
                "case {case}: expected the snapshotted blueprint, got {restored:?}"
            );
            // The PLAN is the request's.
            assert_eq!(
                restored["bundleId"].as_str(),
                Some(row.restore_bundle_id),
                "case {case}: expected the requested bundle, got {restored:?}"
            );
            assert_eq!(
                restored["location"]["availabilityZone"].as_str(),
                Some(row.restore_zone),
                "case {case}: expected the requested zone, got {:?}",
                restored["location"]
            );
            assert_eq!(
                restored["location"]["regionName"].as_str(),
                Some(row.region),
                "case {case}: expected the zone's region, got {:?}",
                restored["location"]
            );
            assert_eq!(
                restored["ipAddressType"].as_str(),
                Some(row.ip_address_type.unwrap_or("dualstack")),
                "case {case}: expected the requested IP address type, got {restored:?}"
            );
            assert_eq!(
                restored["sshKeyName"].as_str(),
                Some(row.key_pair_name.unwrap_or("LightsailDefaultKeyPair")),
                "case {case}: expected the requested key pair, got {restored:?}"
            );
            assert_eq!(
                restored["isStaticIp"].as_bool(),
                Some(false),
                "case {case}: expected no static IP on a fresh instance, got {restored:?}"
            );
            assert_eq!(
                restored["tags"],
                tag_list(row.tags),
                "case {case}: expected the requested tags, got {:?}",
                restored["tags"]
            );
            match row.auto_snapshot_at {
                None => assert_eq!(
                    restored["addOns"].as_array().map(Vec::len),
                    Some(0),
                    "case {case}: expected no add-ons, got {:?}",
                    restored["addOns"]
                ),
                Some(at) => {
                    let add_on = restored["addOns"][0].clone();
                    assert_eq!(
                        add_on["name"].as_str(),
                        Some("AutoSnapshot"),
                        "case {case}: expected the AutoSnapshot add-on, got {add_on:?}"
                    );
                    assert_eq!(
                        add_on["status"].as_str(),
                        Some("Enabled"),
                        "case {case}: expected the add-on enabled, got {add_on:?}"
                    );
                    let hour = add_on["snapshotTimeOfDay"].as_str().unwrap_or_default();
                    if at.is_empty() {
                        assert!(
                            hour.len() == 5 && hour.ends_with(":00"),
                            "case {case}: expected a service-picked hour, got {hour:?}"
                        );
                    } else {
                        assert_eq!(
                            hour, at,
                            "case {case}: expected the requested hour, got {hour:?}"
                        );
                    }
                }
            }
            // The plan's own hardware, from the catalog the service publishes.
            assert_eq!(
                restored["hardware"]["cpuCount"], plan["cpuCount"],
                "case {case}: expected the plan's vCPU count, got {:?}",
                restored["hardware"]
            );
            assert_eq!(
                restored["networking"]["monthlyTransfer"]["gbPerMonthAllocated"],
                plan["transferPerMonthInGb"],
                "case {case}: expected the plan's monthly transfer, got {:?}",
                restored["networking"]
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
                disks[0]["isSystemDisk"].as_bool(),
                Some(true),
                "case {case}: expected the system disk first, got {:?}",
                disks[0]
            );
            assert_eq!(
                disks[0]["path"].as_str(),
                Some(SYSTEM_DISK_PATH),
                "case {case}: expected the system disk at the root device, got {:?}",
                disks[0]
            );
            assert_eq!(
                disks[0]["sizeInGb"].as_i64(),
                Some(system_disk_size),
                "case {case}: expected the restore plan's SSD size, got {:?}",
                disks[0]
            );

            for (position, (_, original_path, new_disk_name)) in mapped.iter().enumerate() {
                // The size the snapshot captured for that device path, read off
                // the source disk the service itself stored.
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
                    restored_disk["sizeInGb"].as_i64(),
                    Some(captured_size),
                    "case {case}: expected the captured volume's size, got {restored_disk:?}"
                );
                assert_eq!(
                    restored_disk["path"].as_str(),
                    Some(*original_path),
                    "case {case}: expected the captured device path, got {restored_disk:?}"
                );
                assert_eq!(
                    restored_disk["attachedTo"].as_str(),
                    Some(*name),
                    "case {case}: expected the disk attached to the new instance, got \
                     {restored_disk:?}"
                );
                assert_eq!(
                    restored_disk["isAttached"].as_bool(),
                    Some(true),
                    "case {case}: expected an attached disk, got {restored_disk:?}"
                );
                assert_eq!(
                    restored_disk["state"].as_str(),
                    Some("in-use"),
                    "case {case}: expected an in-use disk, got {restored_disk:?}"
                );
                assert_eq!(
                    restored_disk["location"]["availabilityZone"].as_str(),
                    Some(row.restore_zone),
                    "case {case}: expected the disk where the instance was restored, got \
                     {restored_disk:?}"
                );
                assert_eq!(
                    restored_disk["tags"],
                    tag_list(row.tags),
                    "case {case}: expected the requested tags on the restored disk, got {:?}",
                    restored_disk["tags"]
                );
                let disk_arn = restored_disk["arn"].as_str().unwrap_or_default();
                let want_disk_prefix =
                    format!("arn:aws:lightsail:{}:{ACCOUNT}:Disk/", row.region);
                assert!(
                    disk_arn.starts_with(&want_disk_prefix)
                        && is_uuid_shaped(&disk_arn[want_disk_prefix.len()..]),
                    "case {case}: expected a disk ARN of the restore region, got {disk_arn:?}"
                );
                assert!(
                    arns.insert(disk_arn.to_string()),
                    "case {case}: expected a fresh disk ARN, got the already-used {disk_arn:?}"
                );
                // The mapping's order is the order the disks are reported in
                // after the system disk.
                assert_eq!(
                    disks[1 + position]["name"].as_str(),
                    Some(*new_disk_name),
                    "case {case}: expected the mapped disks in mapping order, got {disks:?}"
                );
            }

            // Server-assigned identity: an ARN of the restore region carrying a
            // minted id, a support code, and addresses nothing else was given.
            let arn = restored["arn"].as_str().unwrap_or_default();
            let want_prefix = format!("arn:aws:lightsail:{}:{ACCOUNT}:Instance/", row.region);
            assert!(
                arn.starts_with(&want_prefix) && is_uuid_shaped(&arn[want_prefix.len()..]),
                "case {case}: expected an instance ARN of the restore region, got {arn:?}"
            );
            assert!(
                arns.insert(arn.to_string()),
                "case {case}: expected a fresh instance ARN, got the already-used {arn:?}"
            );
            assert!(
                restored["supportCode"]
                    .as_str()
                    .is_some_and(|code| code.contains(ACCOUNT)),
                "case {case}: expected a support code carrying the account, got {restored:?}"
            );
            for member in ["privateIpAddress", "publicIpAddress"] {
                if let Some(address) = restored[member].as_str() {
                    assert!(
                        addresses.insert(address.to_string()),
                        "case {case}: expected a fresh {member}, got the already-used \
                         {address:?}"
                    );
                }
            }

            // A restored instance is not serving yet: the clock settles it,
            // which the pending read above pinned.
            wait_for_instance_state(&emulator, name, "running").await;
        }

        // A restore reads the snapshot and writes nothing to it.
        let snapshot_after = snapshot(&emulator, row.snapshot_name).await;
        assert_eq!(
            snapshot_after, snapshot_before,
            "case {case}: expected the snapshot untouched by the restore, got {snapshot_after:?}"
        );
    }
}

/// RULE: an IP address type gives an instance exactly the addresses of that
/// family, so a restored instance reports the IPv4 addresses only when its type
/// includes IPv4 and an IPv6 address only when it includes IPv6.
#[tokio::test]
async fn a_restored_instance_carries_the_addresses_of_its_ip_address_type() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_source_snapshot(&emulator, row).await;

    // (requested type, wants IPv4, wants IPv6) — the modelled `IpAddressType`
    // enum's three members, plus the absent member's documented default
    // ("The default value is dualstack").
    let cases: &[(Option<&str>, bool, bool)] = &[
        (Some("ipv4"), true, false),
        (Some("ipv6"), false, true),
        (Some("dualstack"), true, true),
        (None, true, true),
    ];
    for (index, (requested, wants_ipv4, wants_ipv6)) in cases.iter().enumerate() {
        let name = format!("addressed-{index}");
        let mut body = json!({
            "instanceNames": [&name],
            "instanceSnapshotName": row.snapshot_name,
            "availabilityZone": row.restore_zone,
            "bundleId": row.restore_bundle_id,
        });
        if let Some(requested) = requested {
            body["ipAddressType"] = json!(requested);
        }
        emulator.ok("CreateInstancesFromSnapshot", body).await;
        let restored = instance(&emulator, &name).await;
        assert_eq!(
            restored["privateIpAddress"].is_string(),
            *wants_ipv4,
            "case {requested:?}: expected privateIpAddress presence {wants_ipv4}, got \
             {restored:?}"
        );
        assert_eq!(
            restored["publicIpAddress"].is_string(),
            *wants_ipv4,
            "case {requested:?}: expected publicIpAddress presence {wants_ipv4}, got \
             {restored:?}"
        );
        assert_eq!(
            restored["ipv6Addresses"]
                .as_array()
                .is_some_and(|addresses| !addresses.is_empty()),
            *wants_ipv6,
            "case {requested:?}: expected ipv6Addresses presence {wants_ipv6}, got {restored:?}"
        );
    }
}

/// RULE: a restored system volume holds every block the snapshot captured, so
/// the plan it is restored onto must provide an SSD at least that large — each
/// published plan from the snapshotted one up is accepted and every smaller one
/// is refused, and a refusal creates nothing.
#[tokio::test]
async fn a_restore_needs_a_plan_that_holds_the_snapshotted_volume() {
    let emulator = Emulator::new();
    // The source runs on a mid-range plan, so published plans sit on both sides
    // of the snapshotted volume.
    let row = &ROWS[2];
    create_source_snapshot(&emulator, row).await;
    let snapshotted = snapshot(&emulator, row.snapshot_name).await["sizeInGb"]
        .as_i64()
        .unwrap_or_default();
    assert!(
        snapshotted > 0,
        "case setup: expected the snapshot to report the captured volume's size"
    );

    // Every plan size the service publishes, read off its own catalog: the
    // bound is checked on each one, not on a hand-picked pair.
    let catalog = emulator.ok("GetBundles", json!({})).await["bundles"]
        .as_array()
        .expect("the bundle catalog is an array")
        .clone();
    assert!(
        catalog.len() >= 4,
        "case setup: expected a published plan catalog, got {catalog:?}"
    );
    let mut refused = 0usize;
    let mut accepted = 0usize;
    for (index, plan) in catalog.iter().enumerate() {
        let bundle_id = plan["bundleId"].as_str().unwrap_or_default();
        let disk_size = plan["diskSizeInGb"].as_i64().unwrap_or_default();
        let name = format!("sized-restore-{index}");
        let body = json!({
            "instanceNames": [&name],
            "instanceSnapshotName": row.snapshot_name,
            "availabilityZone": row.restore_zone,
            "bundleId": bundle_id,
        });
        if disk_size >= snapshotted {
            emulator.ok("CreateInstancesFromSnapshot", body).await;
            let restored = instance(&emulator, &name).await;
            assert_eq!(
                restored["hardware"]["disks"][0]["sizeInGb"].as_i64(),
                Some(disk_size),
                "case {bundle_id}: expected the plan's SSD size, got {restored:?}"
            );
            accepted += 1;
        } else {
            refused_as_invalid(&emulator, bundle_id, body, "bundleId").await;
            let (status, read) = emulator
                .call("GetInstance", json!({"instanceName": &name}))
                .await;
            assert_eq!(
                status, 400,
                "case {bundle_id}: a refused restore stores nothing, got {status} {read:?}"
            );
            refused += 1;
        }
    }
    assert!(
        refused > 0 && accepted > 0,
        "case the published catalog: expected plans on both sides of the snapshotted volume, \
         got {refused} refused and {accepted} accepted"
    );
}

/// RULE: exactly one source is restored from — `instanceSnapshotName` and
/// `sourceInstanceName` are mutually exclusive, and the automatic-snapshot
/// members (`restoreDate`, `useLatestRestorableAutoSnapshot`, themselves mutually
/// exclusive) belong only with the latter — so every other combination is refused
/// and stores nothing.
#[tokio::test]
async fn exactly_one_source_is_restored_from() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_source_snapshot(&emulator, row).await;

    // (case, the members under test, the member the refusal must name)
    let cases: &[(&str, Value, &str)] = &[
        ("no source at all", json!({}), "instanceSnapshotName"),
        (
            "both sources",
            json!({
                "instanceSnapshotName": row.snapshot_name,
                "sourceInstanceName": row.source_name,
            }),
            "instanceSnapshotName",
        ),
        (
            "a manual snapshot with a restore date",
            json!({"instanceSnapshotName": row.snapshot_name, "restoreDate": "2026-10-24"}),
            "restoreDate",
        ),
        (
            "a manual snapshot with the latest-restorable flag",
            json!({
                "instanceSnapshotName": row.snapshot_name,
                "useLatestRestorableAutoSnapshot": true,
            }),
            "restoreDate",
        ),
        (
            "a source instance with neither automatic-snapshot member",
            json!({"sourceInstanceName": row.source_name}),
            "sourceInstanceName",
        ),
        (
            "a source instance with both automatic-snapshot members",
            json!({
                "sourceInstanceName": row.source_name,
                "restoreDate": "2026-10-24",
                "useLatestRestorableAutoSnapshot": true,
            }),
            "restoreDate",
        ),
        (
            "a restore date that is not a date",
            json!({"sourceInstanceName": row.source_name, "restoreDate": "24-10-2026"}),
            "restoreDate",
        ),
        (
            "a restore date with a time on it",
            json!({
                "sourceInstanceName": row.source_name,
                "restoreDate": "2026-10-24T01:05:05Z",
            }),
            "restoreDate",
        ),
    ];
    for (index, (case, members, names_member)) in cases.iter().enumerate() {
        let name = format!("sourced-{index}");
        let mut body = json!({
            "instanceNames": [&name],
            "availabilityZone": row.restore_zone,
            "bundleId": row.restore_bundle_id,
        });
        for (member, value) in members.as_object().expect("the case members are an object") {
            body[member] = value.clone();
        }
        refused_as_invalid(&emulator, case, body, names_member).await;
        let (status, read) = emulator
            .call("GetInstance", json!({"instanceName": &name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a refused restore stores nothing, got {status} {read:?}"
        );
    }
}

/// RULE: the source must be a resource this account holds, so a well-formed name
/// that names none is refused with the service's `DoesNotExist` envelope — naming
/// the resource TYPE the request pointed at — and stores nothing; and this store
/// takes no automatic snapshots, so a source instance it DOES hold has no
/// automatic snapshot to restore either.
#[tokio::test]
async fn a_source_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_source_snapshot(&emulator, row).await;

    // (case, the source members, the message the refusal must carry)
    let cases: &[(&str, Value, Option<String>)] = &[
        (
            "a snapshot name nothing holds",
            json!({"instanceSnapshotName": "absent-snapshot"}),
            Some("The InstanceSnapshot does not exist: absent-snapshot".to_string()),
        ),
        (
            // An instance is not an instance snapshot, so the snapshot member
            // finds nothing under its name.
            "the source INSTANCE's name as a snapshot name",
            json!({"instanceSnapshotName": row.source_name}),
            Some(format!(
                "The InstanceSnapshot does not exist: {}",
                row.source_name
            )),
        ),
        (
            "an automatic snapshot of an instance nothing holds",
            json!({
                "sourceInstanceName": "absent-instance",
                "useLatestRestorableAutoSnapshot": true,
            }),
            Some("The Instance does not exist: absent-instance".to_string()),
        ),
        (
            "the latest automatic snapshot of an instance that exists",
            json!({"sourceInstanceName": row.source_name, "useLatestRestorableAutoSnapshot": true}),
            None,
        ),
        (
            "a dated automatic snapshot of an instance that exists",
            json!({"sourceInstanceName": row.source_name, "restoreDate": "2026-10-24"}),
            None,
        ),
    ];
    for (index, (case, members, want_message)) in cases.iter().enumerate() {
        let name = format!("absent-source-{index}");
        let mut body = json!({
            "instanceNames": [&name],
            "availabilityZone": row.restore_zone,
            "bundleId": row.restore_bundle_id,
        });
        for (member, value) in members.as_object().expect("the case members are an object") {
            body[member] = value.clone();
        }
        let (status, answered) = emulator.call("CreateInstancesFromSnapshot", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "NotFoundException",
            "case {case}: expected the modelled NotFoundException, got {answered:?}"
        );
        assert_eq!(
            answered["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected code DoesNotExist, got {answered:?}"
        );
        if let Some(message) = want_message {
            assert_eq!(
                answered["message"].as_str(),
                Some(message.as_str()),
                "case {case}: expected the resource-type/name message, got {answered:?}"
            );
        }
        let (read_status, read) = emulator
            .call("GetInstance", json!({"instanceName": &name}))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused restore stores nothing, got {read_status} {read:?}"
        );
    }
}

/// RULE: a snapshot belongs to the region it was taken in, so it restores only
/// into a zone of that region; a zone of another region — or of no Lightsail
/// region at all — is refused and stores nothing.
#[tokio::test]
async fn a_snapshot_restores_only_into_its_own_region() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_source_snapshot(&emulator, row).await;

    for (index, zone) in ["eu-west-1a", "ap-southeast-2b", "xx-middle-9z"]
        .iter()
        .enumerate()
    {
        let name = format!("cross-region-{index}");
        refused_as_invalid(
            &emulator,
            zone,
            json!({
                "instanceNames": [&name],
                "instanceSnapshotName": row.snapshot_name,
                "availabilityZone": zone,
                "bundleId": row.restore_bundle_id,
            }),
            "availabilityZone",
        )
        .await;
        let (status, read) = emulator
            .call("GetInstance", json!({"instanceName": &name}))
            .await;
        assert_eq!(
            status, 400,
            "case {zone}: a refused restore stores nothing, got {status} {read:?}"
        );
    }
}

/// RULE: `attachedDiskMapping` maps disks the snapshot captured onto instances
/// this request creates, so an entry that names anything else — an instance the
/// request does not create, a device path the snapshot never captured, a disk
/// name already taken or named twice, a path named twice, or an entry missing
/// either member — is refused, and a refused request creates no instance and no
/// disk.
#[tokio::test]
async fn a_disk_mapping_names_a_captured_path_and_a_created_instance() {
    let emulator = Emulator::new();
    // The source carries two captured disks, at `/dev/xvdf` and `/dev/xvdg`.
    let row = &ROWS[2];
    create_source_snapshot(&emulator, row).await;

    // (case, the instanceNames of the request, the mapping, the member the
    // refusal must name)
    let cases: &[(&str, &[&str], Value, &str)] = &[
        (
            "a key naming an instance the request does not create",
            &["mapped-a"],
            json!({"someone-else": [{"originalDiskPath": "/dev/xvdf", "newDiskName": "map-a"}]}),
            "attachedDiskMapping",
        ),
        (
            "an entry without a new disk name",
            &["mapped-b"],
            json!({"mapped-b": [{"originalDiskPath": "/dev/xvdf"}]}),
            "newDiskName",
        ),
        (
            "an entry without an original disk path",
            &["mapped-c"],
            json!({"mapped-c": [{"newDiskName": "map-c"}]}),
            "originalDiskPath",
        ),
        (
            "a device path the snapshot never captured",
            &["mapped-d"],
            json!({"mapped-d": [{"originalDiskPath": "/dev/xvdz", "newDiskName": "map-d"}]}),
            "originalDiskPath",
        ),
        (
            "the snapshotted instance's own system volume",
            &["mapped-e"],
            json!({"mapped-e": [{"originalDiskPath": "/dev/xvda", "newDiskName": "map-e"}]}),
            "originalDiskPath",
        ),
        (
            "one device path twice on one instance",
            &["mapped-f"],
            json!({"mapped-f": [
                {"originalDiskPath": "/dev/xvdf", "newDiskName": "map-f-1"},
                {"originalDiskPath": "/dev/xvdf", "newDiskName": "map-f-2"},
            ]}),
            "originalDiskPath",
        ),
        (
            "one new disk name on two instances",
            &["mapped-g-1", "mapped-g-2"],
            json!({
                "mapped-g-1": [{"originalDiskPath": "/dev/xvdf", "newDiskName": "map-g"}],
                "mapped-g-2": [{"originalDiskPath": "/dev/xvdf", "newDiskName": "map-g"}],
            }),
            "newDiskName",
        ),
        (
            "a new disk name the account already holds",
            &["mapped-h"],
            json!({
                "mapped-h": [{
                    "originalDiskPath": "/dev/xvdf",
                    "newDiskName": row.source_disks[0].0,
                }],
            }),
            "already exists",
        ),
    ];
    for (case, instance_names, mapping, names_member) in cases {
        refused_as_invalid(
            &emulator,
            case,
            json!({
                "instanceNames": instance_names,
                "instanceSnapshotName": row.snapshot_name,
                "availabilityZone": row.restore_zone,
                "bundleId": row.restore_bundle_id,
                "attachedDiskMapping": mapping,
            }),
            names_member,
        )
        .await;
        for name in *instance_names {
            let (status, read) = emulator
                .call("GetInstance", json!({"instanceName": name}))
                .await;
            assert_eq!(
                status, 400,
                "case {case}: a refused restore creates no instance, got {status} {read:?}"
            );
        }
    }
    // Nor any disk: the one captured disk's name still addresses the SOURCE
    // disk, and no `map-*` name was created.
    for name in ["map-a", "map-c", "map-d", "map-e", "map-f-1", "map-f-2", "map-g"] {
        let (status, read) = emulator.call("GetDisk", json!({"diskName": name})).await;
        assert_eq!(
            status, 400,
            "case a refused mapping: expected no disk named {name}, got {status} {read:?}"
        );
    }
    let source_disk = disk(&emulator, row.source_disks[0].0).await;
    assert_eq!(
        source_disk["attachedTo"].as_str(),
        Some(row.source_name),
        "case a refused mapping: expected the source disk untouched, got {source_disk:?}"
    );

    // The mapping is caller-sized, so it is bounded before any entry is planned:
    // a request cannot restore more disks than this emulator holds
    // (`crate::state::MAX_DISKS`). At the bound the planner runs and the request
    // is refused for the paths the snapshot never captured; one entry past it is
    // refused for the bound itself, before any entry is looked at.
    for (case, count, names_member) in [
        (
            "the most mapping entries one request carries",
            MAX_DISKS,
            "originalDiskPath",
        ),
        ("one entry past that", MAX_DISKS + 1, "attachedDiskMapping"),
    ] {
        let entries: Vec<Value> = (0..count)
            .map(|index| {
                json!({
                    "originalDiskPath": format!("/dev/xvd-{index}"),
                    "newDiskName": format!("bulk-mapped-{index}"),
                })
            })
            .collect();
        let body = json!({
            "instanceNames": ["bulk-mapped"],
            "instanceSnapshotName": row.snapshot_name,
            "availabilityZone": row.restore_zone,
            "bundleId": row.restore_bundle_id,
            "attachedDiskMapping": {"bulk-mapped": entries},
        });
        refused_as_invalid(&emulator, case, body, names_member).await;
        let (status, read) = emulator
            .call("GetInstance", json!({"instanceName": "bulk-mapped"}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a refused restore creates nothing, got {status} {read:?}"
        );
    }
}

/// RULE: a snapshot is restorable only once every block it captures is
/// captured, so a restore from a snapshot Lightsail is still taking is refused
/// with the modelled `OperationFailureException` and creates nothing; once the
/// clock settles the snapshot, the same request is served.
#[tokio::test]
async fn a_snapshot_still_being_taken_cannot_be_restored() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [row.source_name],
                "availabilityZone": row.source_zone,
                "blueprintId": row.blueprint_id,
                "bundleId": row.source_bundle_id,
            }),
        )
        .await;
    wait_for_instance_state(&emulator, row.source_name, "running").await;
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({
                "instanceName": row.source_name,
                "instanceSnapshotName": row.snapshot_name,
            }),
        )
        .await;

    let body = json!({
        "instanceNames": ["too-early"],
        "instanceSnapshotName": row.snapshot_name,
        "availabilityZone": row.restore_zone,
        "bundleId": row.restore_bundle_id,
    });
    let (status, answered) = emulator
        .call("CreateInstancesFromSnapshot", body.clone())
        .await;
    assert_eq!(
        status, 400,
        "case restoring a pending snapshot: expected a 400 refusal, got {status} {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "OperationFailureException",
        "case restoring a pending snapshot: expected the modelled OperationFailureException, \
         got {answered:?}"
    );
    let (read_status, read) = emulator
        .call("GetInstance", json!({"instanceName": "too-early"}))
        .await;
    assert_eq!(
        read_status, 400,
        "case restoring a pending snapshot: a refused restore stores nothing, got \
         {read_status} {read:?}"
    );

    // The clock settles the snapshot, and then the very same request is served.
    wait_for_snapshot_state(&emulator, row.snapshot_name, "available").await;
    emulator.ok("CreateInstancesFromSnapshot", body).await;
    let restored = instance(&emulator, "too-early").await;
    assert_eq!(
        restored["blueprintId"].as_str(),
        Some(row.blueprint_id),
        "case restoring a settled snapshot: expected the snapshotted blueprint, got {restored:?}"
    );
}

/// RULE: an instance name addresses ONE instance, so a request naming a name the
/// account already holds — or naming one twice, or naming none at all, or naming
/// a plan or a zone the service does not serve — is refused, leaves every stored
/// record exactly as it was, and creates none of the request's other names.
#[tokio::test]
async fn a_restore_that_cannot_stand_leaves_the_store_as_it_was() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_source_snapshot(&emulator, row).await;
    let before = instance(&emulator, row.source_name).await;

    // (case, instanceNames, the extra members, the member the refusal names)
    let cases: &[(&str, Value, Value, &str)] = &[
        (
            "a name the account already holds",
            json!([row.source_name]),
            json!({}),
            "already exists",
        ),
        (
            "an existing name beside a free one",
            json!([row.source_name, "would-be-created"]),
            json!({}),
            "already exists",
        ),
        (
            "one name twice",
            json!(["twice", "twice"]),
            json!({}),
            "more than once",
        ),
        (
            "no name at all",
            json!([]),
            json!({}),
            "instanceNames",
        ),
        (
            "a plan the catalog does not hold",
            json!(["bad-plan"]),
            json!({"bundleId": "teapot_9_9"}),
            "bundleId",
        ),
    ];
    for (case, instance_names, extra, names_member) in cases {
        let mut body = json!({
            "instanceNames": instance_names,
            "instanceSnapshotName": row.snapshot_name,
            "availabilityZone": row.restore_zone,
            "bundleId": row.restore_bundle_id,
        });
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            body[member] = value.clone();
        }
        refused_as_invalid(&emulator, case, body, names_member).await;
        for name in instance_names
            .as_array()
            .expect("instanceNames is an array")
            .iter()
            .filter_map(Value::as_str)
            .filter(|name| *name != row.source_name)
        {
            let (status, read) = emulator
                .call("GetInstance", json!({"instanceName": name}))
                .await;
            assert_eq!(
                status, 400,
                "case {case}: a refused restore creates nothing, got {status} {read:?}"
            );
        }
    }

    let after = instance(&emulator, row.source_name).await;
    assert_eq!(
        after, before,
        "case a refused restore: expected the stored instance untouched, got {after:?}"
    );

    // One call creates at most the names this service accepts in one request
    // (`crate::state::MAX_INSTANCE_NAMES_PER_REQUEST`): the bound itself is
    // served and one name past it is refused.
    for (case, prefix, count, accepted) in [
        (
            "the most names one request carries",
            "bulk",
            MAX_INSTANCE_NAMES_PER_REQUEST,
            true,
        ),
        (
            "one name past that",
            "over",
            MAX_INSTANCE_NAMES_PER_REQUEST + 1,
            false,
        ),
    ] {
        let names: Vec<String> = (0..count).map(|index| format!("{prefix}-{index}")).collect();
        let body = json!({
            "instanceNames": names,
            "instanceSnapshotName": row.snapshot_name,
            "availabilityZone": row.restore_zone,
            "bundleId": row.restore_bundle_id,
        });
        if accepted {
            let answered = emulator.ok("CreateInstancesFromSnapshot", body).await;
            assert_eq!(
                answered["operations"].as_array().map(Vec::len),
                Some(count),
                "case {case}: expected one operation per name, got {answered:?}"
            );
        } else {
            refused_as_invalid(&emulator, case, body, "instanceNames").await;
            let (status, read) = emulator
                .call("GetInstance", json!({"instanceName": &names[0]}))
                .await;
            assert_eq!(
                status, 400,
                "case {case}: a refused restore creates nothing, got {status} {read:?}"
            );
        }
    }
}

/// Call `CreateInstancesFromSnapshot` with `body` and require the modelled
/// `InvalidInputException` — HTTP 400 — whose message names `names_member`.
async fn refused_as_invalid(emulator: &Emulator, case: &str, body: Value, names_member: &str) {
    let (status, answered) = emulator.call("CreateInstancesFromSnapshot", body).await;
    assert_eq!(
        status, 400,
        "case {case}: expected a 400 refusal, got {status} {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "InvalidInputException",
        "case {case}: expected the modelled InvalidInputException, got {answered:?}"
    );
    let message = answered["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(names_member),
        "case {case}: expected the message to name {names_member}, got {message:?}"
    );
}

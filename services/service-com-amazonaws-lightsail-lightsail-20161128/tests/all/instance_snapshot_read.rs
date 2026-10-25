// SPDX-License-Identifier: CC0-1.0

//! Reading ONE Lightsail instance snapshot — `GetInstanceSnapshot` — driven over
//! the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `GetInstanceSnapshot` REPORTS the one stored instance
//! snapshot its `instanceSnapshotName` names — every member read out of the
//! single record the create persisted, which is also the record the listing
//! reports, so the two agree member for member — while changing nothing a later
//! caller sees; the region the read is signed for selects nothing, since the
//! snapshot is located where its source was; and a well-formed name that names no
//! stored snapshot is refused with the service's `DoesNotExist` envelope, never a
//! 501 and never a hollow 200.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::disk_attachment::wait_for_instance_state;
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// The stored instance snapshot `name`, as `GetInstanceSnapshot` reports it.
async fn snapshot(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetInstanceSnapshot", json!({"instanceSnapshotName": name}))
        .await["instanceSnapshot"]
        .clone()
}

/// The stored instance `name`, as `GetInstance` reports it.
async fn instance(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetInstance", json!({"instanceName": name})).await["instance"].clone()
}

/// The snapshot `name` as the LISTING reports it, which must be reported exactly
/// once.
async fn listed(emulator: &Emulator, name: &str) -> Value {
    let mut matching: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => {
                emulator
                    .ok("GetInstanceSnapshots", json!({"pageToken": token}))
                    .await
            }
            None => emulator.ok("GetInstanceSnapshots", json!({})).await,
        };
        let entries = body["instanceSnapshots"].as_array().cloned().unwrap_or_default();
        matching.extend(
            entries
                .into_iter()
                .filter(|entry| entry["name"].as_str() == Some(name)),
        );
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        matching.len(),
        1,
        "case reading {name} back from the listing: expected exactly one entry, got {matching:?}"
    );
    matching[0].clone()
}

/// Poll `GetInstanceSnapshot` until it reports `want`, answering the snapshot.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = snapshot(emulator, name).await;
        if snapshot["state"].as_str() == Some(want) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            snapshot["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
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
        let disk = emulator.ok("GetDisk", json!({"diskName": name})).await["disk"].clone();
        if disk["state"].as_str() == Some("available") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for disk {name} to be available: got {:?}",
            disk["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The read table
// ---------------------------------------------------------------------------

/// One snapshot the service must report back through `GetInstanceSnapshot`.
///
/// The rows differ along everything the rule generalises over: the source's
/// BLUEPRINT (a Linux distribution, an application blueprint, Windows), its
/// bundle — and so the system volume size the snapshot reports, from the smallest
/// published plan at 20 GB through a 320 GB one, including a bundle id carrying
/// the Windows platform token and one at a generation the catalog no longer
/// publishes — the Availability Zone and so the region the snapshot's ARN and
/// location carry, the region the READ is signed for (which must select
/// nothing), the number of block storage disks attached to the source (none,
/// one, two) and their sizes, the number of tags, and the form of the snapshot
/// name (hyphens, underscores, digits, mixed case, the shortest the modelled
/// `ResourceName` pattern admits).
struct Row {
    case: &'static str,
    instance_name: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    /// A blueprint id this service publishes (`GetBlueprints`).
    blueprint_id: &'static str,
    /// A bundle id this service serves (`GetBundles`); its leading size token is
    /// what names the plan.
    bundle_id: &'static str,
    snapshot_name: &'static str,
    /// The region the READ is signed for: a snapshot is located where its source
    /// is, so this must change nothing about what is reported.
    read_region: Option<&'static str>,
    /// The block storage disks attached to the source before the snapshot is
    /// taken: name, device path and size.
    disks: &'static [(&'static str, &'static str, i64)],
    tags: &'static [(&'static str, &'static str)],
}

/// The table. Blueprint and bundle ids are entries of this service's own
/// published catalogs (`crate::state::BLUEPRINTS` and
/// `crate::state::BUNDLE_SPECS`), and none of them is a pair the sibling listing
/// suite drives.
const ROWS: &[Row] = &[
    Row {
        case: "debian on the smallest published bundle, no attached disk, no tags",
        instance_name: "debian-nano",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "debian_12",
        bundle_id: "nano_3_0",
        snapshot_name: "debian-nano-snapshot",
        read_region: None,
        disks: &[],
        tags: &[],
    },
    Row {
        case: "an application blueprint on a small bundle, one attached disk, read signed for \
               another region",
        instance_name: "nodejs_app_1",
        availability_zone: "eu-west-1b",
        region: "eu-west-1",
        blueprint_id: "nodejs",
        bundle_id: "small_3_0",
        snapshot_name: "nodejs_snap_1",
        read_region: Some("us-west-2"),
        disks: &[("nodejs-data", "/dev/xvdf", 32)],
        tags: &[("env", "prod")],
    },
    Row {
        case: "windows on a medium bundle whose id carries the platform token, two attached \
               disks, three tags",
        instance_name: "Win2016Build",
        availability_zone: "ap-southeast-2a",
        region: "ap-southeast-2",
        blueprint_id: "windows_server_2016",
        bundle_id: "medium_win_3_0",
        snapshot_name: "Win2016Build-2026-10-24",
        read_region: Some("ap-southeast-2"),
        disks: &[
            ("win-build-one", "/dev/sdf", 256),
            ("win_build_two", "/dev/xvdp", 512),
        ],
        tags: &[("env", "staging"), ("owner", "builds"), ("tier", "silver")],
    },
    Row {
        case: "alma linux on the largest bundle in the table, shortest snapshot name the \
               ResourceName pattern admits",
        instance_name: "alma-xlarge",
        availability_zone: "ca-central-1a",
        region: "ca-central-1",
        blueprint_id: "almalinux_9",
        bundle_id: "xlarge_3_0",
        snapshot_name: "s9",
        read_region: Some("eu-central-1"),
        disks: &[],
        tags: &[("app", "ci")],
    },
    Row {
        case: "ubuntu on a bundle generation the catalog no longer publishes, in a fifth \
               region, one small disk",
        instance_name: "ubuntu_legacy_gen",
        availability_zone: "eu-north-1c",
        region: "eu-north-1",
        blueprint_id: "ubuntu_24_04",
        bundle_id: "micro_2_0",
        snapshot_name: "Snapshot_Legacy_5",
        read_region: None,
        disks: &[("north-disk-8", "/dev/xvdg", 8)],
        tags: &[("env", "dev"), ("owner", "qa")],
    },
];

/// The `tags` member a row's request carries, in the modelled `TagList` form.
fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// Stand up `row`'s source instance, attach every disk the row names, and take
/// the snapshot.
async fn create_row(emulator: &Emulator, row: &Row) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [row.instance_name],
                "availabilityZone": row.availability_zone,
                "blueprintId": row.blueprint_id,
                "bundleId": row.bundle_id,
            }),
        )
        .await;
    wait_for_instance_state(emulator, row.instance_name, "running").await;
    for (disk_name, disk_path, size_in_gb) in row.disks {
        create_available_disk(emulator, disk_name, row.availability_zone, *size_in_gb).await;
        emulator
            .ok(
                "AttachDisk",
                json!({
                    "diskName": disk_name,
                    "instanceName": row.instance_name,
                    "diskPath": disk_path,
                }),
            )
            .await;
    }
    let mut body = json!({
        "instanceName": row.instance_name,
        "instanceSnapshotName": row.snapshot_name,
    });
    if !row.tags.is_empty() {
        body["tags"] = tag_list(row.tags);
    }
    emulator.ok("CreateInstanceSnapshot", body).await;
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: `GetInstanceSnapshot` reports the ONE stored snapshot its name selects,
/// every member read out of the record the create persisted — the source
/// instance's own stored values, the request's tags verbatim, and server-assigned
/// members computed from them plus the store's own allocators — and the listing
/// reports the very same record.
#[tokio::test]
async fn a_stored_snapshot_is_reported_member_for_member() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        create_row(&emulator, row).await;

        // The source as the service stores it: everything the snapshot says
        // about its source must be exactly this.
        let source = instance(&emulator, row.instance_name).await;
        let read = snapshot(&emulator, row.snapshot_name).await;

        assert_eq!(
            read["name"].as_str(),
            Some(row.snapshot_name),
            "case {case}: expected the snapshot the request named, got {read:?}"
        );
        assert_eq!(
            read["resourceType"].as_str(),
            Some("InstanceSnapshot"),
            "case {case}: expected the InstanceSnapshot resource type, got {read:?}"
        );
        assert_eq!(
            read["fromInstanceName"].as_str(),
            Some(row.instance_name),
            "case {case}: expected the source instance's name, got {read:?}"
        );
        assert_eq!(
            read["fromInstanceArn"], source["arn"],
            "case {case}: expected the source instance's own ARN {:?}, got {:?}",
            source["arn"], read["fromInstanceArn"]
        );
        assert_eq!(
            read["fromBlueprintId"], source["blueprintId"],
            "case {case}: expected the source's own blueprint id {:?}, got {:?}",
            source["blueprintId"], read["fromBlueprintId"]
        );
        assert_eq!(
            read["fromBundleId"], source["bundleId"],
            "case {case}: expected the source's own bundle id {:?}, got {:?}",
            source["bundleId"], read["fromBundleId"]
        );

        // The snapshotted volume is the instance's SYSTEM disk, the first disk
        // `GetInstance` reports in its hardware.
        let system_disk = source["hardware"]["disks"][0].clone();
        assert_eq!(
            system_disk["isSystemDisk"].as_bool(),
            Some(true),
            "case {case}: expected the instance's first disk to be its system disk, got \
             {system_disk:?}"
        );
        assert_eq!(
            read["sizeInGb"], system_disk["sizeInGb"],
            "case {case}: expected the system volume's own size {:?}, got {:?}",
            system_disk["sizeInGb"], read["sizeInGb"]
        );

        // Every block storage disk attached to the source is reported, and the
        // system disk is not one of them.
        let attached = read["fromAttachedDisks"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a disk array, got {read:?}"))
            .clone();
        let mut reported_disks: Vec<(String, i64)> = attached
            .iter()
            .map(|disk| {
                (
                    disk["name"].as_str().unwrap_or_default().to_string(),
                    disk["sizeInGb"].as_i64().unwrap_or_default(),
                )
            })
            .collect();
        reported_disks.sort();
        let mut want_disks: Vec<(String, i64)> = row
            .disks
            .iter()
            .map(|(name, _, size_in_gb)| ((*name).to_string(), *size_in_gb))
            .collect();
        want_disks.sort();
        assert_eq!(
            reported_disks, want_disks,
            "case {case}: expected the block storage disks attached to the source, got \
             {attached:?}"
        );

        // Located where the source is.
        assert_eq!(
            read["location"], source["location"],
            "case {case}: expected the source's own location {:?}, got {:?}",
            source["location"], read["location"]
        );

        // The tags the request carried, verbatim and in order — and an empty
        // list, not an absent one, for a request that carried none.
        assert_eq!(
            read["tags"],
            tag_list(row.tags),
            "case {case}: expected the tags the request carried, got {:?}",
            read["tags"]
        );

        // The ARN carries the SOURCE's region and the canonical emulator
        // account, and ends in a uuid no other snapshot is using; the support
        // code names the same id.
        let arn = read["arn"].as_str().unwrap_or_default().to_string();
        let prefix = format!("arn:aws:lightsail:{}:{ACCOUNT}:InstanceSnapshot/", row.region);
        let id = arn
            .strip_prefix(&prefix)
            .unwrap_or_else(|| panic!("case {case}: expected an ARN starting '{prefix}', got {arn:?}"));
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped ARN id, got {id:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: expected a fresh ARN, got the already-used {arn:?}"
        );
        assert_eq!(
            read["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: expected the account/resource-id support code, got {:?}",
            read["supportCode"]
        );
        assert!(
            read["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a creation timestamp, got {:?}",
            read["createdAt"]
        );

        // `progress` "is populated only for disk snapshots, and is null for
        // instance snapshots", and an explicit snapshot is not one the
        // AutoSnapshot add-on took.
        assert!(
            read["progress"].is_null(),
            "case {case}: expected no progress on an instance snapshot, got {read:?}"
        );
        assert_eq!(
            read["isFromAutoSnapshot"].as_bool(),
            Some(false),
            "case {case}: expected a snapshot the caller asked for, got {read:?}"
        );
        let state = read["state"].as_str().unwrap_or_default().to_string();
        assert!(
            state == "pending" || state == "available",
            "case {case}: expected a pending or available snapshot, got {state:?}"
        );

        // One record, two readers: the settled snapshot the single read reports
        // is the entry the listing reports, member for member.
        let settled = wait_for_state(&emulator, row.snapshot_name, "available").await;
        let entry = listed(&emulator, row.snapshot_name).await;
        assert_eq!(
            settled, entry,
            "case {case}: expected the single read and the listing to report one record, got \
             {settled:?} and {entry:?}"
        );

        // A read selects nothing by region and changes nothing: the same name
        // read again, signed for whatever region the row names, answers the
        // same record.
        let again = emulator
            .ok_at(
                None,
                row.read_region,
                "GetInstanceSnapshot",
                json!({"instanceSnapshotName": row.snapshot_name}),
            )
            .await["instanceSnapshot"]
            .clone();
        assert_eq!(
            again, settled,
            "case {case}: a read must not change what a later caller sees, and the region it is \
             signed for selects nothing, got {again:?}"
        );
    }
}

/// RULE: a well-formed name that names no stored snapshot is refused with the
/// service's `DoesNotExist` envelope — HTTP 400, `NotFoundException`, body code
/// `DoesNotExist` and a message naming the resource type and the requested name —
/// never a 501 and never a hollow 200.
#[tokio::test]
async fn an_unknown_snapshot_does_not_exist() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["present-instance"],
                "availabilityZone": "us-east-2a",
                "blueprintId": "amazon_linux_2",
                "bundleId": "nano_3_0",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({
                "instanceName": "present-instance",
                "instanceSnapshotName": "present-snapshot",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({
                "diskSnapshotName": "present-disk-snapshot",
                "instanceName": "present-instance",
            }),
        )
        .await;

    // A name of nothing, a near-miss of a stored name, a name in another form —
    // and the names of resources of OTHER types, which share no namespace with
    // an instance snapshot.
    for name in [
        "absent-snapshot",
        "present-snapshot-2",
        "other_snapshot",
        "present-instance",
        "present-disk-snapshot",
    ] {
        let (status, body) = emulator
            .call("GetInstanceSnapshot", json!({"instanceSnapshotName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case reading {name}: expected a 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case reading {name}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case reading {name}: expected code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The InstanceSnapshot does not exist: {name}").as_str()),
            "case reading {name}: expected the resource-type/name message, got {body:?}"
        );
    }

    // The refusals stored nothing: the one snapshot the account holds is still
    // the only one, and still readable.
    let present = snapshot(&emulator, "present-snapshot").await;
    assert_eq!(
        present["name"].as_str(),
        Some("present-snapshot"),
        "case reading the stored snapshot after the refusals: got {present:?}"
    );
}

/// RULE: a snapshot is created `pending` and the CLOCK, not a reader, settles it
/// into `available` — so a caller reading straight after the create still sees
/// `pending`, no number of reads moves it, and the transition arrives all the
/// same.
#[tokio::test]
async fn a_read_does_not_settle_a_pending_snapshot() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["settling-source"],
                "availabilityZone": "us-west-2a",
                "blueprintId": "debian_11",
                "bundleId": "small_3_0",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({
                "instanceName": "settling-source",
                "instanceSnapshotName": "settling-snapshot",
            }),
        )
        .await;

    let first = snapshot(&emulator, "settling-snapshot").await;
    assert_eq!(
        first["state"].as_str(),
        Some("pending"),
        "case reading straight after the create: expected a pending snapshot, got {first:?}"
    );

    // The settled state is the CLOCK's, so it arrives at the same instant
    // whether or not anyone reads; the read is only how we observe it.
    let settled = wait_for_state(&emulator, "settling-snapshot", "available").await;
    assert_eq!(
        settled["arn"], first["arn"],
        "case settling: expected the same record, got {settled:?}"
    );
    assert_eq!(
        settled["createdAt"], first["createdAt"],
        "case settling: expected the creation time to be unchanged, got {settled:?}"
    );

    // Terminal: a snapshot AWS reports ready stays ready, read after read.
    let after = snapshot(&emulator, "settling-snapshot").await;
    assert_eq!(
        after, settled,
        "case reading a settled snapshot twice: expected the same record, got {after:?}"
    );
}

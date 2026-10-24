// SPDX-License-Identifier: CC0-1.0

//! The Lightsail INSTANCE snapshot — create and list — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `GetInstanceSnapshots` reports the account's stored
//! instance snapshots — every member of every entry read out of the one record
//! `CreateInstanceSnapshot` persisted — in the store's own total order by
//! snapshot name, at most one page at a time, with `nextPageToken` NAMING the
//! first snapshot of the page after it and absent once none remain. A snapshot
//! is a point-in-time copy, so everything it says about its source (name, ARN,
//! blueprint id, bundle id, system volume size, attached block storage disks,
//! location) is read out of the stored INSTANCE record at the instant it is
//! taken, the request's tags are kept verbatim, and every server-assigned member
//! (ARN, support code, creation time, resource type, state) is computed from them
//! plus the store's own allocators. The snapshot is created `pending` and the
//! CLOCK, not a reader, settles it into `available`; a source the account does not
//! hold, or a snapshot name already taken, is refused and stores nothing.

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

/// How many snapshots one page of `GetInstanceSnapshots` carries
/// (`crate::state::INSTANCE_SNAPSHOT_PAGE_SIZE`).
const PAGE_SIZE: usize = 10;

/// The device path an instance's system volume is exposed at
/// (`crate::state::SYSTEM_DISK_PATH`).
const SYSTEM_DISK_PATH: &str = "/dev/xvda";

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// One page of `GetInstanceSnapshots`: the entries it reports, and the token for
/// the page after it when one remains.
async fn snapshot_page(emulator: &Emulator, token: Option<&str>) -> (Vec<Value>, Option<String>) {
    let body = match token {
        Some(token) => {
            emulator
                .ok("GetInstanceSnapshots", json!({"pageToken": token}))
                .await
        }
        None => emulator.ok("GetInstanceSnapshots", json!({})).await,
    };
    let entries = body["instanceSnapshots"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case listing snapshots at {token:?}: expected an array, got {body:?}")
        })
        .clone();
    (entries, body["nextPageToken"].as_str().map(str::to_string))
}

/// Every snapshot the listing reports, walking every page, together with the
/// size of each page.
async fn all_snapshots(emulator: &Emulator) -> (Vec<Value>, Vec<usize>) {
    let mut all: Vec<Value> = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (entries, next) = snapshot_page(emulator, token.as_deref()).await;
        page_sizes.push(entries.len());
        all.extend(entries);
        match next {
            // A page carries at least one entry whenever a next token is
            // reported, so the walk always advances.
            Some(next) => token = Some(next),
            None => break,
        }
    }
    (all, page_sizes)
}

/// The listed snapshot `name`, which must be reported exactly once.
async fn listed(emulator: &Emulator, name: &str) -> Value {
    let (all, _) = all_snapshots(emulator).await;
    let matching: Vec<&Value> = all
        .iter()
        .filter(|entry| entry["name"].as_str() == Some(name))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case reading {name} back from the listing: expected exactly one entry, got {matching:?}"
    );
    matching[0].clone()
}

/// The stored instance `name`, as `GetInstance` reports it.
async fn instance(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetInstance", json!({"instanceName": name})).await["instance"].clone()
}

/// Poll the listing until the snapshot `name` reports `want`, answering it.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = listed(emulator, name).await;
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
// The create/list table
// ---------------------------------------------------------------------------

/// One `CreateInstanceSnapshot` request the service must serve, with the source
/// instance it is taken of.
///
/// The rows differ along everything the rule generalises over: the source's
/// BLUEPRINT (Linux, Windows, an application blueprint), its bundle — and so the
/// system volume size the snapshot reports, from the smallest published plan at
/// 20 GB to a 1,280 GB one, including a bundle id carrying a platform token —
/// the Availability Zone and so the region the snapshot's ARN and location carry,
/// the region the SNAPSHOT call is signed for, which this operation must ignore,
/// the number of block storage disks attached to the source (none, one, two) and
/// their sizes, the number of tags, and the form of the snapshot name (hyphens,
/// underscores, digits, mixed case).
struct Row {
    case: &'static str,
    instance_name: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    /// A blueprint id this service publishes (`GetBlueprints`).
    blueprint_id: &'static str,
    /// A bundle id this service publishes (`GetBundles`).
    bundle_id: &'static str,
    snapshot_name: &'static str,
    /// The region the SNAPSHOT create is signed for: a snapshot is located where
    /// its source is, so this must change nothing.
    snapshot_region: Option<&'static str>,
    /// The block storage disks attached to the source before the snapshot is
    /// taken: name, device path and size.
    disks: &'static [(&'static str, &'static str, i64)],
    tags: &'static [(&'static str, &'static str)],
}

/// The table. Blueprint and bundle ids are entries of this service's own
/// published catalogs (`crate::state::BLUEPRINTS` and
/// `crate::state::BUNDLE_SPECS`), which the first case reads back through
/// `GetInstance` to prove the snapshot copies them rather than inventing them.
const ROWS: &[Row] = &[
    Row {
        case: "amazon linux on a micro bundle, no attached disk, no tags",
        instance_name: "al2023-micro",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "micro_3_0",
        snapshot_name: "al2023-micro-snapshot",
        snapshot_region: None,
        disks: &[],
        tags: &[],
    },
    Row {
        case: "ubuntu on the smallest published bundle, one attached disk, snapshot signed \
               for another region",
        instance_name: "ubuntu_nano_1",
        availability_zone: "us-west-2b",
        region: "us-west-2",
        blueprint_id: "ubuntu_22_04",
        bundle_id: "nano_3_0",
        snapshot_name: "ubuntu_nano_snap_1",
        snapshot_region: Some("eu-central-1"),
        disks: &[("ubuntu-nano-data", "/dev/xvdf", 64)],
        tags: &[("env", "prod")],
    },
    Row {
        case: "windows on a large bundle, two attached disks, three tags",
        instance_name: "WinServer2022",
        availability_zone: "ap-northeast-1c",
        region: "ap-northeast-1",
        blueprint_id: "windows_server_2022",
        bundle_id: "large_3_0",
        snapshot_name: "WinServer2022-2026-10-24",
        snapshot_region: Some("ap-northeast-1"),
        disks: &[
            ("win-data-one", "/dev/sdf", 128),
            ("win_data_two", "/dev/xvdp", 1024),
        ],
        tags: &[("env", "staging"), ("owner", "platform"), ("tier", "gold")],
    },
    Row {
        case: "an application blueprint on the largest bundle in the table, shortest \
               snapshot name",
        instance_name: "wordpress-4xl",
        availability_zone: "eu-west-3a",
        region: "eu-west-3",
        blueprint_id: "wordpress",
        bundle_id: "4xlarge_3_0",
        snapshot_name: "s4",
        snapshot_region: Some("us-east-1"),
        disks: &[],
        tags: &[("app", "blog")],
    },
    Row {
        case: "a bundle id carrying a platform token, in a fifth region, one small disk",
        instance_name: "win2019_micro",
        availability_zone: "sa-east-1a",
        region: "sa-east-1",
        blueprint_id: "windows_server_2019",
        bundle_id: "micro_win_2_0",
        snapshot_name: "Snapshot_Five",
        snapshot_region: None,
        disks: &[("sa-disk-8", "/dev/xvdg", 8)],
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

/// Stand up `row`'s source instance and attach every disk the row names.
async fn create_source(emulator: &Emulator, row: &Row) {
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
}

/// The `CreateInstanceSnapshot` request `row` describes.
fn snapshot_body(row: &Row) -> Value {
    let mut body = json!({
        "instanceName": row.instance_name,
        "instanceSnapshotName": row.snapshot_name,
    });
    if !row.tags.is_empty() {
        body["tags"] = tag_list(row.tags);
    }
    body
}

/// Create `row`'s source instance and then its snapshot, answering the
/// `CreateInstanceSnapshot` response body.
async fn create_row(emulator: &Emulator, row: &Row) -> Value {
    create_source(emulator, row).await;
    emulator
        .ok_at(
            None,
            row.snapshot_region,
            "CreateInstanceSnapshot",
            snapshot_body(row),
        )
        .await
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: every member of a listed snapshot comes from the ONE record the create
/// persisted — the source instance's own stored values, the request's tags
/// verbatim, and server-assigned members computed from them plus the store's
/// allocators.
#[tokio::test]
async fn created_snapshots_are_listed_exactly() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let answered = create_row(&emulator, row).await;

        // One create, one logged operation, located where the source is.
        let operations = answered["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {answered:?}"))
            .clone();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: one snapshot is created, so one operation is logged, got {operations:?}"
        );
        let operation = &operations[0];
        assert_eq!(
            operation["operationType"].as_str(),
            Some("CreateInstanceSnapshot"),
            "case {case}: expected this call's own operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("InstanceSnapshot"),
            "case {case}: expected the snapshot resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(row.snapshot_name),
            "case {case}: expected the operation to name the snapshot, got {operation:?}"
        );
        let operation_id = operation["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&operation_id),
            "case {case}: expected a uuid-shaped operation id, got {operation_id:?}"
        );
        assert!(
            operation_ids.insert(operation_id.clone()),
            "case {case}: expected a fresh operation id, got the already-used {operation_id:?}"
        );

        // The source as the service stores it: everything the snapshot says
        // about its source must be exactly this.
        let source = instance(&emulator, row.instance_name).await;
        let snapshot = listed(&emulator, row.snapshot_name).await;

        assert_eq!(
            snapshot["name"].as_str(),
            Some(row.snapshot_name),
            "case {case}: expected the snapshot name the request carried, got {snapshot:?}"
        );
        assert_eq!(
            snapshot["resourceType"].as_str(),
            Some("InstanceSnapshot"),
            "case {case}: expected the InstanceSnapshot resource type, got {snapshot:?}"
        );
        assert_eq!(
            snapshot["fromInstanceName"].as_str(),
            Some(row.instance_name),
            "case {case}: expected the source instance's name, got {snapshot:?}"
        );
        assert_eq!(
            snapshot["fromInstanceArn"], source["arn"],
            "case {case}: expected the source instance's own ARN {:?}, got {:?}",
            source["arn"], snapshot["fromInstanceArn"]
        );
        assert_eq!(
            snapshot["fromBlueprintId"], source["blueprintId"],
            "case {case}: expected the source's own blueprint id {:?}, got {:?}",
            source["blueprintId"], snapshot["fromBlueprintId"]
        );
        assert_eq!(
            snapshot["fromBundleId"], source["bundleId"],
            "case {case}: expected the source's own bundle id {:?}, got {:?}",
            source["bundleId"], snapshot["fromBundleId"]
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
            snapshot["sizeInGb"], system_disk["sizeInGb"],
            "case {case}: expected the system volume's own size {:?}, got {:?}",
            system_disk["sizeInGb"], snapshot["sizeInGb"]
        );

        // Every block storage disk attached to the source is reported, and the
        // system disk is not one of them: `fromAttachedDisks` is "an array of
        // disk objects containing information about all block storage disks".
        let attached = snapshot["fromAttachedDisks"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a disk array, got {snapshot:?}"))
            .clone();
        let mut listed_disks: Vec<(String, i64)> = attached
            .iter()
            .map(|disk| {
                (
                    disk["name"].as_str().unwrap_or_default().to_string(),
                    disk["sizeInGb"].as_i64().unwrap_or_default(),
                )
            })
            .collect();
        listed_disks.sort();
        let mut want_disks: Vec<(String, i64)> = row
            .disks
            .iter()
            .map(|(name, _, size_in_gb)| ((*name).to_string(), *size_in_gb))
            .collect();
        want_disks.sort();
        assert_eq!(
            listed_disks, want_disks,
            "case {case}: expected the block storage disks attached to the source, got \
             {attached:?}"
        );

        // Located where the source is — whatever region the snapshot call was
        // signed for.
        assert_eq!(
            snapshot["location"], source["location"],
            "case {case}: expected the source's own location {:?}, got {:?}",
            source["location"], snapshot["location"]
        );
        assert_eq!(
            operation["location"], source["location"],
            "case {case}: expected the operation to be located where the source is, got {:?}",
            operation["location"]
        );

        // The tags the request carried, verbatim and in order — and an empty
        // list, not an absent one, for a request that carried none.
        assert_eq!(
            snapshot["tags"],
            tag_list(row.tags),
            "case {case}: expected the tags the request carried, got {:?}",
            snapshot["tags"]
        );

        // The ARN carries the SOURCE's region and the canonical emulator
        // account, and ends in a uuid no other snapshot is using; the support
        // code names the same id.
        let arn = snapshot["arn"].as_str().unwrap_or_default().to_string();
        let prefix = format!(
            "arn:aws:lightsail:{}:{ACCOUNT}:InstanceSnapshot/",
            row.region
        );
        let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case}: expected an ARN starting '{prefix}', got {arn:?}")
        });
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped ARN id, got {id:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: expected a fresh ARN, got the already-used {arn:?}"
        );
        assert_eq!(
            snapshot["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: expected the account/resource-id support code, got {:?}",
            snapshot["supportCode"]
        );
        assert!(
            snapshot["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a creation timestamp, got {:?}",
            snapshot["createdAt"]
        );

        // `progress` "is populated only for disk snapshots, and is null for
        // instance snapshots", and an explicit snapshot is not one the
        // AutoSnapshot add-on took.
        assert!(
            snapshot["progress"].is_null(),
            "case {case}: expected no progress on an instance snapshot, got {snapshot:?}"
        );
        assert_eq!(
            snapshot["isFromAutoSnapshot"].as_bool(),
            Some(false),
            "case {case}: expected a snapshot the caller asked for, got {snapshot:?}"
        );

        // The state is one this store produces.
        let state = snapshot["state"].as_str().unwrap_or_default().to_string();
        assert!(
            state == "pending" || state == "available",
            "case {case}: expected a pending or available snapshot, got {state:?}"
        );
    }

    // The listing reports exactly the snapshots created, once each, in the
    // store's own order by name — and two walks answer the same thing.
    let (all, _) = all_snapshots(&emulator).await;
    let listed_names: Vec<String> = all
        .iter()
        .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut want_names: Vec<String> = ROWS.iter().map(|row| row.snapshot_name.to_string()).collect();
    want_names.sort();
    assert_eq!(
        listed_names, want_names,
        "case listing every created snapshot: expected the created names in name order, got \
         {listed_names:?}"
    );
    let (again, _) = all_snapshots(&emulator).await;
    assert_eq!(
        again, all,
        "case listing twice: a read must not change what a later caller sees, got {again:?}"
    );
}

/// RULE: an account holding no instance snapshot is told so — an EMPTY list, not
/// an absent one, and no next page token.
#[tokio::test]
async fn an_account_with_no_snapshots_lists_an_empty_array() {
    let emulator = Emulator::new();
    let body = emulator.ok("GetInstanceSnapshots", json!({})).await;
    assert_eq!(
        body["instanceSnapshots"],
        json!([]),
        "case an empty account: expected an empty snapshot list, got {body:?}"
    );
    assert!(
        body["nextPageToken"].is_null(),
        "case an empty account: expected no next page token, got {body:?}"
    );

    // An instance with no snapshot taken of it is still no snapshot.
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["unsnapshotted"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            }),
        )
        .await;
    let body = emulator.ok("GetInstanceSnapshots", json!({})).await;
    assert_eq!(
        body["instanceSnapshots"],
        json!([]),
        "case an instance with no snapshot: expected an empty snapshot list, got {body:?}"
    );
}

/// RULE: the listing is paged — a page carries at most `PAGE_SIZE` snapshots in
/// name order, `nextPageToken` NAMES the first snapshot of the page after it,
/// and the pages together are every snapshot exactly once.
#[tokio::test]
async fn the_listing_pages_in_name_order() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["paging-source"],
                "availabilityZone": "us-east-1b",
                "blueprintId": "ubuntu_22_04",
                "bundleId": "nano_3_0",
            }),
        )
        .await;

    // Two full pages and a partial one — the count is deliberately not a
    // multiple of the page size. The names are created out of order so the
    // listing's order is the STORE's, not the order of creation.
    let total = PAGE_SIZE * 2 + 3;
    let mut names: Vec<String> = (0..total).map(|index| format!("snap-{index:03}")).collect();
    for name in names.iter().rev() {
        emulator
            .ok(
                "CreateInstanceSnapshot",
                json!({"instanceName": "paging-source", "instanceSnapshotName": name}),
            )
            .await;
    }
    names.sort();

    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let (entries, next) = snapshot_page(&emulator, token.as_deref()).await;
        pages += 1;
        assert!(
            entries.len() <= PAGE_SIZE,
            "case page {pages}: expected at most {PAGE_SIZE} entries, got {}",
            entries.len()
        );
        let page_names: Vec<String> = entries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
            .collect();
        match &next {
            Some(next) => {
                assert_eq!(
                    page_names.len(),
                    PAGE_SIZE,
                    "case page {pages}: a page that reports a next token is full, got \
                     {page_names:?}"
                );
                let (following, _) = snapshot_page(&emulator, Some(next)).await;
                assert_eq!(
                    following.first().and_then(|entry| entry["name"].as_str()),
                    Some(next.as_str()),
                    "case page {pages}: the next page token must NAME the first snapshot of the \
                     page after it, got {following:?}"
                );
            }
            None => assert!(
                page_names.len() < PAGE_SIZE || walked.len() + page_names.len() == total,
                "case page {pages}: the last page reports no next token only when nothing \
                 remains, got {page_names:?}"
            ),
        }
        walked.extend(page_names);
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(
        walked, names,
        "case walking every page: expected every snapshot exactly once in name order, got \
         {walked:?}"
    );
    assert_eq!(
        pages, 3,
        "case walking every page: {total} snapshots at {PAGE_SIZE} per page is 3 pages, got \
         {pages}"
    );
}

/// RULE: a snapshot is created `pending` and the CLOCK settles it into
/// `available` — so a caller reading straight after the create still sees
/// `pending`, and the transition arrives without anyone polling for it.
#[tokio::test]
async fn a_snapshot_is_created_pending_and_the_clock_settles_it() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["settling-source"],
                "availabilityZone": "us-west-2a",
                "blueprintId": "amazon_linux_2023",
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

    let fresh = listed(&emulator, "settling-snapshot").await;
    assert_eq!(
        fresh["state"].as_str(),
        Some("pending"),
        "case reading straight after the create: expected a pending snapshot, got {fresh:?}"
    );

    let settled = wait_for_state(&emulator, "settling-snapshot", "available").await;
    // Settling changes the state and NOTHING else about the record.
    for member in [
        "name",
        "arn",
        "supportCode",
        "createdAt",
        "location",
        "resourceType",
        "sizeInGb",
        "fromInstanceName",
        "fromInstanceArn",
        "fromBlueprintId",
        "fromBundleId",
        "fromAttachedDisks",
        "isFromAutoSnapshot",
        "tags",
    ] {
        assert_eq!(
            settled[member], fresh[member],
            "case settling to available: expected {member} to be untouched, got {:?}",
            settled[member]
        );
    }
}

/// RULE: a snapshot is a point-in-time COPY, so what it captured does not follow
/// its source afterwards: the block storage disks it recorded stay recorded once
/// they are detached, and the snapshot outlives the instance it was taken of.
#[tokio::test]
async fn a_snapshot_does_not_follow_its_source_afterwards() {
    let emulator = Emulator::new();
    let row = &Row {
        case: "one attached disk, detached after the snapshot",
        instance_name: "point-in-time",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "micro_3_0",
        snapshot_name: "point-in-time-snapshot",
        snapshot_region: None,
        disks: &[("point-in-time-data", "/dev/xvdf", 32)],
        tags: &[],
    };
    create_row(&emulator, row).await;
    let before = listed(&emulator, row.snapshot_name).await;
    assert_eq!(
        before["fromAttachedDisks"]
            .as_array()
            .map(|disks| disks.len()),
        Some(1),
        "case before the detach: expected the attached disk to be captured, got {before:?}"
    );

    // Detaching the disk, and then deleting the instance, changes nothing the
    // snapshot reports. A disk is detached from a STOPPED instance, so the
    // source is stopped first.
    emulator
        .ok("StopInstance", json!({"instanceName": row.instance_name}))
        .await;
    wait_for_instance_state(&emulator, row.instance_name, "stopped").await;
    emulator
        .ok("DetachDisk", json!({"diskName": "point-in-time-data"}))
        .await;
    let after_detach = listed(&emulator, row.snapshot_name).await;
    for member in [
        "fromAttachedDisks",
        "fromInstanceName",
        "fromInstanceArn",
        "fromBlueprintId",
        "fromBundleId",
        "sizeInGb",
        "arn",
        "location",
    ] {
        assert_eq!(
            after_detach[member], before[member],
            "case after detaching the disk: expected {member} to be untouched, got {:?}",
            after_detach[member]
        );
    }

    emulator
        .ok("DeleteInstance", json!({"instanceName": row.instance_name}))
        .await;
    let after_delete = listed(&emulator, row.snapshot_name).await;
    assert_eq!(
        after_delete["fromInstanceName"].as_str(),
        Some(row.instance_name),
        "case after deleting the source: expected the snapshot to keep naming it, got \
         {after_delete:?}"
    );
    assert_eq!(
        after_delete["fromInstanceArn"], before["fromInstanceArn"],
        "case after deleting the source: expected the captured ARN to be untouched, got \
         {after_delete:?}"
    );
}

/// RULE: the source must be an instance this account holds, so a well-formed
/// name that names none is refused with the service's `DoesNotExist` envelope —
/// HTTP 400, `NotFoundException`, body code `DoesNotExist` and a message naming
/// the resource type and the requested name — and stores nothing.
#[tokio::test]
async fn a_source_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    // A snapshot of an instance that exists, so the store is not merely empty.
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["present-instance"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            }),
        )
        .await;
    emulator
        .ok("CreateDisk", json!({
            "diskName": "present-disk",
            "availabilityZone": "us-east-1a",
            "sizeInGb": 16,
        }))
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
    let (before, _) = all_snapshots(&emulator).await;

    // Each name below is admitted by the modelled `ResourceName` pattern and
    // names no instance: one never created, one differing from the stored
    // instance's name only in case, a DISK, and the snapshot itself.
    for (case, source_name) in [
        ("an instance that was never created", "absent-instance"),
        ("the stored name in another case", "PRESENT-INSTANCE"),
        ("a disk used as the source", "present-disk"),
        ("a snapshot name used as the source", "present-snapshot"),
    ] {
        let (status, body) = emulator
            .call(
                "CreateInstanceSnapshot",
                json!({
                    "instanceName": source_name,
                    "instanceSnapshotName": "refused-snapshot",
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The Instance does not exist: {source_name}").as_str()),
            "case {case}: expected the resource-type/name message, got {body:?}"
        );

        let (after, _) = all_snapshots(&emulator).await;
        assert_eq!(
            after, before,
            "case {case}: a refused create stores nothing, got {after:?}"
        );
    }
}

/// RULE: a snapshot name addresses ONE snapshot, so creating one the account
/// already holds is refused with the modelled `InvalidInputException` and leaves
/// the stored record — and the listing — untouched.
#[tokio::test]
async fn creating_a_snapshot_that_exists_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    for (instance_name, blueprint_id, bundle_id) in [
        ("instance-one", "amazon_linux_2023", "nano_3_0"),
        ("instance-two", "ubuntu_22_04", "medium_3_0"),
    ] {
        emulator
            .ok(
                "CreateInstances",
                json!({
                    "instanceNames": [instance_name],
                    "availabilityZone": "us-east-1a",
                    "blueprintId": blueprint_id,
                    "bundleId": bundle_id,
                }),
            )
            .await;
    }
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({
                "instanceName": "instance-one",
                "instanceSnapshotName": "taken-name",
                "tags": [{"key": "env", "value": "prod"}],
            }),
        )
        .await;
    let before = listed(&emulator, "taken-name").await;

    // Re-taking the name — even for a DIFFERENT source instance — is refused,
    // and the record the first create stored is the one that remains.
    for (case, source_name) in [
        ("the same source instance", "instance-one"),
        ("a different source instance", "instance-two"),
    ] {
        let (status, body) = emulator
            .call(
                "CreateInstanceSnapshot",
                json!({
                    "instanceName": source_name,
                    "instanceSnapshotName": "taken-name",
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected the modelled InvalidInputException, got {body:?}"
        );

        let after = listed(&emulator, "taken-name").await;
        assert_eq!(
            after["fromInstanceName"], before["fromInstanceName"],
            "case {case}: expected the stored snapshot's source to be untouched, got {after:?}"
        );
        assert_eq!(
            after["arn"], before["arn"],
            "case {case}: expected the stored snapshot's ARN to be untouched, got {after:?}"
        );
        assert_eq!(
            after["tags"], before["tags"],
            "case {case}: expected the stored snapshot's tags to be untouched, got {after:?}"
        );
    }
}

/// RULE: one instance can be snapshotted as often as the caller likes and every
/// snapshot is its own listed record.
#[tokio::test]
async fn each_snapshot_of_one_instance_is_its_own_record() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["repeat-source"],
                "availabilityZone": "eu-west-3a",
                "blueprintId": "wordpress",
                "bundleId": "small_3_0",
            }),
        )
        .await;
    let names = ["repeat-a", "repeat-b", "repeat-c"];
    for name in names {
        emulator
            .ok(
                "CreateInstanceSnapshot",
                json!({"instanceName": "repeat-source", "instanceSnapshotName": name}),
            )
            .await;
    }

    let (all, _) = all_snapshots(&emulator).await;
    assert_eq!(
        all.len(),
        names.len(),
        "case snapshotting one instance three times: expected three listed snapshots, got {all:?}"
    );
    let mut arns: BTreeSet<String> = BTreeSet::new();
    for name in names {
        let snapshot = listed(&emulator, name).await;
        assert_eq!(
            snapshot["fromInstanceName"].as_str(),
            Some("repeat-source"),
            "case {name}: expected the one source instance, got {snapshot:?}"
        );
        let arn = snapshot["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            arns.insert(arn.clone()),
            "case {name}: expected an ARN of its own, got the already-used {arn:?}"
        );
    }
}

/// RULE: an instance snapshot is exportable to Amazon EC2 exactly as a disk
/// snapshot is — the export record describes it as an `InstanceSnapshot`, names
/// the instance it was taken of, carries the blueprint and bundle that instance
/// was created from together with the disks that were attached to it (its system
/// volume first), and creates an Amazon Machine Image rather than an EBS volume.
#[tokio::test]
async fn an_instance_snapshot_is_exportable() {
    let emulator = Emulator::new();
    let row = &Row {
        case: "an instance snapshot with one attached disk",
        instance_name: "export-source",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "micro_3_0",
        snapshot_name: "export-source-snapshot",
        snapshot_region: None,
        disks: &[("export-data", "/dev/xvdf", 48)],
        tags: &[],
    };
    create_row(&emulator, row).await;
    let snapshot = listed(&emulator, row.snapshot_name).await;

    let exported = emulator
        .ok(
            "ExportSnapshot",
            json!({"sourceSnapshotName": row.snapshot_name}),
        )
        .await;
    let operation = exported["operations"][0].clone();
    assert_eq!(
        operation["resourceType"].as_str(),
        Some("InstanceSnapshot"),
        "case exporting an instance snapshot: expected the snapshot's resource type, got \
         {operation:?}"
    );

    let records = emulator.ok("GetExportSnapshotRecords", json!({})).await["exportSnapshotRecords"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        records.len(),
        1,
        "case exporting an instance snapshot: expected one export record, got {records:?}"
    );
    let source_info = records[0]["sourceInfo"].clone();
    assert_eq!(
        source_info["resourceType"].as_str(),
        Some("InstanceSnapshot"),
        "case exporting an instance snapshot: expected the InstanceSnapshot source type, got \
         {source_info:?}"
    );
    assert_eq!(
        source_info["name"].as_str(),
        Some(row.snapshot_name),
        "case exporting an instance snapshot: expected the exported snapshot's name, got \
         {source_info:?}"
    );
    assert_eq!(
        source_info["arn"], snapshot["arn"],
        "case exporting an instance snapshot: expected the exported snapshot's own ARN, got \
         {source_info:?}"
    );
    assert_eq!(
        source_info["fromResourceName"].as_str(),
        Some(row.instance_name),
        "case exporting an instance snapshot: expected the snapshot's source instance, got \
         {source_info:?}"
    );
    assert!(
        source_info["diskSnapshotInfo"].is_null(),
        "case exporting an instance snapshot: the export of an instance snapshot describes no \
         disk snapshot, got {source_info:?}"
    );
    let info = source_info["instanceSnapshotInfo"].clone();
    assert_eq!(
        info["fromBlueprintId"].as_str(),
        Some(row.blueprint_id),
        "case exporting an instance snapshot: expected the source's blueprint id, got {info:?}"
    );
    assert_eq!(
        info["fromBundleId"].as_str(),
        Some(row.bundle_id),
        "case exporting an instance snapshot: expected the source's bundle id, got {info:?}"
    );
    let disks = info["fromDiskInfo"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        disks.len(),
        2,
        "case exporting an instance snapshot: expected the system volume and the one attached \
         disk, got {disks:?}"
    );
    assert_eq!(
        disks[0]["isSystemDisk"].as_bool(),
        Some(true),
        "case exporting an instance snapshot: expected the system volume first, got {disks:?}"
    );
    assert_eq!(
        disks[0]["path"].as_str(),
        Some(SYSTEM_DISK_PATH),
        "case exporting an instance snapshot: expected the system volume's path, got {disks:?}"
    );
    assert_eq!(
        disks[0]["sizeInGb"], snapshot["sizeInGb"],
        "case exporting an instance snapshot: expected the snapshotted volume's size, got \
         {disks:?}"
    );
    assert_eq!(
        disks[1]["name"].as_str(),
        Some("export-data"),
        "case exporting an instance snapshot: expected the attached disk, got {disks:?}"
    );
    assert_eq!(
        disks[1]["isSystemDisk"].as_bool(),
        Some(false),
        "case exporting an instance snapshot: a block storage disk is not a system disk, got \
         {disks:?}"
    );

    // "Exported instance snapshots appear in Amazon EC2 as Amazon Machine Images
    // (AMIs)", so the destination resource is an image, not a volume.
    let deadline = Instant::now() + Duration::from_secs(5);
    let destination_id = loop {
        let records = emulator.ok("GetExportSnapshotRecords", json!({})).await
            ["exportSnapshotRecords"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let record = records[0].clone();
        if record["state"].as_str() == Some("Succeeded") {
            break record["destinationInfo"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case exporting an instance snapshot: expected the export to succeed within 5s, got \
             {record:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(
        destination_id.starts_with("ami-"),
        "case exporting an instance snapshot: expected an AMI id, got {destination_id:?}"
    );
}

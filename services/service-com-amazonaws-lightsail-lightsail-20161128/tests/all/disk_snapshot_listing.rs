// SPDX-License-Identifier: CC0-1.0

//! The Lightsail disk snapshot LISTING — `GetDiskSnapshots` — driven over the
//! wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a listing of a stored resource reports the persisted
//! records the account holds in the Region the request is signed for, as this
//! operation's modelled sentence scopes it ("all block storage disk snapshots in
//! your AWS account and region") — every member of every entry read out of the one
//! record the create persisted, so a listed entry is exactly what the singular
//! reader answers for the same name — in the store's own total order by that
//! resource's name, at most one page at a time, with `nextPageToken` NAMING the
//! first record of the page after it and absent once none remain; a Region holding
//! none is told so with an empty list; and the listing is a report, so walking it
//! twice changes nothing, while a create or a delete is visible in the next walk.
//!
//! Which Region each listing reports, across the Regions one account holds records
//! in, is `block_storage_region_scope.rs`; here every record is created in the
//! Region the walk is made in, so these cases are about the records themselves.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness this test target already exposes: a sibling suite drives the
// service through the same front door rather than standing up a second one.
use crate::instance_lifecycle::Emulator;

/// How many snapshots one page of `GetDiskSnapshots` carries
/// (`crate::state::DISK_SNAPSHOT_PAGE_SIZE`).
const PAGE_SIZE: usize = 10;

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// One page of `GetDiskSnapshots`, made in `region` when one is named — a request
/// naming none is made in the default Region: the entries it reports, and the
/// token for the page after it when one remains.
async fn snapshot_page_in(
    emulator: &Emulator,
    region: Option<&str>,
    token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let request = match token {
        Some(token) => json!({"pageToken": token}),
        None => json!({}),
    };
    let body = emulator
        .ok_at(None, region, "GetDiskSnapshots", request)
        .await;
    let entries = body["diskSnapshots"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case listing snapshots in {region:?} at {token:?}: expected an array, got {body:?}"
            )
        })
        .clone();
    (entries, body["nextPageToken"].as_str().map(str::to_string))
}

/// The same page, for a case whose records are in the default Region.
async fn snapshot_page(emulator: &Emulator, token: Option<&str>) -> (Vec<Value>, Option<String>) {
    snapshot_page_in(emulator, None, token).await
}

/// Every snapshot the listing made in `region` reports, walking every page.
async fn all_snapshots_in(emulator: &Emulator, region: Option<&str>) -> Vec<Value> {
    let mut all: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (entries, next) = snapshot_page_in(emulator, region, token.as_deref()).await;
        all.extend(entries);
        match next {
            // A page carries at least one entry whenever a next token is
            // reported, so the walk always advances.
            Some(next) => token = Some(next),
            None => break,
        }
    }
    all
}

/// The names the listing reports, in the order it reports them.
fn names_of(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The snapshot `name` as the listing of `region` reports it, which must report it
/// exactly once.
async fn listed(emulator: &Emulator, region: &str, name: &str) -> Value {
    let all = all_snapshots_in(emulator, Some(region)).await;
    let matching: Vec<&Value> = all
        .iter()
        .filter(|entry| entry["name"].as_str() == Some(name))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case reading {name} back from the listing in {region:?}: expected exactly one entry, got \
         {matching:?}"
    );
    matching[0].clone()
}

/// The stored disk snapshot `name`, as the SINGULAR `GetDiskSnapshot` reports it.
async fn singular(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetDiskSnapshot", json!({"diskSnapshotName": name}))
        .await["diskSnapshot"]
        .clone()
}

/// Poll the singular reader until the snapshot `name` is `completed`, so a later
/// comparison is between two reads of a settled record.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_completed(emulator: &Emulator, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = singular(emulator, name).await;
        if snapshot["state"].as_str() == Some("completed") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for 'completed' on {name}: expected it within 5s, got {:?}",
            snapshot["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The create/list table
// ---------------------------------------------------------------------------

/// What one row snapshots: the two mutually exclusive sources the modelled
/// `CreateDiskSnapshot` input admits.
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

/// One snapshot the listing must report.
///
/// The rows differ along everything the rule generalises over: the KIND of
/// source (a block storage disk, an instance's system volume), the source
/// volume's size — from the smallest disk Lightsail creates to a large one — the
/// Availability Zone and so the Region the record carries and the Region whose
/// listing reports it, the number of tags, and the form of the snapshot name
/// (hyphens, underscores, digits, mixed case).
struct Row {
    case: &'static str,
    source_name: &'static str,
    snapshot_name: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    source: Source,
    tags: &'static [(&'static str, &'static str)],
}

/// The table. Blueprint and bundle ids are entries of this service's own
/// published catalogs (`GetBlueprints` and `GetBundles`).
const ROWS: &[Row] = &[
    Row {
        case: "the documented disk example, no tags",
        source_name: "Disk-Virginia-1",
        snapshot_name: "my-disk-snapshot",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        source: Source::Disk { size_in_gb: 32 },
        tags: &[],
    },
    Row {
        case: "the smallest disk Lightsail creates, in another region, one tag",
        source_name: "small-disk-8",
        snapshot_name: "small_disk_snapshot_1",
        availability_zone: "eu-west-1c",
        region: "eu-west-1",
        source: Source::Disk { size_in_gb: 8 },
        tags: &[("tier", "edge")],
    },
    Row {
        case: "a large disk, two tags, mixed-case name",
        source_name: "data-disk-4096",
        snapshot_name: "Data_Disk_4096_Backup",
        availability_zone: "ap-southeast-2b",
        region: "ap-southeast-2",
        source: Source::Disk { size_in_gb: 4096 },
        tags: &[("stack", "prod"), ("owner", "storage-team")],
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
        tags: &[],
    },
    Row {
        case: "an instance's system volume, a larger bundle, one tag, shortest name",
        source_name: "medium_instance_2",
        snapshot_name: "s5",
        availability_zone: "ca-central-1a",
        region: "ca-central-1",
        source: Source::Instance {
            blueprint_id: "debian_12",
            bundle_id: "medium_3_0",
        },
        tags: &[("keep", "yes")],
    },
];

/// A Region the service serves that none of the rows creates a record in, so its
/// listing reports none of them.
const REGION_HOLDING_NONE: &str = "ap-northeast-1";

/// Create `row`'s source resource.
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

/// The `CreateDiskSnapshot` body for `row`.
fn create_body(row: &Row) -> Value {
    let mut body = json!({"diskSnapshotName": row.snapshot_name});
    match row.source {
        Source::Disk { .. } => body["diskName"] = json!(row.source_name),
        Source::Instance { .. } => body["instanceName"] = json!(row.source_name),
    }
    if !row.tags.is_empty() {
        body["tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        );
    }
    body
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: a listed entry is the ONE persisted record, every member of it — so it
/// is exactly what the singular reader answers for the same name — and the
/// listing of a Region reports every record the account holds THERE, once each,
/// in name order.
#[tokio::test]
async fn created_disk_snapshots_are_listed_exactly() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let name = row.snapshot_name;
        create_source(&emulator, row).await;
        emulator.ok("CreateDiskSnapshot", create_body(row)).await;
        wait_for_completed(&emulator, name).await;

        // The listed entry and the singular read are two reads of ONE record,
        // so they agree member for member.
        let from_listing = listed(&emulator, row.region, name).await;
        let from_singular = singular(&emulator, name).await;
        assert_eq!(
            from_listing, from_singular,
            "case {case}: the listing must report the same record the singular reader does, got \
             {from_listing:?}"
        );

        // Spot-check that the record is the stored one rather than a zeroed
        // entry: the source's own location and the request's tags.
        assert_eq!(
            from_listing["location"]["availabilityZone"].as_str(),
            Some(row.availability_zone),
            "case {case}: expected the source's own zone, got {from_listing:?}"
        );
        assert_eq!(
            from_listing["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the source's own region, got {from_listing:?}"
        );
        assert_eq!(
            from_listing["resourceType"].as_str(),
            Some("DiskSnapshot"),
            "case {case}: expected the DiskSnapshot resource type, got {from_listing:?}"
        );
        let listed_tags: Vec<(String, String)> = from_listing["tags"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a tags array, got {from_listing:?}"))
            .iter()
            .map(|tag| {
                (
                    tag["key"].as_str().unwrap_or_default().to_string(),
                    tag["value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let want_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            listed_tags, want_tags,
            "case {case}: expected the tags the request carried, got {listed_tags:?}"
        );
        match row.source {
            Source::Disk { .. } => assert_eq!(
                from_listing["fromDiskName"].as_str(),
                Some(row.source_name),
                "case {case}: expected the source disk's name, got {from_listing:?}"
            ),
            Source::Instance { .. } => assert_eq!(
                from_listing["fromInstanceName"].as_str(),
                Some(row.source_name),
                "case {case}: expected the source instance's name, got {from_listing:?}"
            ),
        }
    }

    // Every created snapshot, once each, in the store's own order by name — under
    // the Region whose records it is: the modelled sentence scopes this listing to
    // "your AWS account and region", and a snapshot belongs to the Region it was
    // taken in.
    let mut by_region: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for row in ROWS {
        by_region
            .entry(row.region)
            .or_default()
            .push(row.snapshot_name.to_string());
    }
    for names in by_region.values_mut() {
        names.sort();
    }
    let mut walked: Vec<(&str, Vec<Value>)> = Vec::new();
    for (region, want_names) in &by_region {
        let (entries, next) = snapshot_page_in(&emulator, Some(region), None).await;
        assert_eq!(
            names_of(&entries),
            *want_names,
            "case listing the snapshots {region} holds: expected its created names in name \
             order, got {:?}",
            names_of(&entries)
        );
        assert_eq!(
            next, None,
            "case listing the snapshots {region} holds: {} of them fit one page of {PAGE_SIZE}, \
             so no next token is reported, got {next:?}",
            want_names.len()
        );
        walked.push((region, entries));
    }

    // A Region this account holds no snapshot in is told so, rather than handed
    // another Region's.
    let (none_held, next) = snapshot_page_in(&emulator, Some(REGION_HOLDING_NONE), None).await;
    assert!(
        none_held.is_empty(),
        "case listing the snapshots {REGION_HOLDING_NONE} holds: expected none, got {none_held:?}"
    );
    assert_eq!(
        next, None,
        "case listing the snapshots {REGION_HOLDING_NONE} holds: expected no next page token, got \
         {next:?}"
    );

    // A read must not change what a later caller sees.
    for (region, first) in &walked {
        let (again, _) = snapshot_page_in(&emulator, Some(region), None).await;
        assert_eq!(
            &again, first,
            "case listing {region} twice: a read must not change what a later caller sees, got \
             {again:?}"
        );
    }
}

/// RULE: an account holding no record of the listed kind is told so — an EMPTY
/// list, not an absent one, and no next page token.
#[tokio::test]
async fn an_account_with_no_disk_snapshots_lists_an_empty_array() {
    let emulator = Emulator::new();
    let body = emulator.ok("GetDiskSnapshots", json!({})).await;
    assert_eq!(
        body["diskSnapshots"],
        json!([]),
        "case an empty account: expected an empty snapshot list, got {body:?}"
    );
    assert!(
        body["nextPageToken"].is_null(),
        "case an empty account: expected no next page token, got {body:?}"
    );

    // A disk with no snapshot taken of it is still no snapshot: the listing
    // reports snapshots, not sources.
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": "unsnapshotted",
                "availabilityZone": "us-east-1a",
                "sizeInGb": 8,
            }),
        )
        .await;
    let body = emulator.ok("GetDiskSnapshots", json!({})).await;
    assert_eq!(
        body["diskSnapshots"],
        json!([]),
        "case a disk with no snapshot: expected an empty snapshot list, got {body:?}"
    );
}

/// RULE: the listing is paged — a page carries at most one page size of records
/// in name order, `nextPageToken` NAMES the first record of the page after it,
/// and the pages together are every record exactly once.
#[tokio::test]
async fn the_listing_pages_in_name_order() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": "paging-source",
                "availabilityZone": "us-east-1b",
                "sizeInGb": 16,
            }),
        )
        .await;

    // Two full pages and a partial one — the count is deliberately not a
    // multiple of the page size. The names are created in DESCENDING order so
    // the listing's order is the STORE's, not the order of creation.
    let total = PAGE_SIZE * 2 + 3;
    let mut names: Vec<String> = (0..total).map(|index| format!("snap-{index:03}")).collect();
    for name in names.iter().rev() {
        emulator
            .ok(
                "CreateDiskSnapshot",
                json!({"diskName": "paging-source", "diskSnapshotName": name}),
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
        let page_names = names_of(&entries);
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
                    "case page {pages}: the next page token must NAME the first record of the \
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
        "case walking every page: {total} snapshots at {PAGE_SIZE} per page is 3 pages, got {pages}"
    );
}

/// RULE: the listing answers from the store, so a record the region no longer
/// holds is no longer reported while every other record still is — and a page
/// token names a record rather than an index, so a delete between two pages
/// cannot make the walk skip or repeat another record.
#[tokio::test]
async fn a_deleted_snapshot_leaves_the_listing() {
    let emulator = Emulator::new();
    // The source — and so every snapshot taken of it — is in a Region of its own,
    // which is the Region this case's walks are made in.
    let region = "eu-central-1";
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": "delete-source",
                "availabilityZone": "eu-central-1a",
                "sizeInGb": 8,
            }),
        )
        .await;
    // More than one page, so a delete is exercised across a page boundary too.
    let total = PAGE_SIZE + 4;
    let names: Vec<String> = (0..total).map(|index| format!("keep-{index:03}")).collect();
    for name in &names {
        emulator
            .ok(
                "CreateDiskSnapshot",
                json!({"diskName": "delete-source", "diskSnapshotName": name}),
            )
            .await;
    }

    // Delete one record of the first page and one of the second.
    let gone = [names[2].clone(), names[PAGE_SIZE + 1].clone()];
    for name in &gone {
        emulator
            .ok("DeleteDiskSnapshot", json!({"diskSnapshotName": name}))
            .await;
    }

    let want: Vec<String> = names
        .iter()
        .filter(|name| !gone.contains(name))
        .cloned()
        .collect();
    let walked = names_of(&all_snapshots_in(&emulator, Some(region)).await);
    assert_eq!(
        walked, want,
        "case after deleting {gone:?}: expected every remaining snapshot exactly once in name \
         order, got {walked:?}"
    );
}

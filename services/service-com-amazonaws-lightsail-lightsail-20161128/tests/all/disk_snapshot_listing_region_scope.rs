// SPDX-License-Identifier: CC0-1.0

//! The REGION a `GetDiskSnapshots` listing reports, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The `Authorization` header carries only a credential SCOPE, which is
//! what names the Region a request is made in.
//!
//! The rule under test: a listing whose own modelled documentation scopes it to
//! the Region the request is made in reports exactly the stored records whose own
//! location is that Region — `GetDiskSnapshots` returns "all block storage disk
//! snapshots in your AWS account and region" — every one of them, once each, in
//! the store's own total order by name and paged, so a Region holding none is told
//! it holds none, and a `pageToken` naming a record of ANOTHER Region names no
//! page of this listing and is refused. A snapshot's Region is the one its stored
//! location carries, taken from the source it was captured from, which is why
//! `CopySnapshot` is what moves one between Regions. The sibling listing the same
//! modelled sentence scopes, `GetDisks`, is pinned by
//! `disk_listing_region_scope.rs`.
//!
//! The rows span what that rule generalises over: how many snapshots a Region
//! holds (more than one page, a handful, exactly one, none), WHICH Region it is
//! (the default one a request carrying no scope is served in, and three others on
//! three continents), the size of the snapshotted volume, and how the names of one
//! Region's snapshots sit among the others' — the names are handed out round-robin
//! across the Regions, so every Region's records are interleaved with every
//! other's in the one name-ordered store and a listing that walked the store
//! without selecting a Region would fail every row. The two source kinds the
//! modelled `CreateDiskSnapshot` input admits (a block storage disk, an instance's
//! system volume) and a snapshot that got its Region from a COPY are cases of their
//! own.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state;

// The harness, the wire error code and the settled-snapshot wait this test target
// already exposes: a sibling suite drives the service through the same front door
// rather than standing up a second one.
use crate::instance_lifecycle::{error_code, Emulator};
use crate::paging::{expected_page_sizes, names, walk_pages};
use crate::snapshot_copy::{wait_for_snapshot_state, Kind};

/// How many snapshots one page of `GetDiskSnapshots` carries — the handler's OWN
/// constant, so the rows keep straddling the page boundary whatever it is set to.
const PAGE_SIZE: usize = state::DISK_SNAPSHOT_PAGE_SIZE;

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One Region's worth of snapshots: the Region, and how many snapshots this
/// account holds there.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    count: usize,
}

/// The table.
///
/// Every Region named here is an entry of the service's own Region catalog
/// (`state::REGION_SPECS`, what `GetRegions` reports), and the first row is the
/// Region a request carrying no credential scope is served in
/// (`state::DEFAULT_REGION`) — asserted below rather than assumed.
const ROWS: &[Row] = &[
    Row {
        case: "the Region an unscoped request is served in, more than one page",
        region: "us-east-1",
        count: PAGE_SIZE + 2,
    },
    Row {
        case: "another Region on the same continent, a handful",
        region: "us-west-2",
        count: 3,
    },
    Row {
        case: "a Region on another continent, exactly one",
        region: "eu-west-1",
        count: 1,
    },
    Row {
        case: "a Region this account holds no snapshot in",
        region: "ap-northeast-1",
        count: 0,
    },
];

/// The zones the service's own Region catalog publishes for `region` — the
/// `GetRegions` catalog, so this suite hand-writes no zone name.
fn zones_of(region: &str) -> Vec<String> {
    let spec = state::REGION_SPECS
        .iter()
        .find(|spec| spec.name == region)
        .unwrap_or_else(|| panic!("case stocking {region}: expected the Region catalog to serve it"));
    let zones = spec.zone_names();
    assert!(
        !zones.is_empty(),
        "case stocking {region}: expected the catalog to publish a zone for it"
    );
    zones
}

/// One snapshot this suite creates: its name, the name and zone of the disk it is
/// taken of, the Region that zone belongs to and the size the disk was asked for.
#[derive(Debug, Clone)]
struct Planned {
    name: String,
    source_disk: String,
    region: &'static str,
    zone: String,
    size_in_gb: i64,
}

/// The snapshots every row holds together, named in ONE sequence handed out
/// round-robin across the rows.
///
/// So the store's name order interleaves the Regions: `rs-snapshot-00` is the
/// first row's, `rs-snapshot-01` the second's, and no Region's records are
/// contiguous in the one name-keyed map every disk snapshot of the account lives
/// in.
fn plan() -> Vec<Planned> {
    let mut left: Vec<usize> = ROWS.iter().map(|row| row.count).collect();
    let mut planned: Vec<Planned> = Vec::new();
    while left.iter().any(|remaining| *remaining > 0) {
        for (index, row) in ROWS.iter().enumerate() {
            if left[index] == 0 {
                continue;
            }
            left[index] -= 1;
            let order = planned.len();
            let zones = zones_of(row.region);
            planned.push(Planned {
                // Zero-padded, so name order is the order they were handed out.
                name: format!("rs-snapshot-{order:02}"),
                source_disk: format!("rs-source-disk-{order:02}"),
                region: row.region,
                // Each source of a Region in the next of that Region's zones, so
                // a row spans the zones its Region publishes.
                zone: zones[order % zones.len()].clone(),
                // The sizes differ between neighbours, so every record carries
                // something of its own.
                size_in_gb: 8 + (order as i64 % 4) * 16,
            });
        }
    }
    planned
}

/// Create every planned snapshot and the disk it is taken of, IN REVERSE NAME
/// ORDER, so a listing that echoed the order the records were created in rather
/// than the store's own order would fail.
///
/// Each create carries no credential scope at all: a disk's Region comes from the
/// `availabilityZone` the request names, and a snapshot keeps the Region of the
/// source it was taken of, so one unscoped caller can stock every Region of the
/// table.
async fn stock(emulator: &Emulator, planned: &[Planned]) {
    for snapshot in planned.iter().rev() {
        emulator
            .ok(
                "CreateDisk",
                json!({
                    "diskName": snapshot.source_disk,
                    "availabilityZone": snapshot.zone,
                    "sizeInGb": snapshot.size_in_gb,
                }),
            )
            .await;
        emulator
            .ok(
                "CreateDiskSnapshot",
                json!({
                    "diskName": snapshot.source_disk,
                    "diskSnapshotName": snapshot.name,
                }),
            )
            .await;
    }
}

/// The names the plan puts in `region`, in the store's own name order.
fn held_in(planned: &[Planned], region: &str) -> Vec<String> {
    let mut held: Vec<String> = planned
        .iter()
        .filter(|snapshot| snapshot.region == region)
        .map(|snapshot| snapshot.name.clone())
        .collect();
    held.sort();
    held
}

/// Every snapshot `GetDiskSnapshots` reports in `region`, walked page by page
/// under the shared checked walk — which asserts the paging contract as it goes.
async fn listing_in(
    emulator: &Emulator,
    case: &str,
    region: Option<&str>,
) -> (Vec<Value>, Vec<usize>) {
    walk_pages(
        emulator,
        case,
        region,
        "GetDiskSnapshots",
        "diskSnapshots",
        PAGE_SIZE,
    )
    .await
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: `GetDiskSnapshots` reports the disk snapshots of the Region the request
/// was made in, and no other Region's — every one of them, in the store's own name
/// order, paged.
#[tokio::test(flavor = "current_thread")]
async fn get_disk_snapshots_reports_the_snapshots_of_the_region_the_request_was_made_in() {
    let emulator = Emulator::new();
    let planned = plan();
    stock(&emulator, &planned).await;

    let mut reported_everywhere: Vec<String> = Vec::new();
    for row in ROWS {
        let case = row.case;
        let held = held_in(&planned, row.region);
        assert_eq!(
            held.len(),
            row.count,
            "case {case}: expected the plan to hold {} snapshots in {}, got {held:?}",
            row.count,
            row.region
        );

        let (records, page_sizes) = listing_in(&emulator, case, Some(row.region)).await;
        let reported = names(&records);
        assert_eq!(
            reported, held,
            "case {case}: expected exactly {}'s snapshots in name order, got {reported:?}",
            row.region
        );
        assert_eq!(
            page_sizes,
            expected_page_sizes(row.count, PAGE_SIZE),
            "case {case}: expected full pages until the remainder, got {page_sizes:?}"
        );

        // Every reported entry IS a record of this Region: the location it
        // carries, and the zone inside it, are the Region's own.
        for record in &records {
            assert_eq!(
                record["location"]["regionName"].as_str(),
                Some(row.region),
                "case {case}: expected every entry to report its own Region, got {record:?}"
            );
            let zone = record["location"]["availabilityZone"]
                .as_str()
                .unwrap_or_default();
            assert!(
                zones_of(row.region).iter().any(|published| published == zone),
                "case {case}: expected a zone {} publishes, got {record:?}",
                row.region
            );
        }
        reported_everywhere.extend(reported);
    }

    // Nothing was lost on the way: the Regions of the table together report every
    // snapshot the account holds, each exactly once.
    let mut every_name: Vec<String> = planned
        .iter()
        .map(|snapshot| snapshot.name.clone())
        .collect();
    every_name.sort();
    reported_everywhere.sort();
    assert_eq!(
        reported_everywhere, every_name,
        "case the Regions together: expected every created snapshot once, got \
         {reported_everywhere:?}"
    );
    let distinct: BTreeSet<&String> = reported_everywhere.iter().collect();
    assert_eq!(
        distinct.len(),
        reported_everywhere.len(),
        "case the Regions together: expected every snapshot reported once, got \
         {reported_everywhere:?}"
    );
}

/// RULE (the same one, for the request that names no Region): a request carrying
/// no credential scope is served in the service's default Region, so it is
/// answered the snapshots of that Region alone.
#[tokio::test(flavor = "current_thread")]
async fn a_request_with_no_credential_scope_is_served_in_the_default_region() {
    let emulator = Emulator::new();
    let planned = plan();
    stock(&emulator, &planned).await;

    let (unscoped, unscoped_pages) = listing_in(&emulator, "a request with no scope", None).await;
    let (default_region, default_pages) = listing_in(
        &emulator,
        "a request scoped to the default Region",
        Some(state::DEFAULT_REGION),
    )
    .await;
    assert_eq!(
        unscoped, default_region,
        "case a request with no scope: expected the default Region's snapshots, got {:?}",
        names(&unscoped)
    );
    assert_eq!(
        unscoped_pages, default_pages,
        "case a request with no scope: expected the default Region's paging, got {unscoped_pages:?}"
    );
    assert_eq!(
        names(&unscoped),
        held_in(&planned, state::DEFAULT_REGION),
        "case a request with no scope: expected exactly the default Region's names, got {:?}",
        names(&unscoped)
    );
}

/// RULE (the same one, where the Region holds nothing): a Region this account
/// holds no disk snapshot in is told so with an EMPTY list and no page token, never
/// with another Region's records.
#[tokio::test(flavor = "current_thread")]
async fn a_region_holding_no_snapshot_reports_an_empty_list() {
    let emulator = Emulator::new();
    let planned = plan();
    stock(&emulator, &planned).await;

    // Every Region the service serves that the table stocks nothing in — so the
    // case is the whole set of them, not one chosen Region.
    let stocked: BTreeSet<&str> = ROWS
        .iter()
        .filter(|row| row.count > 0)
        .map(|row| row.region)
        .collect();
    let empty: Vec<&str> = state::REGION_SPECS
        .iter()
        .map(|spec| spec.name)
        .filter(|name| !stocked.contains(name))
        .collect();
    assert!(
        !empty.is_empty(),
        "case the Regions holding nothing: expected the catalog to serve more Regions than the \
         table stocks, got {empty:?}"
    );

    for region in empty {
        let case = format!("the Region {region}, which holds no snapshot");
        let body = emulator
            .ok_at(None, Some(region), "GetDiskSnapshots", json!({}))
            .await;
        assert_eq!(
            body["diskSnapshots"],
            json!([]),
            "case {case}: expected an empty diskSnapshots array, got {body:?}"
        );
        assert!(
            body["nextPageToken"].is_null(),
            "case {case}: expected no next page token, got {body:?}"
        );
    }
}

/// RULE (the same one, through the page token): a `pageToken` names the snapshot a
/// page opens at, so a token naming a snapshot of ANOTHER Region names no page of
/// this listing and is refused with the modelled 400 `InvalidInputException`
/// rather than resumed at the next name after it.
#[tokio::test(flavor = "current_thread")]
async fn a_page_token_naming_another_regions_snapshot_is_refused() {
    let emulator = Emulator::new();
    let planned = plan();
    stock(&emulator, &planned).await;

    for row in ROWS {
        let held = held_in(&planned, row.region);

        // Every name this Region does NOT hold is a token it cannot answer —
        // including the names that sort immediately around its own, which is what
        // a walk that ignored the Region would resume at.
        for other in &planned {
            if held.iter().any(|name| name == &other.name) {
                continue;
            }
            let case = format!(
                "{} in {}, resuming from {}'s snapshot {}",
                row.case, row.region, other.region, other.name
            );
            let (status, body) = emulator
                .call_in(
                    Some(row.region),
                    "GetDiskSnapshots",
                    json!({"pageToken": other.name}),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected the invalid-input status, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {case}: expected the modelled InvalidInputException, got {body:?}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&other.name),
                "case {case}: expected the refusal to name the token sent, got {body:?}"
            );
            assert!(
                message.contains("pageToken"),
                "case {case}: expected the refusal to name the member it is about, got {body:?}"
            );
        }

        // And a token that IS one of this Region's own names opens the page at it,
        // carrying this Region's snapshots from there onward and nothing else.
        for (index, token) in held.iter().enumerate() {
            let case = format!("{} in {}, resuming from {token}", row.case, row.region);
            let body = emulator
                .ok_at(
                    None,
                    Some(row.region),
                    "GetDiskSnapshots",
                    json!({"pageToken": token}),
                )
                .await;
            let page = body["diskSnapshots"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected a diskSnapshots array, got {body:?}"))
                .clone();
            let expected: Vec<String> = held[index..].iter().take(PAGE_SIZE).cloned().collect();
            assert_eq!(
                names(&page),
                expected,
                "case {case}: expected this Region's names from there onward, got {body:?}"
            );
            assert_eq!(
                body["nextPageToken"].as_str(),
                held.get(index + PAGE_SIZE).map(String::as_str),
                "case {case}: expected the next token to name this Region's next snapshot, got \
                 {body:?}"
            );
        }
    }
}

/// RULE (the same one, where the snapshot's source is an instance's SYSTEM
/// VOLUME): what decides is the snapshot's own stored location, which it takes
/// from the source it was captured from — whatever kind of source that is.
#[tokio::test(flavor = "current_thread")]
async fn a_system_volume_snapshot_is_listed_in_the_region_of_its_instance() {
    let emulator = Emulator::new();
    let region = "ap-southeast-2";
    let zone = zones_of(region).remove(0);
    let instance = "rs-system-volume-host";
    let snapshot = "rs-system-volume-snapshot";

    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [instance],
                "availabilityZone": zone,
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"instanceName": instance, "diskSnapshotName": snapshot}),
        )
        .await;

    let case = format!("a system volume snapshot taken in {region}");
    let (records, _) = listing_in(&emulator, &case, Some(region)).await;
    assert_eq!(
        names(&records),
        vec![snapshot.to_string()],
        "case {case}: expected the Region of the instance to report it, got {records:?}"
    );
    assert_eq!(
        records[0]["fromInstanceName"].as_str(),
        Some(instance),
        "case {case}: expected the listed record to name the instance it was taken of, got {:?}",
        records[0]
    );

    let case = format!("a system volume snapshot of {region}, listed from elsewhere");
    let (elsewhere, pages) = listing_in(&emulator, &case, Some(state::DEFAULT_REGION)).await;
    assert!(
        elsewhere.is_empty(),
        "case {case}: expected no record, got {elsewhere:?}"
    );
    assert_eq!(
        pages,
        vec![0],
        "case {case}: expected one empty page and no token, got {pages:?}"
    );
}

/// RULE (the same one, where a snapshot is COPIED): a copy is a snapshot of the
/// Region it was made in, so the listing that reports it is that Region's and the
/// source's listing still reports the source alone. And a listing is a READ, so
/// walking either of them again reports the same records.
#[tokio::test(flavor = "current_thread")]
async fn a_snapshot_copied_into_another_region_is_listed_there() {
    let emulator = Emulator::new();
    let source_region = "ca-central-1";
    let source_zone = zones_of(source_region).remove(0);
    let target_region = "eu-central-1";
    let disk = "rs-copy-source-disk";
    let source_snapshot = "rs-copy-source-snapshot";
    let copy = "rs-copied-snapshot";

    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": disk, "availabilityZone": source_zone, "sizeInGb": 8}),
        )
        .await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskName": disk, "diskSnapshotName": source_snapshot}),
        )
        .await;
    // A snapshot is copyable once the service has finished taking it.
    wait_for_snapshot_state(
        &emulator,
        Kind::DiskSnapshot,
        source_snapshot,
        Kind::DiskSnapshot.settled(),
    )
    .await;
    emulator
        .ok_at(
            None,
            Some(target_region),
            "CopySnapshot",
            json!({
                "sourceRegion": source_region,
                "sourceSnapshotName": source_snapshot,
                "targetSnapshotName": copy,
            }),
        )
        .await;

    let case = format!("the snapshots {source_region} holds after the copy");
    let (source_side, _) = listing_in(&emulator, &case, Some(source_region)).await;
    assert_eq!(
        names(&source_side),
        vec![source_snapshot.to_string()],
        "case {case}: expected the source alone, got {source_side:?}"
    );

    let case = format!("the snapshots {target_region} holds after the copy");
    let (target_side, _) = listing_in(&emulator, &case, Some(target_region)).await;
    assert_eq!(
        names(&target_side),
        vec![copy.to_string()],
        "case {case}: expected the copy alone, got {target_side:?}"
    );
    assert_eq!(
        target_side[0]["location"]["regionName"].as_str(),
        Some(target_region),
        "case {case}: expected the copy to be located in the Region it was made in, got {:?}",
        target_side[0]
    );

    let (again_source, _) = listing_in(&emulator, "listing twice", Some(source_region)).await;
    let (again_target, _) = listing_in(&emulator, "listing twice", Some(target_region)).await;
    assert_eq!(
        (again_source, again_target),
        (source_side, target_side),
        "case listing twice: a read must not change what a later caller sees"
    );
}

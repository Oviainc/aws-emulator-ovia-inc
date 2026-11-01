// SPDX-License-Identifier: CC0-1.0

//! The ONE Region-scoped walk both Lightsail block storage listings share
//! (`crate::state::records_in_region_from`), driven over the wire through an
//! operation from each of them.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The `Authorization` header carries only a credential SCOPE, which is
//! what names the Region a request is made in.
//!
//! The rule under test: the two listings whose modelled sentence scopes them to
//! the caller's Region — `GetDisks` ("all block storage disks in your AWS account
//! and region") and `GetDiskSnapshots` ("all block storage disk snapshots in your
//! AWS account and region") — draw the SAME Region boundary over one account,
//! because each reports the records whose own stored location names the Region the
//! request is made in. So a disk and the snapshot taken of it are reported by the
//! same Region's pair of listings and by no other Region's, and neither listing
//! reports a record the other's Region holds.
//!
//! What each listing reports on its own, across the Regions one account holds
//! records in — the counts, the paging, the empty Region, the page token of
//! another Region — is `disk_listing_region_scope.rs` for the disks and
//! `disk_snapshot_listing_region_scope.rs` for the snapshots. This suite is about
//! the two of them AGREEING, which is the property the shared walk is what
//! guarantees.

use std::collections::BTreeMap;

use serde_json::json;
use service_com_amazonaws_lightsail_lightsail_20161128::state;

// The harness this test target already exposes: a sibling suite drives the
// service through the same front door rather than standing up a second one.
use crate::instance_lifecycle::Emulator;
use crate::paging::{names, walk_pages};

/// The Regions this suite stocks, and how many disks (each with one snapshot
/// taken of it) the account holds in each.
///
/// Every Region named here is an entry of the service's own Region catalog
/// (`state::REGION_SPECS`, what `GetRegions` reports); one of them is the Region a
/// request carrying no credential scope is served in, and the counts differ so a
/// listing that answered the wrong Region's records could not pass by accident.
const ROWS: &[(&str, usize)] = &[
    (state::DEFAULT_REGION, 3),
    ("eu-west-1", 2),
    ("ap-southeast-2", 1),
];

/// The zones the service's own Region catalog publishes for `region`, so this
/// suite hand-writes no zone name.
fn zones_of(region: &str) -> Vec<String> {
    state::REGION_SPECS
        .iter()
        .find(|spec| spec.name == region)
        .unwrap_or_else(|| panic!("case stocking {region}: expected the Region catalog to serve it"))
        .zone_names()
}

/// Create every row's disks and one snapshot of each, naming them in ONE sequence
/// handed out round-robin across the Regions so their names interleave in the
/// store's one total order by name; answer, per Region, the disk names and the
/// snapshot names it then holds.
async fn stock(emulator: &Emulator) -> BTreeMap<&'static str, (Vec<String>, Vec<String>)> {
    let mut left: Vec<usize> = ROWS.iter().map(|(_, count)| *count).collect();
    let mut held: BTreeMap<&'static str, (Vec<String>, Vec<String>)> = BTreeMap::new();
    let mut order = 0;
    while left.iter().any(|remaining| *remaining > 0) {
        for (index, (region, _)) in ROWS.iter().enumerate() {
            if left[index] == 0 {
                continue;
            }
            left[index] -= 1;
            // Zero-padded, so name order is the order the names were handed out.
            let disk = format!("bs-disk-{order:02}");
            let snapshot = format!("bs-snapshot-{order:02}");
            let zones = zones_of(region);
            emulator
                .ok(
                    "CreateDisk",
                    json!({
                        "diskName": disk,
                        "availabilityZone": zones[order % zones.len()],
                        // Sizes differ between neighbours, so no two records are
                        // the same record.
                        "sizeInGb": 8 + (order as i64 % 3) * 8,
                    }),
                )
                .await;
            emulator
                .ok(
                    "CreateDiskSnapshot",
                    json!({"diskName": disk, "diskSnapshotName": snapshot}),
                )
                .await;
            let entry = held.entry(region).or_default();
            entry.0.push(disk);
            entry.1.push(snapshot);
            order += 1;
        }
    }
    for (disks, snapshots) in held.values_mut() {
        disks.sort();
        snapshots.sort();
    }
    held
}

/// RULE: both block storage listings report the records of the Region the request
/// was made in, so the pair of them answers one Region with exactly that Region's
/// disks and the snapshots taken of them, and with no record of another Region.
#[tokio::test(flavor = "current_thread")]
async fn both_block_storage_listings_draw_the_same_region_boundary() {
    let emulator = Emulator::new();
    let held = stock(&emulator).await;

    for (region, (want_disks, want_snapshots)) in &held {
        let case = format!("the block storage {region} holds");
        let (disks, _) = walk_pages(
            &emulator,
            &case,
            Some(region),
            "GetDisks",
            "disks",
            state::DISK_PAGE_SIZE,
        )
        .await;
        let (snapshots, _) = walk_pages(
            &emulator,
            &case,
            Some(region),
            "GetDiskSnapshots",
            "diskSnapshots",
            state::DISK_SNAPSHOT_PAGE_SIZE,
        )
        .await;
        assert_eq!(
            names(&disks),
            *want_disks,
            "case {case}: expected this Region's disks in name order, got {:?}",
            names(&disks)
        );
        assert_eq!(
            names(&snapshots),
            *want_snapshots,
            "case {case}: expected this Region's snapshots in name order, got {:?}",
            names(&snapshots)
        );

        // The two listings agree: every snapshot this Region reports was taken of
        // a disk this Region reports too, and both carry this Region's location.
        let reported_disks = names(&disks);
        for snapshot in &snapshots {
            let source = snapshot["fromDiskName"].as_str().unwrap_or_default();
            assert!(
                reported_disks.iter().any(|disk| disk == source),
                "case {case}: expected the source disk {source} to be reported by the same \
                 Region's disk listing, got {reported_disks:?}"
            );
        }
        for record in disks.iter().chain(snapshots.iter()) {
            assert_eq!(
                record["location"]["regionName"].as_str(),
                Some(*region),
                "case {case}: expected every reported record to be located here, got {record:?}"
            );
        }

        // And no record of any OTHER Region is among them.
        for (other, (other_disks, other_snapshots)) in &held {
            if other == region {
                continue;
            }
            for name in other_disks {
                assert!(
                    !reported_disks.iter().any(|disk| disk == name),
                    "case {case}: expected the disk {name} of {other} to be absent, got \
                     {reported_disks:?}"
                );
            }
            let reported_snapshots = names(&snapshots);
            for name in other_snapshots {
                assert!(
                    !reported_snapshots.iter().any(|snapshot| snapshot == name),
                    "case {case}: expected the snapshot {name} of {other} to be absent, got \
                     {reported_snapshots:?}"
                );
            }
        }
    }
}

/// RULE (the same one, read from the account's side): the Regions together report
/// every record the account holds, each exactly once — the Region scoping narrows
/// which listing reports a record, and loses none of them.
#[tokio::test(flavor = "current_thread")]
async fn the_regions_together_report_every_block_storage_record_once() {
    let emulator = Emulator::new();
    let held = stock(&emulator).await;

    let mut want_disks: Vec<String> = Vec::new();
    let mut want_snapshots: Vec<String> = Vec::new();
    let mut reported_disks: Vec<String> = Vec::new();
    let mut reported_snapshots: Vec<String> = Vec::new();
    for (region, (disks, snapshots)) in &held {
        want_disks.extend(disks.iter().cloned());
        want_snapshots.extend(snapshots.iter().cloned());
        let case = format!("walking {region}");
        let (listed_disks, _) = walk_pages(
            &emulator,
            &case,
            Some(region),
            "GetDisks",
            "disks",
            state::DISK_PAGE_SIZE,
        )
        .await;
        let (listed_snapshots, _) = walk_pages(
            &emulator,
            &case,
            Some(region),
            "GetDiskSnapshots",
            "diskSnapshots",
            state::DISK_SNAPSHOT_PAGE_SIZE,
        )
        .await;
        reported_disks.extend(names(&listed_disks));
        reported_snapshots.extend(names(&listed_snapshots));
    }
    want_disks.sort();
    want_snapshots.sort();
    reported_disks.sort();
    reported_snapshots.sort();
    assert_eq!(
        reported_disks, want_disks,
        "case every Region together: expected every created disk exactly once, got \
         {reported_disks:?}"
    );
    assert_eq!(
        reported_snapshots, want_snapshots,
        "case every Region together: expected every created snapshot exactly once, got \
         {reported_snapshots:?}"
    );
}

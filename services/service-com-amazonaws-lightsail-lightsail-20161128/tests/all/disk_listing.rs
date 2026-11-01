// SPDX-License-Identifier: CC0-1.0

//! `GetDisks` — the paged listing of the block storage disks an account holds —
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a listing reports the records the store holds of that
//! resource type IN THE REGION THE REQUEST WAS MADE IN, each entry exactly as the
//! single-record read reports it, in the store's own name order, carried one page
//! at a time, with `nextPageToken` NAMING the first record of the page after it
//! while records remain and ABSENT on the last page — and, being a read, it
//! reports the same records however often it is called. Every case here holds its
//! disks in one Region, the one every call of this suite is served in; the Region
//! scope itself is the subject of `disk_listing_region_scope.rs`.
//!
//! The rows span what that rule generalises over: how many disks the Region
//! holds (none, one, exactly a page, one past a page, several pages and a
//! remainder), the creation order against the reported order (every row is
//! created in reverse name order, so a listing that merely echoed the creation
//! sequence would fail), and the size, zone and tags each record was asked for —
//! so the server-assigned members that are DERIVED from them (IOPS, the location
//! and the region the ARN carries) are computed per entry and a canned body could
//! not satisfy them.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state;

// The harness this test target already exposes: a sibling suite drives the
// service through the same front door rather than standing up a second one.
use crate::instance_lifecycle::{error_code, Emulator};

/// How many disks one page of `GetDisks` carries — the handler's OWN constant, so
/// the table's rows keep straddling the page boundary whatever it is set to.
const PAGE_SIZE: usize = state::DISK_PAGE_SIZE;

/// The sizes, in GB, the rows ask for. Every one is inside the range the service
/// creates a disk in (`crate::state::MIN_DISK_SIZE_IN_GB` ..=
/// `MAX_DISK_SIZE_IN_GB`), and they straddle the IOPS floor
/// (`crate::state::disk_iops`), so neighbouring entries differ in both the size
/// they carry and the IOPS derived from it.
const SIZES_IN_GB: &[i64] = &[8, 16, 33, 64, 128, 1024];

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Every availability zone the service itself publishes for the Region every call
/// of this suite is served in, in report order.
///
/// SOURCE: the service's own Region catalog — `GetRegions` with
/// `includeAvailabilityZones`, which reports the zones of the Region the request
/// was made in — so no zone is hand-written in this suite and the rows span the
/// zones that Region actually serves.
async fn published_zones(emulator: &Emulator) -> Vec<String> {
    let body = emulator
        .ok("GetRegions", json!({"includeAvailabilityZones": true}))
        .await;
    let regions = body["regions"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading the Region catalog: expected an array, got {body:?}"))
        .clone();
    let mut zones = Vec::new();
    for region in regions {
        let published = region["availabilityZones"].as_array().cloned().unwrap_or_default();
        for zone in published {
            if let Some(name) = zone["zoneName"].as_str() {
                zones.push(name.to_string());
            }
        }
    }
    assert!(
        !zones.is_empty(),
        "case reading the Region catalog: expected a non-empty list of availability zones"
    );
    zones
}

/// One `GetDisks` page, resuming from `page_token` when one is given.
async fn page(emulator: &Emulator, page_token: Option<&str>) -> Value {
    let body = match page_token {
        Some(token) => json!({"pageToken": token}),
        None => json!({}),
    };
    emulator.ok("GetDisks", body).await
}

/// The entries a page reports, in the order it reports them.
fn entries(page: &Value) -> Vec<Value> {
    page["disks"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a disks array, got {page:?}"))
        .clone()
}

/// The names a page reports, in the order it reports them.
fn names(page: &Value) -> Vec<String> {
    entries(page)
        .iter()
        .map(|disk| {
            disk["name"]
                .as_str()
                .unwrap_or_else(|| panic!("expected every entry to carry a name, got {page:?}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of the listing, answering the records in the order reported
/// and the size of each page.
///
/// Checks the paging contract as it goes: no page carries more than `PAGE_SIZE`
/// records, a `nextPageToken` names the FIRST record of the page after it, a page
/// that is not the last one is full, and the walk terminates because the token is
/// absent rather than because it ran out of patience.
async fn walk(emulator: &Emulator, case: &str, max_pages: usize) -> (Vec<Value>, Vec<usize>) {
    let mut records = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    for page_index in 0..max_pages {
        let body = page(emulator, token.as_deref()).await;
        let page_names = names(&body);
        assert!(
            page_names.len() <= PAGE_SIZE,
            "case {case}: page {page_index} carries more than the {PAGE_SIZE} of a page, got \
             {page_names:?}"
        );
        if let Some(token) = &token {
            assert_eq!(
                page_names.first().map(String::as_str),
                Some(token.as_str()),
                "case {case}: a page token names the first record of the page after it, got \
                 {page_names:?}"
            );
        }
        page_sizes.push(page_names.len());
        records.extend(entries(&body));
        match body["nextPageToken"].as_str() {
            Some(next) => {
                assert_eq!(
                    page_names.len(),
                    PAGE_SIZE,
                    "case {case}: a page that is not the last one is full, got {page_names:?}"
                );
                token = Some(next.to_string());
            }
            None => return (records, page_sizes),
        }
    }
    panic!("case {case}: the walk did not terminate within {max_pages} pages");
}

/// The stored disk `name`, as `GetDisk` reports it.
async fn read_disk(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetDisk", json!({"diskName": name})).await["disk"].clone()
}

/// Poll `GetDisk` until `name` reports `available`.
///
/// The transition is owed by the clock, not by the poll: waiting for it here
/// means the listing and the single-record read are compared in the SAME
/// lifecycle state, so a difference between them is a difference in what the two
/// operations report, never a transition that landed between the two calls.
async fn wait_for_available(emulator: &Emulator, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let disk = read_disk(emulator, name).await;
        if disk["state"].as_str() == Some("available") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for 'available' on {name}: expected it within 10s, got {:?}",
            disk["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The listing table
// ---------------------------------------------------------------------------

/// One Region's worth of disks: how many it holds, and the name prefix that
/// tells this row's records apart.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    prefix: &'static str,
    count: usize,
}

/// The table. The counts straddle every page boundary the rule can turn on.
const ROWS: &[Row] = &[
    Row {
        case: "a Region holding nothing",
        prefix: "empty",
        count: 0,
    },
    Row {
        case: "a Region holding one",
        prefix: "solo",
        count: 1,
    },
    Row {
        case: "exactly one page",
        prefix: "full",
        count: PAGE_SIZE,
    },
    Row {
        case: "one past a page",
        prefix: "spill",
        count: PAGE_SIZE + 1,
    },
    Row {
        case: "several pages and a remainder",
        prefix: "wide",
        count: 2 * PAGE_SIZE + 3,
    },
];

/// The name of this row's disk `index`. Zero-padded, so name order is index order
/// however many a row holds.
fn name_of(row: &Row, index: usize) -> String {
    format!("{}-disk-{index:02}", row.prefix)
}

/// The page sizes a listing of `total` records must report: full pages until the
/// remainder, and always at least one page — an empty account still answers once.
fn expected_page_sizes(total: usize) -> Vec<usize> {
    if total == 0 {
        return vec![0];
    }
    let mut sizes = Vec::new();
    let mut left = total;
    while left > 0 {
        let page = left.min(PAGE_SIZE);
        sizes.push(page);
        left -= page;
    }
    sizes
}

/// What one created disk was asked for: its name, the zone it was created in, its
/// size in GB and the tag value it carries.
#[derive(Debug, Clone)]
struct Created {
    name: String,
    zone: String,
    size_in_gb: i64,
    tag_value: String,
}

/// Create this row's disks, IN REVERSE NAME ORDER, each in its own zone out of the
/// zones this request's Region publishes, at its own size and with its own tag,
/// and wait until every one is `available`.
async fn create_row(emulator: &Emulator, row: &Row) -> Vec<Created> {
    let zones = published_zones(emulator).await;

    let mut created = Vec::new();
    for index in (0..row.count).rev() {
        let name = name_of(row, index);
        let zone = zones[index % zones.len()].clone();
        let size_in_gb = SIZES_IN_GB[index % SIZES_IN_GB.len()];
        let tag_value = format!("tier-{index}");
        emulator
            .ok(
                "CreateDisk",
                json!({
                    "diskName": name,
                    "availabilityZone": zone,
                    "sizeInGb": size_in_gb,
                    "tags": [{"key": "workload", "value": tag_value}],
                }),
            )
            .await;
        created.push(Created {
            name,
            zone,
            size_in_gb,
            tag_value,
        });
    }
    for disk in &created {
        wait_for_available(emulator, &disk.name).await;
    }
    created.sort_by(|left, right| left.name.cmp(&right.name));
    created
}

/// The IOPS a disk of `size_in_gb` must report: the EBS general purpose SSD
/// (`gp2`) baseline of 3 IOPS per GB, never below 100 and never above 16,000.
fn want_iops(size_in_gb: i64) -> i64 {
    (size_in_gb * 3).clamp(100, 16_000)
}

#[tokio::test(flavor = "current_thread")]
async fn get_disks_reports_every_stored_disk_in_name_order() {
    for row in ROWS {
        // One account per row: the store keys a disk by name alone, so a row
        // reads a store holding exactly what that row created.
        let emulator = Emulator::new();
        let case = row.case;
        let created = create_row(&emulator, row).await;

        let (records, page_sizes) = walk(&emulator, case, row.count / PAGE_SIZE + 3).await;

        // The listing reports every stored record, in the store's name order.
        let reported: Vec<String> = records
            .iter()
            .map(|disk| {
                disk["name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: expected a name, got {disk:?}"))
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = created.iter().map(|disk| disk.name.clone()).collect();
        assert_eq!(
            reported, expected,
            "case {case}: expected every stored name in name order, got {reported:?}"
        );

        // Each record is reported once; a page boundary neither repeats nor skips
        // one.
        let distinct: BTreeSet<&String> = reported.iter().collect();
        assert_eq!(
            distinct.len(),
            reported.len(),
            "case {case}: expected every record once, got {reported:?}"
        );

        // The pages are full until the remainder.
        assert_eq!(
            page_sizes,
            expected_page_sizes(row.count),
            "case {case}: expected full pages until the remainder, got {page_sizes:?}"
        );

        for (record, asked) in records.iter().zip(&created) {
            let name = &asked.name;
            // Every listed entry is exactly what the single-record read reports:
            // one stored record, two operations.
            let single = read_disk(&emulator, name).await;
            assert_eq!(
                record, &single,
                "case {case}: the listing of {name} must match GetDisk, got {record:?}"
            );
            // And it carries THIS record's request, not a canned body.
            assert_eq!(
                record["sizeInGb"].as_i64(),
                Some(asked.size_in_gb),
                "case {case}: expected {name} to report the size it was created with, got \
                 {record:?}"
            );
            assert_eq!(
                record["location"]["availabilityZone"].as_str(),
                Some(asked.zone.as_str()),
                "case {case}: expected {name} to report the zone it was created in, got {record:?}"
            );
            assert_eq!(
                record["tags"],
                json!([{"key": "workload", "value": asked.tag_value}]),
                "case {case}: expected {name} to report the tags it was created with, got \
                 {record:?}"
            );
            // The members the service DERIVES from that request, each computed
            // from this record rather than fixed.
            assert_eq!(
                record["iops"].as_i64(),
                Some(want_iops(asked.size_in_gb)),
                "case {case}: expected {name} to report the IOPS of its size, got {record:?}"
            );
            let region = asked
                .zone
                .strip_suffix(|c: char| c.is_ascii_alphabetic())
                .unwrap_or(asked.zone.as_str());
            let arn = record["arn"].as_str().unwrap_or_default();
            assert!(
                arn.starts_with(&format!("arn:aws:lightsail:{region}:")),
                "case {case}: expected {name} to report an ARN in the region of its zone, got \
                 {record:?}"
            );
            assert_eq!(
                record["resourceType"].as_str(),
                Some("Disk"),
                "case {case}: expected {name} to report its resource type, got {record:?}"
            );
            assert_eq!(
                record["isSystemDisk"].as_bool(),
                Some(false),
                "case {case}: expected {name} to report that it is not a system disk, got \
                 {record:?}"
            );
            assert_eq!(
                record["isAttached"].as_bool(),
                Some(false),
                "case {case}: expected {name} to report that nothing has attached it, got \
                 {record:?}"
            );
        }
    }
}

/// A listing is a READ: it reports the same records, with the same
/// server-assigned identities, however often it is called.
#[tokio::test(flavor = "current_thread")]
async fn listing_twice_reports_the_same_records() {
    let emulator = Emulator::new();
    let row = Row {
        case: "repeated listing",
        prefix: "repeat",
        count: PAGE_SIZE + 2,
    };
    create_row(&emulator, &row).await;

    let (first, first_sizes) = walk(&emulator, "first listing", 5).await;
    let (second, second_sizes) = walk(&emulator, "second listing", 5).await;
    assert_eq!(
        first, second,
        "case repeated listing: a read must not change what a later caller sees, got {second:?}"
    );
    assert_eq!(
        first_sizes, second_sizes,
        "case repeated listing: expected the same paging, got {second_sizes:?}"
    );
}

/// An empty listing is an empty LIST with no token, never an absent member: the
/// caller asked what it holds and the answer is "nothing". A page token, though,
/// names a disk the account does not hold, so it is refused rather than answered
/// with an empty page the caller cannot tell from that one.
#[tokio::test(flavor = "current_thread")]
async fn an_account_holding_nothing_reports_an_empty_list_and_no_token() {
    let emulator = Emulator::new();
    for token in [None, Some("")] {
        let body = page(&emulator, token).await;
        assert_eq!(
            entries(&body).len(),
            0,
            "case an empty account at {token:?}: expected an empty disks array, got {body:?}"
        );
        assert!(
            body["nextPageToken"].is_null(),
            "case an empty account at {token:?}: a last page carries no nextPageToken, got \
             {body:?}"
        );
    }

    let (status, body) = emulator
        .call("GetDisks", json!({"pageToken": "anything"}))
        .await;
    assert_eq!(
        status, 400,
        "case an empty account at a token naming no disk: expected the invalid-input status, got \
         {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case an empty account at a token naming no disk: expected the modelled \
         InvalidInputException, got {body:?}"
    );
}

/// A page token NAMES a record rather than carrying an index, so resuming from
/// any stored name reports that name onward — the invariant that keeps a record
/// created or removed between two pages from making the walk skip or repeat
/// another one.
#[tokio::test(flavor = "current_thread")]
async fn resuming_from_a_stored_name_reports_that_name_onward() {
    let emulator = Emulator::new();
    let row = Row {
        case: "resuming",
        prefix: "resume",
        count: PAGE_SIZE + 4,
    };
    let created = create_row(&emulator, &row).await;
    let all: Vec<String> = created.iter().map(|disk| disk.name.clone()).collect();

    for (index, token) in all.iter().enumerate() {
        let body = page(&emulator, Some(token)).await;
        let reported = names(&body);
        let expected: Vec<String> = all[index..].iter().take(PAGE_SIZE).cloned().collect();
        assert_eq!(
            reported, expected,
            "case resuming from {token}: expected the names from there onward, got {reported:?}"
        );
        let remaining = all.len() - index;
        assert_eq!(
            body["nextPageToken"].as_str(),
            (remaining > PAGE_SIZE).then(|| all[index + PAGE_SIZE].as_str()),
            "case resuming from {token}: expected the next token to name the first record of the \
             page after it, got {body:?}"
        );
    }
}

/// A disk the clock has not settled yet is still a record the account holds, so
/// the listing reports it — in the same state, and with the same members, the
/// single-record read reports.
#[tokio::test(flavor = "current_thread")]
async fn a_disk_still_being_created_is_listed_as_the_single_read_reports_it() {
    let emulator = Emulator::new();
    let name = "pending-disk";
    // Drawn from the service's own Region catalog, as every other case here does,
    // so no zone is hand-written in this suite.
    let zone = published_zones(&emulator).await.remove(0);
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": name, "availabilityZone": zone, "sizeInGb": 8}),
        )
        .await;

    let listed = entries(&page(&emulator, None).await);
    assert_eq!(
        listed.len(),
        1,
        "case listing a disk being created: expected the one stored record, got {listed:?}"
    );
    let state = listed[0]["state"].as_str().unwrap_or_default().to_string();
    assert!(
        state == "pending" || state == "available",
        "case listing a disk being created: expected 'pending' or 'available', got {state:?}"
    );

    // Once the clock has settled it, the listing and the single read agree again.
    wait_for_available(&emulator, name).await;
    let settled = entries(&page(&emulator, None).await);
    let single = read_disk(&emulator, name).await;
    assert_eq!(
        settled,
        vec![single.clone()],
        "case listing a settled disk: expected exactly what GetDisk reports, got {settled:?}"
    );
    assert_eq!(
        single["state"].as_str(),
        Some("available"),
        "case listing a settled disk: expected 'available', got {single:?}"
    );
}

/// A disk an instance has attached is still a record the account holds, and the
/// listing reports the attachment the same way the single read does: the rule is
/// "every record, as the single read reports it", whatever lifecycle state it is
/// in.
#[tokio::test(flavor = "current_thread")]
async fn an_attached_disk_is_listed_with_its_attachment() {
    let emulator = Emulator::new();
    let zone = published_zones(&emulator).await.remove(0);
    let instance = "listing-host";
    // The attachment is what this case is about, so both resources are settled
    // first: an instance must be running and a disk `available` to be attached.
    crate::disk_attachment::create_running_instance(&emulator, instance, &zone).await;

    let disk = "listing-volume";
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": disk, "availabilityZone": zone, "sizeInGb": 16}),
        )
        .await;
    wait_for_available(&emulator, disk).await;
    emulator
        .ok(
            "AttachDisk",
            json!({"diskName": disk, "instanceName": instance, "diskPath": "/dev/xvdf"}),
        )
        .await;

    let listed = entries(&page(&emulator, None).await);
    let entry = listed
        .iter()
        .find(|entry| entry["name"].as_str() == Some(disk))
        .unwrap_or_else(|| panic!("case listing an attached disk: expected {disk}, got {listed:?}"));
    assert_eq!(
        entry,
        &read_disk(&emulator, disk).await,
        "case listing an attached disk: the listing must match GetDisk, got {entry:?}"
    );
    assert_eq!(
        entry["attachedTo"].as_str(),
        Some(instance),
        "case listing an attached disk: expected the instance it is attached to, got {entry:?}"
    );
    assert_eq!(
        entry["isAttached"].as_bool(),
        Some(true),
        "case listing an attached disk: expected it to report the attachment, got {entry:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! `GetStaticIps` — the paged listing of the static IPs an account holds —
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The `Authorization` header carries only a credential SCOPE, which
//! is what names the Region a request is made in.
//!
//! The rule under test: a listing reports every record the store holds of that
//! resource type, each entry exactly as the single-record read reports it, in the
//! store's own name order, carried one page at a time, with `nextPageToken`
//! NAMING the first record of the page after it while records remain and ABSENT
//! on the last page — and, being a read, it reports the same records however
//! often it is called.
//!
//! The rows span what that rule generalises over: how many static IPs the
//! account holds (none, one, exactly a page, one past a page, several pages and a
//! remainder), the creation order against the reported order (every row is
//! allocated in reverse name order, so a listing that merely echoed the
//! allocation sequence would fail), and the Region each allocation was made in —
//! because a static IP belongs to the ACCOUNT, so a listing made in one Region
//! still reports the ones reserved in another, and the server-assigned members
//! DERIVED from the allocating request (the Region in the ARN and the location,
//! the reserved address) differ from entry to entry so a canned body could not
//! satisfy them.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state;

// The harness and the checked page walk this test target already exposes: a
// suite drives the service through the same front door, and asserts what the
// listing reports rather than carrying its own copy of the paging contract.
use crate::instance_lifecycle::{is_modelled_ipv4, Emulator};
use crate::paging::{expected_page_sizes, names, walk_pages};

/// How many static IPs one page of `GetStaticIps` carries — the handler's OWN
/// constant, so the table's rows keep straddling the page boundary whatever it is
/// set to.
const PAGE_SIZE: usize = state::STATIC_IP_PAGE_SIZE;

/// Reads the service's own Region catalog through `GetRegions`, answering the
/// Region names it publishes.
///
/// The rows allocate one static IP per Region out of this list, so neighbouring
/// entries carry different locations and ARNs and nothing here is hand-written.
async fn published_regions(emulator: &Emulator) -> Vec<String> {
    let body = emulator.ok("GetRegions", json!({})).await;
    let regions: Vec<String> = body["regions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case reading the Region catalog: expected an array, got {body:?}")
        })
        .iter()
        .filter_map(|region| region["name"].as_str().map(str::to_string))
        .collect();
    assert!(
        !regions.is_empty(),
        "case reading the Region catalog: expected a non-empty list of regions, got {body:?}"
    );
    regions
}

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// One `GetStaticIps` page, resuming from `page_token` when one is given and made
/// in `region` when one is given.
async fn page(emulator: &Emulator, region: Option<&str>, page_token: Option<&str>) -> Value {
    let body = match page_token {
        Some(token) => json!({"pageToken": token}),
        None => json!({}),
    };
    emulator
        .ok_at(None, region, "GetStaticIps", body)
        .await
}

/// The entries a page reports, in the order it reports them.
fn entries(page: &Value) -> Vec<Value> {
    page["staticIps"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a staticIps array, got {page:?}"))
        .clone()
}

/// Walk every page of this listing in `region`, answering the records in the
/// order reported and the size of each page — the ONE checked walk this test
/// target holds, named for this operation, its output member and its page size.
async fn walk(emulator: &Emulator, case: &str, region: Option<&str>) -> (Vec<Value>, Vec<usize>) {
    walk_pages(emulator, case, region, "GetStaticIps", "staticIps", PAGE_SIZE).await
}

/// The stored static IP `name`, as `GetStaticIp` reports it.
async fn read_static_ip(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetStaticIp", json!({"staticIpName": name}))
        .await["staticIp"]
        .clone()
}

// ---------------------------------------------------------------------------
// The listing table
// ---------------------------------------------------------------------------

/// One account's worth of static IPs: how many it holds, and the name prefix that
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
        case: "an account holding nothing",
        prefix: "empty",
        count: 0,
    },
    Row {
        case: "an account holding one",
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

/// The name of this row's static IP `index`. Zero-padded, so name order is index
/// order however many a row holds.
fn name_of(row: &Row, index: usize) -> String {
    format!("{}-static-ip-{index:02}", row.prefix)
}

/// What one allocated static IP was asked for: its name and the Region the
/// allocating request was made in.
#[derive(Debug, Clone)]
struct Allocated {
    name: String,
    region: String,
}

/// Allocate this row's static IPs, IN REVERSE NAME ORDER, each in its own Region
/// out of the published catalog, answering them sorted by name.
async fn allocate_row(emulator: &Emulator, row: &Row) -> Vec<Allocated> {
    let regions = published_regions(emulator).await;

    let mut allocated = Vec::new();
    for index in (0..row.count).rev() {
        let name = name_of(row, index);
        let region = regions[index % regions.len()].clone();
        emulator
            .ok_at(
                None,
                Some(&region),
                "AllocateStaticIp",
                json!({"staticIpName": name}),
            )
            .await;
        allocated.push(Allocated { name, region });
    }
    allocated.sort_by(|left, right| left.name.cmp(&right.name));
    allocated
}

#[tokio::test(flavor = "current_thread")]
async fn get_static_ips_reports_every_stored_static_ip_in_name_order() {
    for row in ROWS {
        // One account per row: the store keys a static IP by name alone, so a row
        // reads a store holding exactly what that row allocated.
        let emulator = Emulator::new();
        let case = row.case;
        let allocated = allocate_row(&emulator, row).await;

        // The listing is made in ONE region, whatever region each allocation was
        // made in: a static IP belongs to the account, not to the Region the
        // listing is made in.
        let (records, page_sizes) = walk(&emulator, case, Some("us-east-1")).await;

        // The listing reports every stored record, in the store's name order.
        let reported: Vec<String> = records
            .iter()
            .map(|static_ip| {
                static_ip["name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: expected a name, got {static_ip:?}"))
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = allocated.iter().map(|ip| ip.name.clone()).collect();
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
            expected_page_sizes(row.count, PAGE_SIZE),
            "case {case}: expected full pages until the remainder, got {page_sizes:?}"
        );

        // Every reserved address is held by one record alone.
        let mut addresses: BTreeSet<String> = BTreeSet::new();

        for (record, asked) in records.iter().zip(&allocated) {
            let name = &asked.name;
            // Every listed entry is exactly what the single-record read reports:
            // one stored record, two operations.
            let single = read_static_ip(&emulator, name).await;
            assert_eq!(
                record, &single,
                "case {case}: the listing of {name} must match GetStaticIp, got {record:?}"
            );
            // And it carries THIS record's allocating request, not a canned body.
            assert_eq!(
                record["location"]["regionName"].as_str(),
                Some(asked.region.as_str()),
                "case {case}: expected {name} to report the region it was allocated in, got \
                 {record:?}"
            );
            let arn = record["arn"].as_str().unwrap_or_default();
            assert!(
                arn.starts_with(&format!("arn:aws:lightsail:{}:", asked.region)),
                "case {case}: expected {name} to report an ARN in the region it was allocated \
                 in, got {record:?}"
            );
            assert_eq!(
                record["resourceType"].as_str(),
                Some("StaticIp"),
                "case {case}: expected {name} to report its resource type, got {record:?}"
            );
            let address = record["ipAddress"].as_str().unwrap_or_default().to_string();
            assert!(
                is_modelled_ipv4(&address),
                "case {case}: expected {name} to report a modelled IpAddress, got {record:?}"
            );
            assert!(
                addresses.insert(address.clone()),
                "case {case}: expected {name} to hold an address no other record holds, got the \
                 already-used {address:?}"
            );
            assert_eq!(
                record["isAttached"].as_bool(),
                Some(false),
                "case {case}: expected {name} to report that nothing has attached it, got \
                 {record:?}"
            );
            assert!(
                record["attachedTo"].is_null(),
                "case {case}: expected {name} to report no attachment, got {record:?}"
            );
            assert!(
                record["createdAt"].as_f64().unwrap_or_default() > 0.0,
                "case {case}: expected {name} to report a creation timestamp, got {record:?}"
            );
        }
    }
}

/// A listing is a READ: it reports the same records, with the same
/// server-assigned identities, however often it is called — and from whichever
/// Region it is called in, because a static IP belongs to the account.
#[tokio::test(flavor = "current_thread")]
async fn listing_again_and_from_another_region_reports_the_same_records() {
    let emulator = Emulator::new();
    let row = Row {
        case: "repeated listing",
        prefix: "repeat",
        count: PAGE_SIZE + 2,
    };
    allocate_row(&emulator, &row).await;

    let (first, first_sizes) = walk(&emulator, "first listing", Some("us-east-1")).await;
    let (second, second_sizes) = walk(&emulator, "second listing", Some("us-east-1")).await;
    assert_eq!(
        first, second,
        "case repeated listing: a read must not change what a later caller sees, got {second:?}"
    );
    assert_eq!(
        first_sizes, second_sizes,
        "case repeated listing: expected the same paging, got {second_sizes:?}"
    );

    // The same listing, made in another Region and in no Region at all.
    for region in [Some("ap-northeast-1"), None] {
        let (elsewhere, sizes) = walk(&emulator, "listing elsewhere", region).await;
        assert_eq!(
            elsewhere, first,
            "case listing in {region:?}: a static IP belongs to the account, so the same records \
             must be reported, got {elsewhere:?}"
        );
        assert_eq!(
            sizes, first_sizes,
            "case listing in {region:?}: expected the same paging, got {sizes:?}"
        );
    }
}

/// An empty listing is an empty LIST with no token, never an absent member: the
/// caller asked what it holds and the answer is "nothing".
#[tokio::test(flavor = "current_thread")]
async fn an_account_holding_nothing_reports_an_empty_list_and_no_token() {
    let emulator = Emulator::new();
    for token in [None, Some("anything")] {
        let body = page(&emulator, None, token).await;
        assert_eq!(
            entries(&body).len(),
            0,
            "case an empty account at {token:?}: expected an empty staticIps array, got {body:?}"
        );
        assert!(
            body["nextPageToken"].is_null(),
            "case an empty account at {token:?}: a last page carries no nextPageToken, got \
             {body:?}"
        );
    }
}

/// A page token NAMES a record rather than carrying an index, so resuming from
/// any stored name reports that name onward — the invariant that keeps a record
/// allocated or released between two pages from making the walk skip or repeat
/// another one.
#[tokio::test(flavor = "current_thread")]
async fn resuming_from_a_stored_name_reports_that_name_onward() {
    let emulator = Emulator::new();
    let row = Row {
        case: "resuming",
        prefix: "resume",
        count: PAGE_SIZE + 4,
    };
    let allocated = allocate_row(&emulator, &row).await;
    let all: Vec<String> = allocated.iter().map(|ip| ip.name.clone()).collect();

    for (index, token) in all.iter().enumerate() {
        let body = page(&emulator, None, Some(token)).await;
        let reported = names(&entries(&body));
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

/// A static IP allocated after a listing is a record the account holds, so the
/// NEXT listing reports it, in its name's place in the store's order — the
/// listing answers the store rather than a snapshot taken once.
#[tokio::test(flavor = "current_thread")]
async fn a_static_ip_allocated_later_joins_the_listing_in_name_order() {
    let emulator = Emulator::new();
    // Names chosen so the later allocation sorts BETWEEN two already-held ones.
    for name in ["ip-a", "ip-c"] {
        emulator
            .ok("AllocateStaticIp", json!({"staticIpName": name}))
            .await;
    }
    let before = names(&entries(&page(&emulator, None, None).await));
    assert_eq!(
        before,
        vec!["ip-a".to_string(), "ip-c".to_string()],
        "case before the later allocation: expected the two held names, got {before:?}"
    );

    emulator
        .ok("AllocateStaticIp", json!({"staticIpName": "ip-b"}))
        .await;
    let after = names(&entries(&page(&emulator, None, None).await));
    assert_eq!(
        after,
        vec!["ip-a".to_string(), "ip-b".to_string(), "ip-c".to_string()],
        "case after the later allocation: expected the new name in name order, got {after:?}"
    );

    // And the new entry is exactly what the single-record read reports.
    let listed = entries(&page(&emulator, None, None).await);
    let entry = listed
        .iter()
        .find(|entry| entry["name"].as_str() == Some("ip-b"))
        .unwrap_or_else(|| panic!("case after the later allocation: expected ip-b, got {listed:?}"));
    assert_eq!(
        entry,
        &read_static_ip(&emulator, "ip-b").await,
        "case after the later allocation: the listing must match GetStaticIp, got {entry:?}"
    );
}

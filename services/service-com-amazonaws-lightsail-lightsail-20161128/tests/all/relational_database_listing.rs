// SPDX-License-Identifier: CC0-1.0

//! `GetRelationalDatabases` — the paged listing of the managed relational
//! databases an account holds — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a listing reports every record the store holds of that
//! resource type, each entry exactly as the single-record read reports it, in the
//! store's own name order, carried one page at a time, with `nextPageToken`
//! NAMING the first record of the page after it while records remain and ABSENT
//! on the last page — and, being a read, it reports the same records however
//! often it is called.
//!
//! The rows span what that rule generalises over: how many databases the account
//! holds (none, one, exactly a page, one past a page, several pages and a
//! remainder), the creation order against the reported order (every row is
//! created in reverse name order, so a listing that merely echoed the creation
//! sequence would fail), and which blueprint and bundle each record was asked
//! for — drawn from the service's OWN published catalogs, so the engine, the
//! version and the hardware differ from entry to entry and a canned body could
//! not satisfy them.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state;

// The harness this test target already exposes: a sibling suite drives the
// service through the same front door rather than standing up a second one.
use crate::instance_lifecycle::Emulator;

/// How many databases one page of `GetRelationalDatabases` carries — the
/// handler's OWN constant, so the table's rows keep straddling the page boundary
/// whatever it is set to.
const PAGE_SIZE: usize = state::RELATIONAL_DATABASE_PAGE_SIZE;

/// The master database name and master user name every row asks for: both are
/// accepted by every published engine, so a row varies along the engine without
/// also varying along the credential limits (which
/// `relational_database_lifecycle` covers).
const MASTER_DATABASE_NAME: &str = "appdb";
const MASTER_USERNAME: &str = "dbadmin";

/// A password of exactly `chars` characters, drawn only from characters the
/// modelled `masterUserPassword` documentation admits (printable ASCII, no space
/// and none of `/`, `"`, `@`).
fn password_of(chars: usize) -> String {
    "Pw9#x!Zq".chars().cycle().take(chars).collect()
}

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Every id of a paged catalog, walking its pages.
///
/// SOURCE: the service's own published catalog — `GetRelationalDatabaseBlueprints`
/// and `GetRelationalDatabaseBundles` — so no entry is hand-written here and the
/// rows span every engine and bundle the service publishes, not a chosen few.
async fn catalog_ids(
    emulator: &Emulator,
    operation: &str,
    list_member: &str,
    id_member: &str,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => emulator.ok(operation, json!({"pageToken": token})).await,
            None => emulator.ok(operation, json!({})).await,
        };
        let entries = body[list_member]
            .as_array()
            .unwrap_or_else(|| panic!("case reading {operation}: expected an array, got {body:?}"))
            .clone();
        for entry in entries {
            let id = entry[id_member].as_str().unwrap_or_else(|| {
                panic!("case reading {operation}: expected every entry to carry {id_member}, got {entry:?}")
            });
            ids.push(id.to_string());
        }
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert!(
        !ids.is_empty(),
        "case reading {operation}: expected a non-empty catalog"
    );
    ids
}

/// One `GetRelationalDatabases` page, resuming from `page_token` when one is
/// given.
async fn page(emulator: &Emulator, page_token: Option<&str>) -> Value {
    let body = match page_token {
        Some(token) => json!({"pageToken": token}),
        None => json!({}),
    };
    emulator.ok("GetRelationalDatabases", body).await
}

/// The entries a page reports, in the order it reports them.
fn entries(page: &Value) -> Vec<Value> {
    page["relationalDatabases"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a relationalDatabases array, got {page:?}"))
        .clone()
}

/// The names a page reports, in the order it reports them.
fn names(page: &Value) -> Vec<String> {
    entries(page)
        .iter()
        .map(|database| {
            database["name"]
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

/// The stored database `name`, as `GetRelationalDatabase` reports it.
async fn read_database(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok(
            "GetRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await["relationalDatabase"]
        .clone()
}

/// Poll `GetRelationalDatabase` until `name` reports `available`.
///
/// The transition is owed by the clock, not by the poll: waiting for it here
/// means the listing and the single-record read are compared in the SAME
/// lifecycle state, so a difference between them is a difference in what the two
/// operations report, never a transition that landed between the two calls.
async fn wait_for_available(emulator: &Emulator, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let database = read_database(emulator, name).await;
        if database["state"].as_str() == Some("available") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for 'available' on {name}: expected it within 10s, got {:?}",
            database["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The listing table
// ---------------------------------------------------------------------------

/// One account's worth of databases: how many it holds, and the name prefix that
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

/// The name of this row's database `index`. Zero-padded, so name order is index
/// order however many a row holds.
fn name_of(row: &Row, index: usize) -> String {
    format!("{}-db-{index:02}", row.prefix)
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

/// Create this row's databases, IN REVERSE NAME ORDER, each on its own blueprint
/// and bundle out of the published catalogs, and wait until every one is
/// `available`.
///
/// Answers, per database name, the blueprint and bundle id its request asked for.
async fn create_row(emulator: &Emulator, row: &Row) -> Vec<(String, String, String)> {
    let blueprints = catalog_ids(
        emulator,
        "GetRelationalDatabaseBlueprints",
        "blueprints",
        "blueprintId",
    )
    .await;
    let bundles = catalog_ids(
        emulator,
        "GetRelationalDatabaseBundles",
        "bundles",
        "bundleId",
    )
    .await;

    let mut asked = Vec::new();
    for index in (0..row.count).rev() {
        let name = name_of(row, index);
        let blueprint_id = blueprints[index % blueprints.len()].clone();
        let bundle_id = bundles[index % bundles.len()].clone();
        emulator
            .ok(
                "CreateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "relationalDatabaseBlueprintId": blueprint_id,
                    "relationalDatabaseBundleId": bundle_id,
                    "masterDatabaseName": MASTER_DATABASE_NAME,
                    "masterUsername": MASTER_USERNAME,
                    "masterUserPassword": password_of(16 + index % 8),
                }),
            )
            .await;
        asked.push((name, blueprint_id, bundle_id));
    }
    for (name, _, _) in &asked {
        wait_for_available(emulator, name).await;
    }
    asked.sort();
    asked
}

#[tokio::test(flavor = "current_thread")]
async fn get_relational_databases_reports_every_stored_database_in_name_order() {
    for row in ROWS {
        // One account per row: the store keys a database by name alone, so a row
        // reads a store holding exactly what that row created.
        let emulator = Emulator::new();
        let case = row.case;
        let asked = create_row(&emulator, row).await;

        let (records, page_sizes) = walk(&emulator, case, row.count / PAGE_SIZE + 3).await;

        // The listing reports every stored record, in the store's name order.
        let reported: Vec<String> = records
            .iter()
            .map(|database| {
                database["name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: expected a name, got {database:?}"))
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = asked.iter().map(|(name, _, _)| name.clone()).collect();
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

        for (record, (name, blueprint_id, bundle_id)) in records.iter().zip(&asked) {
            // Every listed entry is exactly what the single-record read reports:
            // one stored record, two operations.
            let single = read_database(&emulator, name).await;
            assert_eq!(
                record, &single,
                "case {case}: the listing of {name} must match GetRelationalDatabase, got \
                 {record:?}"
            );
            // And it carries THIS record's request, not a canned body.
            assert_eq!(
                record["relationalDatabaseBlueprintId"].as_str(),
                Some(blueprint_id.as_str()),
                "case {case}: expected {name} to report the blueprint it was created with, got \
                 {record:?}"
            );
            assert_eq!(
                record["relationalDatabaseBundleId"].as_str(),
                Some(bundle_id.as_str()),
                "case {case}: expected {name} to report the bundle it was created with, got \
                 {record:?}"
            );
            assert_eq!(
                record["resourceType"].as_str(),
                Some("RelationalDatabase"),
                "case {case}: expected {name} to report its resource type, got {record:?}"
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
/// caller asked what it holds and the answer is "nothing".
#[tokio::test(flavor = "current_thread")]
async fn an_account_holding_nothing_reports_an_empty_list_and_no_token() {
    let emulator = Emulator::new();
    for token in [None, Some("anything")] {
        let body = page(&emulator, token).await;
        assert_eq!(
            entries(&body).len(),
            0,
            "case an empty account at {token:?}: expected an empty relationalDatabases array, got \
             {body:?}"
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
    let asked = create_row(&emulator, &row).await;
    let all: Vec<String> = asked.iter().map(|(name, _, _)| name.clone()).collect();

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

/// A database the clock has not settled yet is still a record the account holds,
/// so the listing reports it — in the same state, and with the same members, the
/// single-record read reports.
#[tokio::test(flavor = "current_thread")]
async fn a_database_still_being_created_is_listed_as_the_single_read_reports_it() {
    let emulator = Emulator::new();
    let name = "pending-db";
    // Drawn from the service's own published catalogs, as every other case here
    // does, so no blueprint or bundle id is hand-written in this suite.
    let blueprint_id = catalog_ids(
        &emulator,
        "GetRelationalDatabaseBlueprints",
        "blueprints",
        "blueprintId",
    )
    .await
    .remove(0);
    let bundle_id = catalog_ids(
        &emulator,
        "GetRelationalDatabaseBundles",
        "bundles",
        "bundleId",
    )
    .await
    .remove(0);
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": name,
                "relationalDatabaseBlueprintId": blueprint_id,
                "relationalDatabaseBundleId": bundle_id,
                "masterDatabaseName": MASTER_DATABASE_NAME,
                "masterUsername": MASTER_USERNAME,
                "masterUserPassword": password_of(16),
            }),
        )
        .await;

    let listed = entries(&page(&emulator, None).await);
    assert_eq!(
        listed.len(),
        1,
        "case listing a database being created: expected the one stored record, got {listed:?}"
    );
    let state = listed[0]["state"].as_str().unwrap_or_default().to_string();
    assert!(
        state == "creating" || state == "available",
        "case listing a database being created: expected 'creating' or 'available', got {state:?}"
    );
    if state == "creating" {
        assert!(
            listed[0]["masterEndpoint"].is_null(),
            "case listing a database being created: a database still being provisioned announces \
             no endpoint, got {:?}",
            listed[0]
        );
    }

    // Once the clock has settled it, the listing and the single read agree again.
    wait_for_available(&emulator, name).await;
    let settled = entries(&page(&emulator, None).await);
    let single = read_database(&emulator, name).await;
    assert_eq!(
        settled,
        vec![single.clone()],
        "case listing a settled database: expected exactly what GetRelationalDatabase reports, \
         got {settled:?}"
    );
    assert_eq!(
        single["state"].as_str(),
        Some("available"),
        "case listing a settled database: expected 'available', got {single:?}"
    );
}

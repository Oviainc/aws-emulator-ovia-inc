// SPDX-License-Identifier: CC0-1.0

//! The Lightsail managed relational database SNAPSHOT — create and list — driven
//! over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `GetRelationalDatabaseSnapshots` reports the account's
//! stored database snapshots — every member of every entry read out of the one
//! record `CreateRelationalDatabaseSnapshot` persisted — in the store's own total
//! order by snapshot name, at most one page at a time, with `nextPageToken`
//! NAMING the first snapshot of the page after it and absent once none remain.
//! A snapshot is a point-in-time copy, so everything it says about its source
//! (name, ARN, engine, engine version, disk size, bundle id, blueprint id,
//! location) is read out of the stored DATABASE record at the instant it is
//! taken, the request's tags are kept verbatim, and every server-assigned member
//! (ARN, support code, creation time, resource type, state) is computed from
//! them plus the store's own allocators. The snapshot is created `creating` and
//! the CLOCK, not a reader, settles it into `available`; a source the account
//! does not hold, or a snapshot name already taken, is refused and stores
//! nothing.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// How many snapshots one page of `GetRelationalDatabaseSnapshots` carries
/// (`crate::state::RELATIONAL_DATABASE_SNAPSHOT_PAGE_SIZE`).
const PAGE_SIZE: usize = 10;

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// One page of `GetRelationalDatabaseSnapshots`: the entries it reports, and the
/// token for the page after it when one remains.
async fn snapshot_page(emulator: &Emulator, token: Option<&str>) -> (Vec<Value>, Option<String>) {
    let body = match token {
        Some(token) => {
            emulator
                .ok(
                    "GetRelationalDatabaseSnapshots",
                    json!({"pageToken": token}),
                )
                .await
        }
        None => {
            emulator
                .ok("GetRelationalDatabaseSnapshots", json!({}))
                .await
        }
    };
    let entries = body["relationalDatabaseSnapshots"]
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

/// The stored database `name`, as `GetRelationalDatabase` reports it.
async fn database(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok(
            "GetRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await["relationalDatabase"]
        .clone()
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

// ---------------------------------------------------------------------------
// The create/list table
// ---------------------------------------------------------------------------

/// One `CreateRelationalDatabaseSnapshot` request the service must serve, with
/// the source database it is taken of.
///
/// The rows differ along everything the rule generalises over: the source's
/// ENGINE and engine version (MySQL, and PostgreSQL at three different published
/// series), its bundle — and so the disk size the snapshot reports, from the
/// smallest published plan at 40 GB to the largest at 240 GB, standard and
/// high-availability — whether the source request named an Availability Zone at
/// all, the zone's region (and so the region the snapshot's ARN and location
/// carry), the region the SNAPSHOT call is signed for, which this operation must
/// ignore, the number of tags, and the form of the snapshot name (hyphens,
/// underscores, digits).
struct Row {
    case: &'static str,
    db_name: &'static str,
    /// The region the source create is signed for, or `None` for a request that
    /// carries no credential scope at all.
    db_region: Option<&'static str>,
    availability_zone: Option<&'static str>,
    /// A blueprint id this service publishes (`GetRelationalDatabaseBlueprints`).
    blueprint_id: &'static str,
    /// A bundle id this service publishes (`GetRelationalDatabaseBundles`).
    bundle_id: &'static str,
    snapshot_name: &'static str,
    /// The region the SNAPSHOT create is signed for: a snapshot is located where
    /// its source is, so this must change nothing.
    snapshot_region: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// The table. Blueprint and bundle ids are entries of this service's own
/// published catalogs, which the first case reads back to prove they are.
const ROWS: &[Row] = &[
    Row {
        case: "mysql on the smallest published bundle, no tags",
        db_name: "db-mysql-micro",
        db_region: None,
        availability_zone: Some("us-east-2a"),
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_1_0",
        snapshot_name: "db-mysql-micro-snapshot",
        snapshot_region: None,
        tags: &[],
    },
    Row {
        case: "postgres 17 on the largest published bundle, snapshot signed for another region",
        db_name: "db-pg17-large",
        db_region: Some("us-east-1"),
        availability_zone: Some("us-east-1c"),
        blueprint_id: "postgres_17",
        bundle_id: "large_1_0",
        snapshot_name: "pg17_large_snap_1",
        snapshot_region: Some("eu-central-1"),
        tags: &[("env", "prod")],
    },
    Row {
        case: "postgres 13, the oldest published series, on a high-availability bundle, three tags",
        db_name: "db-pg13-ha",
        db_region: Some("ap-northeast-1"),
        availability_zone: Some("ap-northeast-1c"),
        blueprint_id: "postgres_13",
        bundle_id: "small_ha_1_0",
        snapshot_name: "db-pg13-ha-2026-10-24",
        snapshot_region: Some("ap-northeast-1"),
        tags: &[("env", "staging"), ("owner", "data"), ("cost-centre", "42")],
    },
    Row {
        case: "a source the request placed by region alone, postgres 18 on a medium bundle",
        db_name: "db-default-zone",
        db_region: Some("us-west-2"),
        availability_zone: None,
        blueprint_id: "postgres_18",
        bundle_id: "medium_1_0",
        snapshot_name: "snap-4",
        snapshot_region: Some("us-west-2"),
        tags: &[("tier", "bronze")],
    },
    Row {
        case: "mysql on a small bundle, in a two-zone region, two tags",
        db_name: "db_mysql_small",
        db_region: Some("ca-central-1"),
        availability_zone: Some("ca-central-1b"),
        blueprint_id: "mysql_8_4",
        bundle_id: "small_1_0",
        snapshot_name: "Snapshot_Five",
        snapshot_region: None,
        tags: &[("env", "dev"), ("owner", "qa")],
    },
    Row {
        case: "postgres 14 on a high-availability micro bundle, shortest snapshot name",
        db_name: "db-pg14-ha-micro",
        db_region: Some("us-east-1"),
        availability_zone: Some("us-east-1f"),
        blueprint_id: "postgres_14",
        bundle_id: "micro_ha_1_0",
        snapshot_name: "s6",
        snapshot_region: Some("eu-west-3"),
        tags: &[("app", "search")],
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

/// The `CreateRelationalDatabase` request that creates `row`'s source database.
fn database_body(row: &Row) -> Value {
    let mut body = json!({
        "relationalDatabaseName": row.db_name,
        "relationalDatabaseBlueprintId": row.blueprint_id,
        "relationalDatabaseBundleId": row.bundle_id,
        "masterDatabaseName": "appdata",
        "masterUsername": "dbmaster",
    });
    if let Some(zone) = row.availability_zone {
        body["availabilityZone"] = json!(zone);
    }
    body
}

/// The `CreateRelationalDatabaseSnapshot` request `row` describes.
fn snapshot_body(row: &Row) -> Value {
    let mut body = json!({
        "relationalDatabaseName": row.db_name,
        "relationalDatabaseSnapshotName": row.snapshot_name,
    });
    if !row.tags.is_empty() {
        body["tags"] = tag_list(row.tags);
    }
    body
}

/// Create `row`'s source database and then its snapshot, answering the
/// `CreateRelationalDatabaseSnapshot` response body.
async fn create_row(emulator: &Emulator, row: &Row) -> Value {
    emulator
        .ok_at(
            None,
            row.db_region,
            "CreateRelationalDatabase",
            database_body(row),
        )
        .await;
    emulator
        .ok_at(
            None,
            row.snapshot_region,
            "CreateRelationalDatabaseSnapshot",
            snapshot_body(row),
        )
        .await
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: every member of a listed snapshot comes from the ONE record the create
/// persisted — the source's own stored values, the request's tags verbatim, and
/// server-assigned members computed from them plus the store's allocators.
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
            Some("CreateRelationalDatabaseSnapshot"),
            "case {case}: expected this call's own operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("RelationalDatabaseSnapshot"),
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
        let source = database(&emulator, row.db_name).await;
        let snapshot = listed(&emulator, row.snapshot_name).await;

        assert_eq!(
            snapshot["name"].as_str(),
            Some(row.snapshot_name),
            "case {case}: expected the snapshot name the request carried, got {snapshot:?}"
        );
        assert_eq!(
            snapshot["resourceType"].as_str(),
            Some("RelationalDatabaseSnapshot"),
            "case {case}: expected the RelationalDatabaseSnapshot resource type, got {snapshot:?}"
        );
        assert_eq!(
            snapshot["engine"], source["engine"],
            "case {case}: expected the source's own engine {:?}, got {:?}",
            source["engine"], snapshot["engine"]
        );
        assert_eq!(
            snapshot["engineVersion"], source["engineVersion"],
            "case {case}: expected the source's own engine version {:?}, got {:?}",
            source["engineVersion"], snapshot["engineVersion"]
        );
        assert_eq!(
            snapshot["sizeInGb"], source["hardware"]["diskSizeInGb"],
            "case {case}: expected the source's own disk size {:?}, got {:?}",
            source["hardware"]["diskSizeInGb"], snapshot["sizeInGb"]
        );
        assert_eq!(
            snapshot["fromRelationalDatabaseName"].as_str(),
            Some(row.db_name),
            "case {case}: expected the source database's name, got {snapshot:?}"
        );
        assert_eq!(
            snapshot["fromRelationalDatabaseArn"], source["arn"],
            "case {case}: expected the source database's own ARN {:?}, got {:?}",
            source["arn"], snapshot["fromRelationalDatabaseArn"]
        );
        assert_eq!(
            snapshot["fromRelationalDatabaseBundleId"], source["relationalDatabaseBundleId"],
            "case {case}: expected the source's own bundle id {:?}, got {:?}",
            source["relationalDatabaseBundleId"], snapshot["fromRelationalDatabaseBundleId"]
        );
        assert_eq!(
            snapshot["fromRelationalDatabaseBlueprintId"], source["relationalDatabaseBlueprintId"],
            "case {case}: expected the source's own blueprint id {:?}, got {:?}",
            source["relationalDatabaseBlueprintId"], snapshot["fromRelationalDatabaseBlueprintId"]
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
        let region = source["location"]["regionName"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a source region, got {source:?}"));
        let arn = snapshot["arn"].as_str().unwrap_or_default().to_string();
        let prefix = format!("arn:aws:lightsail:{region}:{ACCOUNT}:RelationalDatabaseSnapshot/");
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

        // The state is one this store produces, and it settles into `available`
        // without the snapshot record changing in any other way.
        let state = snapshot["state"].as_str().unwrap_or_default().to_string();
        assert!(
            state == "creating" || state == "available",
            "case {case}: expected a creating or available snapshot, got {state:?}"
        );
    }

    // The listing reports exactly the snapshots created, once each, in the
    // store's own order by name — and two walks answer the same thing.
    let (all, _) = all_snapshots(&emulator).await;
    let listed_names: Vec<String> = all
        .iter()
        .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut want_names: Vec<String> = ROWS
        .iter()
        .map(|row| row.snapshot_name.to_string())
        .collect();
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

/// RULE: an account holding no database snapshot is told so — an EMPTY list, not
/// an absent one, and no next page token.
#[tokio::test]
async fn an_account_with_no_snapshots_lists_an_empty_array() {
    let emulator = Emulator::new();
    let body = emulator
        .ok("GetRelationalDatabaseSnapshots", json!({}))
        .await;
    assert_eq!(
        body["relationalDatabaseSnapshots"],
        json!([]),
        "case an empty account: expected an empty snapshot list, got {body:?}"
    );
    assert!(
        body["nextPageToken"].is_null(),
        "case an empty account: expected no next page token, got {body:?}"
    );

    // A database with no snapshot taken of it is still no snapshot.
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "db-unsnapshotted",
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": "micro_1_0",
                "masterDatabaseName": "appdata",
                "masterUsername": "dbmaster",
            }),
        )
        .await;
    let body = emulator
        .ok("GetRelationalDatabaseSnapshots", json!({}))
        .await;
    assert_eq!(
        body["relationalDatabaseSnapshots"],
        json!([]),
        "case a database with no snapshot: expected an empty snapshot list, got {body:?}"
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
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "db-paging",
                "relationalDatabaseBlueprintId": "postgres_16",
                "relationalDatabaseBundleId": "micro_1_0",
                "masterDatabaseName": "appdata",
                "masterUsername": "dbmaster",
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
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": "db-paging",
                    "relationalDatabaseSnapshotName": name,
                }),
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

/// RULE: a snapshot is created `creating` and the CLOCK settles it into
/// `available` — so a caller reading straight after the create still sees
/// `creating`, and the transition arrives without anyone polling for it.
#[tokio::test]
async fn a_snapshot_is_created_creating_and_the_clock_settles_it() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "db-settling",
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": "small_1_0",
                "masterDatabaseName": "appdata",
                "masterUsername": "dbmaster",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateRelationalDatabaseSnapshot",
            json!({
                "relationalDatabaseName": "db-settling",
                "relationalDatabaseSnapshotName": "db-settling-snapshot",
            }),
        )
        .await;

    let fresh = listed(&emulator, "db-settling-snapshot").await;
    assert_eq!(
        fresh["state"].as_str(),
        Some("creating"),
        "case reading straight after the create: expected a creating snapshot, got {fresh:?}"
    );

    let settled = wait_for_state(&emulator, "db-settling-snapshot", "available").await;
    // Settling changes the state and NOTHING else about the record.
    for member in [
        "name",
        "arn",
        "supportCode",
        "createdAt",
        "location",
        "resourceType",
        "engine",
        "engineVersion",
        "sizeInGb",
        "fromRelationalDatabaseName",
        "fromRelationalDatabaseArn",
        "fromRelationalDatabaseBundleId",
        "fromRelationalDatabaseBlueprintId",
    ] {
        assert_eq!(
            settled[member], fresh[member],
            "case settling to available: expected {member} to be untouched, got {:?}",
            settled[member]
        );
    }
}

/// RULE: the source must be a database this account holds, so a well-formed name
/// that names none is refused with the service's `DoesNotExist` envelope — HTTP
/// 400, `NotFoundException`, body code `DoesNotExist` and a message naming the
/// resource type and the requested name — and stores nothing.
#[tokio::test]
async fn a_source_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    // A snapshot of a database that exists, so the store is not merely empty.
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "db-present",
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": "micro_1_0",
                "masterDatabaseName": "appdata",
                "masterUsername": "dbmaster",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateRelationalDatabaseSnapshot",
            json!({
                "relationalDatabaseName": "db-present",
                "relationalDatabaseSnapshotName": "db-present-snapshot",
            }),
        )
        .await;
    let (before, _) = all_snapshots(&emulator).await;

    // A name never created, and a name that names a snapshot rather than a
    // database: neither is a database this account holds.
    for (case, source_name) in [
        ("a database that was never created", "db-absent"),
        (
            "a snapshot name used as the source",
            "db-present-snapshot",
        ),
    ] {
        let (status, body) = emulator
            .call(
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": source_name,
                    "relationalDatabaseSnapshotName": "refused-snapshot",
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
            Some(format!("The RelationalDatabase does not exist: {source_name}").as_str()),
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
    for (db_name, blueprint_id, bundle_id) in [
        ("db-one", "mysql_8_4", "micro_1_0"),
        ("db-two", "postgres_15", "large_1_0"),
    ] {
        emulator
            .ok(
                "CreateRelationalDatabase",
                json!({
                    "relationalDatabaseName": db_name,
                    "relationalDatabaseBlueprintId": blueprint_id,
                    "relationalDatabaseBundleId": bundle_id,
                    "masterDatabaseName": "appdata",
                    "masterUsername": "dbmaster",
                }),
            )
            .await;
    }
    emulator
        .ok(
            "CreateRelationalDatabaseSnapshot",
            json!({
                "relationalDatabaseName": "db-one",
                "relationalDatabaseSnapshotName": "taken-name",
                "tags": [{"key": "env", "value": "prod"}],
            }),
        )
        .await;
    let before = listed(&emulator, "taken-name").await;

    // Re-taking the name — even for a DIFFERENT source database — is refused,
    // and the record the first create stored is the one that remains.
    for (case, source_name) in [
        ("the same source database", "db-one"),
        ("a different source database", "db-two"),
    ] {
        let (status, body) = emulator
            .call(
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": source_name,
                    "relationalDatabaseSnapshotName": "taken-name",
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
            after["fromRelationalDatabaseName"], before["fromRelationalDatabaseName"],
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

/// RULE: a snapshot is a point-in-time COPY, so one database can be snapshotted
/// as often as the caller likes and every snapshot is its own listed record.
#[tokio::test]
async fn each_snapshot_of_one_database_is_its_own_record() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "db-repeat",
                "relationalDatabaseBlueprintId": "postgres_17",
                "relationalDatabaseBundleId": "medium_1_0",
                "masterDatabaseName": "appdata",
                "masterUsername": "dbmaster",
            }),
        )
        .await;
    let names = ["repeat-a", "repeat-b", "repeat-c"];
    for name in names {
        emulator
            .ok(
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": "db-repeat",
                    "relationalDatabaseSnapshotName": name,
                }),
            )
            .await;
    }

    let (all, _) = all_snapshots(&emulator).await;
    assert_eq!(
        all.len(),
        names.len(),
        "case snapshotting one database three times: expected three listed snapshots, got {all:?}"
    );
    let mut arns: BTreeSet<String> = BTreeSet::new();
    for name in names {
        let snapshot = listed(&emulator, name).await;
        assert_eq!(
            snapshot["fromRelationalDatabaseName"].as_str(),
            Some("db-repeat"),
            "case {name}: expected the one source database, got {snapshot:?}"
        );
        let arn = snapshot["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            arns.insert(arn.clone()),
            "case {name}: expected an ARN of its own, got the already-used {arn:?}"
        );
    }
}

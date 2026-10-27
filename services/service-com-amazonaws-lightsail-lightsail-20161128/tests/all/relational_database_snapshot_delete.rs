// SPDX-License-Identifier: CC0-1.0

//! The Lightsail managed relational database snapshot's deletion —
//! `DeleteRelationalDatabaseSnapshot` — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `DeleteRelationalDatabaseSnapshot` removes the ONE stored
//! database snapshot the name it carries addresses — whatever that snapshot's
//! source engine, bundle, location, tags or state — so the next read of that name
//! is refused, the listing no longer reports it and the name is free again, while
//! the source database, that database's OTHER snapshots and every other stored
//! record are left exactly as they were. It answers one operation-log entry of
//! the modelled `DeleteRelationalDatabaseSnapshot` type, located where the deleted
//! snapshot was and already terminal, because the record is gone as the call
//! returns. A well-formed name this account holds no database snapshot under is
//! refused with the service's `DoesNotExist` envelope and deletes nothing.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// How long to wait for a clock-owed transition before giving up.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// The snapshot `name` as `GetRelationalDatabaseSnapshot` reports it.
async fn snapshot(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok(
            "GetRelationalDatabaseSnapshot",
            json!({"relationalDatabaseSnapshotName": name}),
        )
        .await["relationalDatabaseSnapshot"]
        .clone()
}

/// Every snapshot `GetRelationalDatabaseSnapshots` reports, walking every page.
async fn all_snapshot_names(emulator: &Emulator) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let request = match &token {
            Some(token) => json!({"pageToken": token}),
            None => json!({}),
        };
        let body = emulator
            .ok("GetRelationalDatabaseSnapshots", request)
            .await;
        let entries = body["relationalDatabaseSnapshots"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case listing snapshots at {token:?}: expected an array, got {body:?}")
            })
            .clone();
        names.extend(
            entries
                .iter()
                .map(|entry| entry["name"].as_str().unwrap_or_default().to_string()),
        );
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    names
}

/// Poll `GetRelationalDatabaseSnapshot` until it reports `want`, answering the
/// snapshot.
///
/// The transition is owed by the CLOCK, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_snapshot_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let read = snapshot(emulator, name).await;
        if read["state"].as_str() == Some(want) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within the settle timeout, got {:?}",
            read["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
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

/// Poll `GetRelationalDatabase` until the clock has settled it into `want`.
async fn wait_for_database_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let read = database(emulator, name).await;
        if read["state"].as_str() == Some(want) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on the database {name}: expected it within the settle \
             timeout, got {:?}",
            read["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Assert that `name` names no stored database snapshot: the service's
/// `DoesNotExist` envelope — HTTP 400, wire error `NotFoundException`, body
/// `code` `DoesNotExist` and the resource-type/name message — from the single
/// read, and no entry of that name anywhere in the listing.
async fn assert_absent(emulator: &Emulator, case: &str, name: &str) {
    let (status, body) = emulator
        .call(
            "GetRelationalDatabaseSnapshot",
            json!({"relationalDatabaseSnapshotName": name}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected a 400 for the deleted {name}, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "case {case}: expected the modelled NotFoundException for {name}, got {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some("DoesNotExist"),
        "case {case}: expected code DoesNotExist for {name}, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(format!("The RelationalDatabaseSnapshot does not exist: {name}").as_str()),
        "case {case}: expected the resource-type/name message for {name}, got {body:?}"
    );
    let listed = all_snapshot_names(emulator).await;
    assert!(
        !listed.iter().any(|entry| entry == name),
        "case {case}: expected the listing to stop reporting {name}, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// The deletion table
// ---------------------------------------------------------------------------

/// One snapshot the service must delete, and the source database it was taken
/// of.
///
/// The rows differ along everything the rule generalises over: the source's
/// ENGINE and engine version (MySQL, and PostgreSQL at three published series),
/// its bundle — standard and high-availability, from the smallest published plan
/// to the largest, so the deleted record's disk size differs — the Availability
/// Zone and so the Region the log entry's location carries, the number of tags
/// the deleted record held, the form of the snapshot name (hyphens, underscores,
/// digits, mixed case), and the snapshot's own STATE when the delete arrives (one
/// the clock has settled into `available`, one still `creating`).
#[derive(Debug)]
struct Row {
    case: &'static str,
    db_name: &'static str,
    /// A blueprint id this service publishes (`GetRelationalDatabaseBlueprints`).
    blueprint_id: &'static str,
    /// A bundle id this service publishes (`GetRelationalDatabaseBundles`).
    bundle_id: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    snapshot_name: &'static str,
    tags: &'static [(&'static str, &'static str)],
    /// Whether to let the clock settle the snapshot into `available` before
    /// deleting it. A row that does not wait is deleted while the copy is still
    /// being taken.
    settled_before_delete: bool,
}

/// The table. Blueprint and bundle ids are entries of this service's own
/// published catalogs (`GetRelationalDatabaseBlueprints`,
/// `GetRelationalDatabaseBundles`), so a wrong id fails the source create rather
/// than this delete.
const ROWS: &[Row] = &[
    Row {
        case: "mysql on the smallest published bundle, no tags, settled",
        db_name: "del-db-mysql-micro",
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_1_0",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        snapshot_name: "del-db-mysql-micro-snapshot",
        tags: &[],
        settled_before_delete: true,
    },
    Row {
        case: "postgres 15 on the largest published bundle, one tag, still creating",
        db_name: "del_db_pg15_large",
        blueprint_id: "postgres_15",
        bundle_id: "large_1_0",
        availability_zone: "eu-west-1c",
        region: "eu-west-1",
        snapshot_name: "del_pg15_large_snap_1",
        tags: &[("env", "prod")],
        settled_before_delete: false,
    },
    Row {
        case: "postgres 13, the oldest published series, on a small high-availability bundle, \
               three tags, settled",
        db_name: "del-db-pg13-ha",
        blueprint_id: "postgres_13",
        bundle_id: "small_ha_1_0",
        availability_zone: "ap-northeast-1c",
        region: "ap-northeast-1",
        snapshot_name: "Del-Db-Pg13-Ha-2026",
        tags: &[("env", "staging"), ("owner", "data"), ("cost-centre", "42")],
        settled_before_delete: true,
    },
    Row {
        case: "postgres 17 on a medium bundle, shortest snapshot name, two tags, still creating",
        db_name: "del-db-pg17-medium",
        blueprint_id: "postgres_17",
        bundle_id: "medium_1_0",
        availability_zone: "ca-central-1b",
        region: "ca-central-1",
        snapshot_name: "d5",
        tags: &[("tier", "bronze"), ("owner", "qa")],
        settled_before_delete: false,
    },
    Row {
        case: "mysql on a micro high-availability bundle, one tag, settled",
        db_name: "del_db_mysql_ha",
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_ha_1_0",
        availability_zone: "us-west-2a",
        region: "us-west-2",
        snapshot_name: "del-db-mysql-ha-backup",
        tags: &[("keep", "no")],
        settled_before_delete: true,
    },
];

/// The `tags` member a request carries, in the modelled `TagList` form.
fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// Create `row`'s source database and then its snapshot, each signed for the
/// row's own Region.
async fn create_row(emulator: &Emulator, row: &Row) {
    emulator
        .ok_at(
            None,
            Some(row.region),
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": row.db_name,
                "relationalDatabaseBlueprintId": row.blueprint_id,
                "relationalDatabaseBundleId": row.bundle_id,
                "availabilityZone": row.availability_zone,
                "masterDatabaseName": "appdata",
                "masterUsername": "dbmaster",
            }),
        )
        .await;
    let mut body = json!({
        "relationalDatabaseName": row.db_name,
        "relationalDatabaseSnapshotName": row.snapshot_name,
    });
    if !row.tags.is_empty() {
        body["tags"] = tag_list(row.tags);
    }
    emulator
        .ok_at(
            None,
            Some(row.region),
            "CreateRelationalDatabaseSnapshot",
            body,
        )
        .await;
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: the delete removes the one stored snapshot its name addresses and
/// answers one terminal `DeleteRelationalDatabaseSnapshot` log entry located
/// where that snapshot was; afterwards the name reads back refused and is gone
/// from the listing, while the snapshot's source database and every other stored
/// snapshot are untouched.
#[tokio::test]
async fn deleting_a_snapshot_removes_that_one_record() {
    let emulator = Emulator::new();

    // A bystander snapshot, created first and never deleted: every deletion
    // below has to leave it exactly as it is.
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "bystander-db",
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
                "relationalDatabaseName": "bystander-db",
                "relationalDatabaseSnapshotName": "bystander_snapshot",
                "tags": [{"key": "keep", "value": "me"}],
            }),
        )
        .await;
    let bystander = wait_for_snapshot_state(&emulator, "bystander_snapshot", "available").await;

    let mut operation_ids: BTreeSet<String> = BTreeSet::new();
    for row in ROWS {
        let case = row.case;
        let name = row.snapshot_name;
        create_row(&emulator, row).await;
        let before = if row.settled_before_delete {
            wait_for_snapshot_state(&emulator, name, "available").await
        } else {
            snapshot(&emulator, name).await
        };
        let state_before = before["state"].as_str().unwrap_or_default().to_string();
        assert!(
            state_before == "creating" || state_before == "available",
            "case {case}: expected a modelled RelationalDatabaseSnapshot state before the delete, \
             got {state_before:?}"
        );
        assert_eq!(
            before["tags"].as_array().map(Vec::len),
            Some(row.tags.len()),
            "case {case}: expected the request's tags on the stored snapshot, got {:?}",
            before["tags"]
        );

        let deleted = emulator
            .ok(
                "DeleteRelationalDatabaseSnapshot",
                json!({"relationalDatabaseSnapshotName": name}),
            )
            .await;
        let operations = deleted["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {deleted:?}"))
            .clone();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {operations:?}"
        );
        let operation = operations[0].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("DeleteRelationalDatabaseSnapshot"),
            "case {case}: expected the DeleteRelationalDatabaseSnapshot operation type, got \
             {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("RelationalDatabaseSnapshot"),
            "case {case}: expected the RelationalDatabaseSnapshot resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(name),
            "case {case}: expected the operation to name the snapshot, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Succeeded"),
            "case {case}: the record is gone as the call returns, so the operation is done, got \
             {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(true),
            "case {case}: expected a terminal operation, got {operation:?}"
        );
        assert_eq!(
            operation["location"], before["location"],
            "case {case}: expected the deleted snapshot's own location {:?}, got {:?}",
            before["location"], operation["location"]
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the deleted snapshot's own region, got {operation:?}"
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
        // The entry is in the account's log, readable back by its id.
        let by_id = emulator
            .ok("GetOperation", json!({"operationId": operation_id}))
            .await["operation"]
            .clone();
        assert_eq!(
            by_id, operation,
            "case {case}: expected GetOperation to answer the same entry, got {by_id:?}"
        );

        // The record is gone, and stays gone however long the clock runs: the
        // transition it was owed went with it.
        assert_absent(&emulator, case, name).await;
        tokio::time::sleep(Duration::from_millis(900)).await;
        assert_absent(&emulator, case, name).await;

        // Deleting a point-in-time copy deletes the copy, never what it was
        // taken of.
        let source = database(&emulator, row.db_name).await;
        assert_eq!(
            source["name"].as_str(),
            Some(row.db_name),
            "case {case}: expected the source database to survive its snapshot's deletion, got \
             {source:?}"
        );
        assert_eq!(
            snapshot(&emulator, "bystander_snapshot").await,
            bystander,
            "case {case}: expected the other stored snapshot to be untouched"
        );
    }
}

/// RULE: a snapshot name addresses one snapshot, so a deletion frees the name — a
/// later `CreateRelationalDatabaseSnapshot` under it stores a NEW record, with its
/// own ARN and its own source, rather than reviving the deleted one.
#[tokio::test]
async fn deleting_a_snapshot_frees_its_name() {
    let emulator = Emulator::new();
    for (db, blueprint, bundle) in [
        ("reused-first", "mysql_8_4", "micro_1_0"),
        ("reused-second", "postgres_17", "small_1_0"),
    ] {
        emulator
            .ok(
                "CreateRelationalDatabase",
                json!({
                    "relationalDatabaseName": db,
                    "relationalDatabaseBlueprintId": blueprint,
                    "relationalDatabaseBundleId": bundle,
                    "masterDatabaseName": "appdata",
                    "masterUsername": "dbmaster",
                }),
            )
            .await;
    }

    let name = "reused-snapshot-name";
    emulator
        .ok(
            "CreateRelationalDatabaseSnapshot",
            json!({
                "relationalDatabaseName": "reused-first",
                "relationalDatabaseSnapshotName": name,
                "tags": [{"key": "generation", "value": "one"}],
            }),
        )
        .await;
    let first = wait_for_snapshot_state(&emulator, name, "available").await;

    emulator
        .ok(
            "DeleteRelationalDatabaseSnapshot",
            json!({"relationalDatabaseSnapshotName": name}),
        )
        .await;
    assert_absent(&emulator, "the name after its snapshot was deleted", name).await;

    // The same name, now of a snapshot of the OTHER database: a fresh record,
    // not the deleted one.
    emulator
        .ok(
            "CreateRelationalDatabaseSnapshot",
            json!({
                "relationalDatabaseName": "reused-second",
                "relationalDatabaseSnapshotName": name,
            }),
        )
        .await;
    let second = wait_for_snapshot_state(&emulator, name, "available").await;
    assert_eq!(
        second["fromRelationalDatabaseName"].as_str(),
        Some("reused-second"),
        "case recreating a deleted name: expected the new source database, got {second:?}"
    );
    assert_eq!(
        second["engine"].as_str(),
        Some("postgres"),
        "case recreating a deleted name: expected the new source's engine, got {second:?}"
    );
    assert_ne!(
        second["arn"], first["arn"],
        "case recreating a deleted name: expected its own ARN, got {second:?}"
    );
    assert_eq!(
        second["tags"].as_array().map(Vec::len),
        Some(0),
        "case recreating a deleted name: expected none of the deleted record's tags, got {:?}",
        second["tags"]
    );
}

/// RULE: each database snapshot is an independent point-in-time copy, so deleting
/// one of a database's snapshots leaves the others readable AND still restorable.
#[tokio::test]
async fn deleting_one_snapshot_of_a_database_leaves_the_others_restorable() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "chain-db",
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": "micro_1_0",
                "masterDatabaseName": "appdata",
                "masterUsername": "dbmaster",
            }),
        )
        .await;
    wait_for_database_state(&emulator, "chain-db", "available").await;

    let chain = ["chain-snapshot-1", "chain_snapshot_2", "chain-snapshot-3"];
    for name in chain {
        emulator
            .ok(
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": "chain-db",
                    "relationalDatabaseSnapshotName": name,
                }),
            )
            .await;
        wait_for_snapshot_state(&emulator, name, "available").await;
    }
    let first = snapshot(&emulator, chain[0]).await;
    let last = snapshot(&emulator, chain[2]).await;

    // The MIDDLE snapshot of the chain goes.
    emulator
        .ok(
            "DeleteRelationalDatabaseSnapshot",
            json!({"relationalDatabaseSnapshotName": chain[1]}),
        )
        .await;
    assert_absent(&emulator, "the deleted middle snapshot", chain[1]).await;

    assert_eq!(
        snapshot(&emulator, chain[0]).await,
        first,
        "case the snapshot taken before the deleted one: expected it untouched"
    );
    assert_eq!(
        snapshot(&emulator, chain[2]).await,
        last,
        "case the snapshot taken after the deleted one: expected it untouched"
    );

    for (case, source) in [
        ("restoring from the older surviving snapshot", chain[0]),
        ("restoring from the newer surviving snapshot", chain[2]),
    ] {
        let restored = format!("restored-from-{source}");
        emulator
            .ok(
                "CreateRelationalDatabaseFromSnapshot",
                json!({
                    "relationalDatabaseName": restored,
                    "relationalDatabaseSnapshotName": source,
                }),
            )
            .await;
        let read = database(&emulator, &restored).await;
        assert_eq!(
            read["engine"].as_str(),
            Some("mysql"),
            "case {case}: expected the snapshotted engine on the restored database, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// What a refusal row names.
struct Refusal {
    case: &'static str,
    /// The `relationalDatabaseSnapshotName` the request carries. Every one
    /// satisfies the modelled `ResourceName` pattern `^\w[\w\-]*\w$`, so the
    /// centralized walk passes it through to the handler.
    name: &'static str,
}

/// The names this account holds no DATABASE SNAPSHOT under: one never created,
/// two close to one that exists, one already deleted, the name of a managed
/// relational DATABASE, and the name of a DISK SNAPSHOT — a different resource
/// type, which this operation must not delete.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a name never created",
        name: "absent-snapshot",
    },
    Refusal {
        case: "a name close to one that exists",
        name: "present-snapshot-2",
    },
    Refusal {
        case: "the shortest name the pattern admits",
        name: "ab",
    },
    Refusal {
        case: "a long name in another form",
        name: "a_name_with_many_underscore_separated_words_and_digits_0123456789",
    },
    Refusal {
        case: "the already-deleted snapshot",
        name: "deleted-snapshot",
    },
    Refusal {
        case: "the name of a managed relational database",
        name: "present-db",
    },
    Refusal {
        case: "the name of a disk snapshot",
        name: "present-disk-snapshot",
    },
];

/// RULE: a well-formed name this account holds no database snapshot under is
/// refused with the service's `DoesNotExist` envelope — HTTP 400,
/// `NotFoundException`, body `code` `DoesNotExist` and the resource-type/name
/// message — and deletes nothing, so a second delete of one snapshot fails rather
/// than reporting a deletion that did not happen.
#[tokio::test]
async fn a_name_the_account_holds_no_database_snapshot_under_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "present-db",
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": "micro_1_0",
                "masterDatabaseName": "appdata",
                "masterUsername": "dbmaster",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": "present-disk", "availabilityZone": "us-east-1a", "sizeInGb": 8}),
        )
        .await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskSnapshotName": "present-disk-snapshot", "diskName": "present-disk"}),
        )
        .await;
    for name in ["present-snapshot", "deleted-snapshot"] {
        emulator
            .ok(
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": "present-db",
                    "relationalDatabaseSnapshotName": name,
                }),
            )
            .await;
    }
    let kept = wait_for_snapshot_state(&emulator, "present-snapshot", "available").await;
    emulator
        .ok(
            "DeleteRelationalDatabaseSnapshot",
            json!({"relationalDatabaseSnapshotName": "deleted-snapshot"}),
        )
        .await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let (status, body) = emulator
            .call(
                "DeleteRelationalDatabaseSnapshot",
                json!({"relationalDatabaseSnapshotName": refusal.name}),
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
            Some(
                format!(
                    "The RelationalDatabaseSnapshot does not exist: {}",
                    refusal.name
                )
                .as_str()
            ),
            "case {case}: expected the resource-type/name message, got {body:?}"
        );
    }

    // Nothing the refusals named was removed, and the one snapshot that was
    // never deleted is exactly as it was.
    assert_eq!(
        snapshot(&emulator, "present-snapshot").await,
        kept,
        "case after every refusal: expected the stored snapshot to be untouched"
    );
    let source = database(&emulator, "present-db").await;
    assert_eq!(
        source["name"].as_str(),
        Some("present-db"),
        "case after every refusal: expected the database to be untouched, got {source:?}"
    );
    let disk_snapshot = emulator
        .ok(
            "GetDiskSnapshot",
            json!({"diskSnapshotName": "present-disk-snapshot"}),
        )
        .await["diskSnapshot"]
        .clone();
    assert_eq!(
        disk_snapshot["name"].as_str(),
        Some("present-disk-snapshot"),
        "case after every refusal: expected the disk snapshot to be untouched, got \
         {disk_snapshot:?}"
    );
}

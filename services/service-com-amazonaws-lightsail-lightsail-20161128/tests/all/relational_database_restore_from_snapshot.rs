// SPDX-License-Identifier: CC0-1.0

//! Restoring a Lightsail managed relational database from a database SNAPSHOT —
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `CreateRelationalDatabaseFromSnapshot` persists ONE new
//! managed database restored from a point-in-time copy of another one — the
//! engine, the engine version and the master credentials are the ones that copy
//! CAPTURED, the hardware is the bundle the request names (never one smaller
//! than the source's) or the source's own bundle when the request names none,
//! the database is placed in the zone the request names or in one its Region
//! publishes, its identity (ARN, support code, creation time, master endpoint)
//! is newly minted rather than the source's, the request's tags and
//! accessibility flag are kept verbatim, and it is created `creating` with the
//! CLOCK — not a reader — settling it into `available`. The call answers exactly
//! one non-terminal `CreateRelationalDatabaseFromSnapshot` operation-log entry,
//! and a request outside what the service restores is refused and stores
//! nothing.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The Region a request that carries no credential scope is served in
/// (`crate::state::DEFAULT_REGION`).
const DEFAULT_REGION: &str = "us-east-1";

/// The suffix every announced master endpoint carries
/// (`crate::state::RELATIONAL_DATABASE_ENDPOINT_SUFFIX`).
const ENDPOINT_SUFFIX: &str = "rds.lightsail.local";

/// The port a database of `engine` must listen on: the engine's own default
/// (MySQL 3306, PostgreSQL 5432).
fn want_port(engine: &str) -> i64 {
    if engine.contains("postgres") {
        5432
    } else {
        3306
    }
}

/// The three-letter day tokens a weekly maintenance window names
/// (`crate::state::MAINTENANCE_WINDOW_DAYS`).
const WINDOW_DAYS: &[&str] = &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Whether `value` is a well-formed `hh24:mi` time of day.
fn is_time_of_day(value: &str) -> bool {
    let Some((hours, minutes)) = value.split_once(':') else {
        return false;
    };
    hours.len() == 2
        && minutes.len() == 2
        && hours.parse::<u32>().is_ok_and(|hours| hours <= 23)
        && minutes.parse::<u32>().is_ok_and(|minutes| minutes <= 59)
}

/// Whether `value` is a well-formed daily window, `hh24:mi-hh24:mi`.
fn is_daily_window(value: &str) -> bool {
    match value.split_once('-') {
        Some((from, to)) => is_time_of_day(from) && is_time_of_day(to),
        None => false,
    }
}

/// Whether `value` is a well-formed weekly window, `ddd:hh24:mi-ddd:hh24:mi`.
fn is_weekly_window(value: &str) -> bool {
    let Some((from, to)) = value.split_once('-') else {
        return false;
    };
    [from, to].iter().all(|part| match part.split_once(':') {
        Some((day, time)) => WINDOW_DAYS.contains(&day) && is_time_of_day(time),
        None => false,
    })
}

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

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

/// Poll `GetRelationalDatabase` until it reports `want`, answering the database.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_database(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let database = read_database(emulator, name).await;
        if database["state"].as_str() == Some(want) {
            return database;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            database["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The stored database snapshot `name`, as `GetRelationalDatabaseSnapshots`
/// reports it — the implemented reader over the snapshot store.
async fn read_snapshot(emulator: &Emulator, name: &str) -> Value {
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
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
        let page = body["relationalDatabaseSnapshots"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case reading the snapshot listing: expected an array, got {body:?}")
            })
            .clone();
        if let Some(found) = page
            .iter()
            .find(|entry| entry["name"].as_str() == Some(name))
        {
            return found.clone();
        }
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => panic!("case reading snapshot {name}: the listing reports no such snapshot"),
        }
    }
}

/// Poll the snapshot listing until `name` reports `want`, answering it.
async fn wait_for_snapshot(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = read_snapshot(emulator, name).await;
        if snapshot["state"].as_str() == Some(want) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on snapshot {name}: expected it within 5s, got {:?}",
            snapshot["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The published bundle `bundle_id` names, as `GetRelationalDatabaseBundles`
/// reports it.
async fn published_bundle(emulator: &Emulator, bundle_id: &str) -> Value {
    let body = emulator.ok("GetRelationalDatabaseBundles", json!({})).await;
    body["bundles"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading the bundle catalog: expected an array, got {body:?}"))
        .iter()
        .find(|bundle| bundle["bundleId"].as_str() == Some(bundle_id))
        .unwrap_or_else(|| {
            panic!("case reading the bundle catalog: expected an entry for {bundle_id}, got {body:?}")
        })
        .clone()
}

// ---------------------------------------------------------------------------
// The restore table
// ---------------------------------------------------------------------------

/// One restore the service must serve.
///
/// The rows differ along everything the rule generalises over: the engine of the
/// source (and so the version, the per-engine port and the credential spelling),
/// whether the restore names a bundle at all and whether that bundle is the
/// source's own, a larger one, its high-availability twin or the standard plan
/// of an HA source (and so whether a standby zone is placed), whether the
/// restore names an Availability Zone, the Region the call is signed for, the
/// accessibility flag's three forms and the number of tags.
struct Row {
    case: &'static str,
    /// The source database the snapshot is taken of.
    source_name: &'static str,
    source_blueprint_id: &'static str,
    source_bundle_id: &'static str,
    master_database_name: &'static str,
    master_username: &'static str,
    /// The Region every call of the row is signed for, or `None` for calls that
    /// carry no credential scope at all.
    region: Option<&'static str>,
    source_zone: Option<&'static str>,
    snapshot_name: &'static str,
    /// The restore request.
    restored_name: &'static str,
    restore_zone: Option<&'static str>,
    restore_bundle_id: Option<&'static str>,
    publicly_accessible: Option<bool>,
    /// The tags the restore carries, as `(key, value)` — a value of `None` is a
    /// key-only tag.
    tags: &'static [(&'static str, Option<&'static str>)],
    /// What the restored database must report.
    reported_region: &'static str,
    want_bundle_id: &'static str,
}

/// The restore cases. Blueprint ids and bundle ids name entries of this
/// service's own published catalogs, which every case reads back through
/// `GetRelationalDatabaseBundles` rather than hard-coding the hardware.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "mysql, nothing optional at all: the source's own plan and an unsigned call",
            source_name: "src-mysql",
            source_blueprint_id: "mysql_8_4",
            source_bundle_id: "micro_1_0",
            master_database_name: "appdb",
            master_username: "dbadmin",
            region: None,
            source_zone: None,
            snapshot_name: "src-mysql-snap",
            restored_name: "restored-mysql",
            restore_zone: None,
            restore_bundle_id: None,
            publicly_accessible: None,
            tags: &[],
            reported_region: DEFAULT_REGION,
            want_bundle_id: "micro_1_0",
        },
        Row {
            case: "postgres 13, restored onto a larger plan in a named zone, public, two tags",
            source_name: "src-postgres",
            source_blueprint_id: "postgres_13",
            source_bundle_id: "micro_1_0",
            master_database_name: "orders",
            master_username: "orders_admin",
            region: Some("us-west-2"),
            source_zone: Some("us-west-2b"),
            snapshot_name: "src-postgres-snap",
            restored_name: "restored_postgres",
            restore_zone: Some("us-west-2c"),
            restore_bundle_id: Some("large_1_0"),
            publicly_accessible: Some(true),
            tags: &[("stage", Some("prod")), ("owner", Some("platform"))],
            reported_region: "us-west-2",
            want_bundle_id: "large_1_0",
        },
        Row {
            case: "mysql restored onto the high-availability twin of the source's plan: a standby \
                   zone appears",
            source_name: "src-ha-target",
            source_blueprint_id: "mysql_8_4",
            source_bundle_id: "small_1_0",
            master_database_name: "billing",
            master_username: "billing_rw",
            region: Some("eu-west-1"),
            source_zone: Some("eu-west-1a"),
            snapshot_name: "src-ha-target-snap",
            restored_name: "restored-ha",
            restore_zone: Some("eu-west-1b"),
            // Same hardware as `small_1_0`, with a standby: not a SMALLER
            // bundle, so the restore is legal.
            restore_bundle_id: Some("small_ha_1_0"),
            publicly_accessible: Some(false),
            tags: &[("key-only", None)],
            reported_region: "eu-west-1",
            want_bundle_id: "small_ha_1_0",
        },
        Row {
            case: "postgres 17 on a high-availability source restored onto the standard plan of \
                   the same size, zone picked by the service",
            source_name: "src-ha-source",
            source_blueprint_id: "postgres_17",
            source_bundle_id: "medium_ha_1_0",
            master_database_name: "analytics_2",
            master_username: "analyst",
            region: Some("ap-northeast-1"),
            source_zone: None,
            snapshot_name: "src-ha-source-snap",
            restored_name: "restored-standard",
            restore_zone: None,
            // Same hardware as `medium_ha_1_0` without the standby: again not
            // smaller, so dropping high availability on a restore is legal.
            restore_bundle_id: Some("medium_1_0"),
            publicly_accessible: Some(true),
            tags: &[("app", Some("reports"))],
            reported_region: "ap-northeast-1",
            want_bundle_id: "medium_1_0",
        },
    ]
}

/// The tags `tags` describes, as the wire reports them back.
fn tag_values(tags: &[(&str, Option<&str>)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| match value {
                Some(value) => json!({"key": key, "value": value}),
                None => json!({"key": key}),
            })
            .collect(),
    )
}

/// Create `row`'s source database, snapshot it, and answer the source database
/// and the snapshot once both are settled.
async fn source_and_snapshot(emulator: &Emulator, row: &Row) -> (Value, Value) {
    let mut body = json!({
        "relationalDatabaseName": row.source_name,
        "relationalDatabaseBlueprintId": row.source_blueprint_id,
        "relationalDatabaseBundleId": row.source_bundle_id,
        "masterDatabaseName": row.master_database_name,
        "masterUsername": row.master_username,
    });
    if let Some(zone) = row.source_zone {
        body["availabilityZone"] = json!(zone);
    }
    emulator
        .ok_at(None, row.region, "CreateRelationalDatabase", body)
        .await;
    let source = wait_for_database(emulator, row.source_name, "available").await;

    emulator
        .ok_at(
            None,
            row.region,
            "CreateRelationalDatabaseSnapshot",
            json!({
                "relationalDatabaseName": row.source_name,
                "relationalDatabaseSnapshotName": row.snapshot_name,
            }),
        )
        .await;
    let snapshot = wait_for_snapshot(emulator, row.snapshot_name, "available").await;
    (source, snapshot)
}

/// The `CreateRelationalDatabaseFromSnapshot` body `row` describes.
fn restore_body(row: &Row) -> Value {
    let mut body = json!({
        "relationalDatabaseName": row.restored_name,
        "relationalDatabaseSnapshotName": row.snapshot_name,
    });
    if let Some(zone) = row.restore_zone {
        body["availabilityZone"] = json!(zone);
    }
    if let Some(bundle_id) = row.restore_bundle_id {
        body["relationalDatabaseBundleId"] = json!(bundle_id);
    }
    if let Some(publicly_accessible) = row.publicly_accessible {
        body["publiclyAccessible"] = json!(publicly_accessible);
    }
    if !row.tags.is_empty() {
        body["tags"] = tag_values(row.tags);
    }
    body
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: a restore persists ONE new database that inherits the engine, the
/// version and the master credentials the snapshot captured, runs on the bundle
/// the request names or on the source's own, carries the request's own tags,
/// zone and accessibility, and gets an identity of its own — and
/// `GetRelationalDatabase` reports that record back.
#[tokio::test]
async fn a_restored_database_is_read_back_exactly() {
    for row in rows() {
        let emulator = Emulator::new();
        let Row {
            case,
            restored_name,
            reported_region,
            want_bundle_id,
            ..
        } = &row;

        let (source, snapshot) = source_and_snapshot(&emulator, &row).await;

        let restored = emulator
            .ok_at(
                None,
                row.region,
                "CreateRelationalDatabaseFromSnapshot",
                restore_body(&row),
            )
            .await;

        // The restore answers ONE operation-log entry, left non-terminal until
        // the provisioning the clock owes the database is due.
        let operations = restored["operations"].as_array().unwrap_or_else(|| {
            panic!("case {case}: expected an operations array, got {restored:?}")
        });
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {restored:?}"
        );
        let operation = &operations[0];
        for (member, want) in [
            ("operationType", "CreateRelationalDatabaseFromSnapshot"),
            ("resourceType", "RelationalDatabase"),
            ("resourceName", restored_name),
            ("status", "Started"),
        ] {
            assert_eq!(
                operation[member].as_str(),
                Some(want),
                "case {case}: expected {member} {want}, got {operation:?}"
            );
        }
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(false),
            "case {case}: the restore is still running when it answers, got {operation:?}"
        );
        assert!(
            operation["id"].as_str().is_some_and(is_uuid_shaped),
            "case {case}: expected a uuid-shaped operation id, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(*reported_region),
            "case {case}: expected the operation logged in the restored database's region, got \
             {operation:?}"
        );

        // A restored database is created `creating` and announces no endpoint
        // yet: a caller reading straight after the restore sees the transient
        // state.
        let creating = read_database(&emulator, restored_name).await;
        assert_eq!(
            creating["state"].as_str(),
            Some("creating"),
            "case {case}: expected the restored database to be created `creating`, got \
             {creating:?}"
        );
        assert!(
            creating["masterEndpoint"].is_null(),
            "case {case}: a database that is still being restored accepts no connection, so it \
             announces no endpoint, got {creating:?}"
        );

        // The clock, not the reader, settles it.
        let database = wait_for_database(&emulator, restored_name, "available").await;

        // What the SNAPSHOT captured: the engine, the version and the master
        // credentials of the database it was taken of.
        for member in ["engine", "engineVersion"] {
            assert_eq!(
                database[member], snapshot[member],
                "case {case}: expected {member} to be the one the snapshot captured, got \
                 {database:?} against {snapshot:?}"
            );
        }
        for member in [
            "engine",
            "engineVersion",
            "masterDatabaseName",
            "masterUsername",
            "relationalDatabaseBlueprintId",
        ] {
            assert_eq!(
                database[member], source[member],
                "case {case}: expected {member} to be the source database's, got {database:?} \
                 against {source:?}"
            );
        }

        // What the caller sent, verbatim.
        assert_eq!(
            database["name"].as_str(),
            Some(*restored_name),
            "case {case}: expected the name the restore asked for, got {database:?}"
        );
        assert_eq!(
            database["relationalDatabaseBundleId"].as_str(),
            Some(*want_bundle_id),
            "case {case}: expected the bundle the restore named, else the source's own, got \
             {database:?}"
        );
        assert_eq!(
            database["publiclyAccessible"].as_bool(),
            Some(row.publicly_accessible.unwrap_or(false)),
            "case {case}: expected the accessibility the restore asked for, defaulting to the \
             account-only one, got {database:?}"
        );
        assert_eq!(
            database["tags"],
            tag_values(row.tags),
            "case {case}: expected the tags the restore carried, got {database:?}"
        );

        // The hardware is the published bundle's, never the request's spelling.
        let bundle = published_bundle(&emulator, want_bundle_id).await;
        for member in ["cpuCount", "diskSizeInGb", "ramSizeInGb"] {
            assert_eq!(
                database["hardware"][member], bundle[member],
                "case {case}: expected hardware.{member} to be the published bundle's, got \
                 {database:?} against {bundle:?}"
            );
        }

        // Identity: minted for the RESTORED database, not copied from the
        // source.
        let arn = database["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an arn, got {database:?}"));
        let want_prefix =
            format!("arn:aws:lightsail:{reported_region}:{ACCOUNT}:RelationalDatabase/");
        let id = arn.strip_prefix(&want_prefix).unwrap_or_else(|| {
            panic!("case {case}: expected an arn under {want_prefix}, got {arn}")
        });
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped database id in the arn, got {arn}"
        );
        assert_eq!(
            database["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: expected the support code to carry the same id as the arn, got \
             {database:?}"
        );
        assert_ne!(
            database["arn"], source["arn"],
            "case {case}: a restored database is a resource of its own, got {database:?} against \
             {source:?}"
        );
        assert_ne!(
            database["supportCode"], source["supportCode"],
            "case {case}: a restored database carries its own support code, got {database:?} \
             against {source:?}"
        );
        assert_eq!(
            database["resourceType"].as_str(),
            Some("RelationalDatabase"),
            "case {case}: expected the modelled resource type, got {database:?}"
        );
        assert!(
            database["createdAt"].as_f64().is_some_and(|at| at > 0.0),
            "case {case}: expected a creation timestamp, got {database:?}"
        );

        // Location: the zone the restore named, else one of the Region's own.
        assert_eq!(
            database["location"]["regionName"].as_str(),
            Some(*reported_region),
            "case {case}: expected the region of the zone the restore named, else the signed \
             region, got {database:?}"
        );
        let zone = database["location"]["availabilityZone"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: expected an availability zone, got {database:?}")
            });
        match row.restore_zone {
            Some(want) => assert_eq!(
                zone, want,
                "case {case}: expected the zone the restore named, got {database:?}"
            ),
            None => assert!(
                zone.starts_with(reported_region) && zone.len() == reported_region.len() + 1,
                "case {case}: expected a zone of {reported_region}, got {zone}"
            ),
        }

        // A standby is placed only on a high-availability bundle — the
        // RESTORED database's own, not the source's.
        match database["secondaryAvailabilityZone"].as_str() {
            Some(secondary) => {
                assert!(
                    want_bundle_id.contains("_ha_"),
                    "case {case}: only a high-availability bundle places a standby, got \
                     {database:?}"
                );
                assert_ne!(
                    secondary, zone,
                    "case {case}: a standby runs in a second zone, got {database:?}"
                );
                assert!(
                    secondary.starts_with(reported_region),
                    "case {case}: expected the standby in {reported_region}, got {secondary}"
                );
            }
            None => assert!(
                !want_bundle_id.contains("_ha_"),
                "case {case}: a high-availability bundle must report its standby zone, got \
                 {database:?}"
            ),
        }

        // The endpoint a client connects to is announced once the restored
        // database is serving, on the engine's own port, and it is the restored
        // database's own address.
        let engine = database["engine"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            database["masterEndpoint"]["port"].as_i64(),
            Some(want_port(&engine)),
            "case {case}: expected the {engine} default port, got {database:?}"
        );
        let address = database["masterEndpoint"]["address"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: expected a master endpoint address, got {database:?}")
            });
        assert!(
            address.ends_with(&format!(".{reported_region}.{ENDPOINT_SUFFIX}"))
                && address.starts_with("ls-"),
            "case {case}: expected the endpoint of a Lightsail-managed instance in \
             {reported_region} under {ENDPOINT_SUFFIX}, got {address}"
        );
        assert_ne!(
            database["masterEndpoint"]["address"], source["masterEndpoint"]["address"],
            "case {case}: two databases never share a master endpoint, got {database:?} against \
             {source:?}"
        );

        // Nothing was modified about the database or the snapshot the restore
        // read: a restore is a create of a THIRD record.
        assert!(
            database["preferredBackupWindow"]
                .as_str()
                .is_some_and(is_daily_window),
            "case {case}: expected a well-formed daily backup window, got {database:?}"
        );
        assert!(
            database["preferredMaintenanceWindow"]
                .as_str()
                .is_some_and(is_weekly_window),
            "case {case}: expected a well-formed weekly maintenance window, got {database:?}"
        );
        assert_eq!(
            database["backupRetentionEnabled"].as_bool(),
            Some(true),
            "case {case}: a database is created with automated backup retention on, got \
             {database:?}"
        );
        assert_eq!(
            database["parameterApplyStatus"].as_str(),
            Some("in-sync"),
            "case {case}: a restored database runs the engine's own defaults, got {database:?}"
        );
        assert_eq!(
            read_database(&emulator, row.source_name).await,
            source,
            "case {case}: the restore must leave the source database untouched"
        );
        assert_eq!(
            read_snapshot(&emulator, row.snapshot_name).await,
            snapshot,
            "case {case}: the restore must leave the snapshot it read untouched"
        );
    }
}

/// RULE: a snapshot is restorable as often as the account likes, and each
/// restore is an independent database: the inherited values are identical and
/// every minted one differs.
#[tokio::test]
async fn two_restores_of_one_snapshot_are_independent_databases() {
    let emulator = Emulator::new();
    let row = Row {
        case: "two restores of one snapshot",
        source_name: "twice-src",
        source_blueprint_id: "postgres_15",
        source_bundle_id: "small_1_0",
        master_database_name: "twice_db",
        master_username: "twice_admin",
        region: Some("eu-central-1"),
        source_zone: Some("eu-central-1a"),
        snapshot_name: "twice-src-snap",
        restored_name: "twice-first",
        restore_zone: None,
        restore_bundle_id: None,
        publicly_accessible: None,
        tags: &[],
        reported_region: "eu-central-1",
        want_bundle_id: "small_1_0",
    };
    source_and_snapshot(&emulator, &row).await;

    let mut restored = Vec::new();
    for name in ["twice-first", "twice-second"] {
        emulator
            .ok_at(
                None,
                row.region,
                "CreateRelationalDatabaseFromSnapshot",
                json!({
                    "relationalDatabaseName": name,
                    "relationalDatabaseSnapshotName": row.snapshot_name,
                }),
            )
            .await;
        restored.push(wait_for_database(&emulator, name, "available").await);
    }

    // What both restores inherited from the one snapshot.
    for member in [
        "engine",
        "engineVersion",
        "masterDatabaseName",
        "masterUsername",
        "relationalDatabaseBlueprintId",
        "relationalDatabaseBundleId",
    ] {
        assert_eq!(
            restored[0][member], restored[1][member],
            "case two restores of one snapshot: expected the same {member} on both, got {:?} and \
             {:?}",
            restored[0], restored[1]
        );
    }
    // What each restore minted for itself.
    for member in ["name", "arn", "supportCode"] {
        assert_ne!(
            restored[0][member], restored[1][member],
            "case two restores of one snapshot: expected a distinct {member} per database, got \
             {:?} and {:?}",
            restored[0], restored[1]
        );
    }
    assert_ne!(
        restored[0]["masterEndpoint"]["address"], restored[1]["masterEndpoint"]["address"],
        "case two restores of one snapshot: two databases never share a master endpoint, got {:?} \
         and {:?}",
        restored[0], restored[1]
    );
}

/// RULE: a snapshot still being TAKEN holds no complete copy, so there is
/// nothing to restore from until the copy the clock owes it is done — and once
/// it is, the very same restore succeeds.
#[tokio::test]
async fn restoring_from_a_snapshot_still_being_taken_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "settling-src",
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": "micro_1_0",
                "masterDatabaseName": "appdb",
                "masterUsername": "dbadmin",
            }),
        )
        .await;
    wait_for_database(&emulator, "settling-src", "available").await;
    emulator
        .ok(
            "CreateRelationalDatabaseSnapshot",
            json!({
                "relationalDatabaseName": "settling-src",
                "relationalDatabaseSnapshotName": "settling-snap",
            }),
        )
        .await;

    let restore = json!({
        "relationalDatabaseName": "settling-restore",
        "relationalDatabaseSnapshotName": "settling-snap",
    });
    let (status, refused) = emulator
        .call("CreateRelationalDatabaseFromSnapshot", restore.clone())
        .await;
    assert_eq!(
        status, 400,
        "case restoring from a snapshot that is still creating: expected a refusal, got \
         {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidInputException",
        "case restoring from a snapshot that is still creating: expected the modelled refusal, \
         got {refused:?}"
    );
    let (status, missing) = emulator
        .call(
            "GetRelationalDatabase",
            json!({"relationalDatabaseName": "settling-restore"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&missing).as_str()),
        (400, "NotFoundException"),
        "case restoring from a snapshot that is still creating: a refused restore stores nothing, \
         got {missing:?}"
    );

    // The clock settles the snapshot, and then the same request is served.
    wait_for_snapshot(&emulator, "settling-snap", "available").await;
    emulator
        .ok("CreateRelationalDatabaseFromSnapshot", restore)
        .await;
    let database = wait_for_database(&emulator, "settling-restore", "available").await;
    assert_eq!(
        database["relationalDatabaseBundleId"].as_str(),
        Some("micro_1_0"),
        "case restoring once the snapshot is available: expected the source's own bundle, got \
         {database:?}"
    );
}

/// RULE: every restore the service does not serve is refused with a modelled
/// error and stores NOTHING — the member combinations the model admits but the
/// documentation forbids, a bundle smaller than the source's, a source or a
/// catalog entry this account does not hold, a snapshot of another Region, a
/// name already taken, and a point-in-time restore of automated backups this
/// account has never taken.
#[tokio::test]
async fn a_restore_outside_what_the_service_serves_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "src-db",
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": "small_1_0",
                "masterDatabaseName": "appdb",
                "masterUsername": "dbadmin",
                "availabilityZone": "us-east-1a",
            }),
        )
        .await;
    let source = wait_for_database(&emulator, "src-db", "available").await;
    emulator
        .ok(
            "CreateRelationalDatabaseSnapshot",
            json!({
                "relationalDatabaseName": "src-db",
                "relationalDatabaseSnapshotName": "src-snap",
            }),
        )
        .await;
    let snapshot = wait_for_snapshot(&emulator, "src-snap", "available").await;

    // The name no case may leave behind. Only the "name already taken" case
    // names anything else, and it names a database that already exists.
    let fresh = "restored-db";
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a bundle smaller than the source's",
            json!({
                "relationalDatabaseName": fresh,
                "relationalDatabaseSnapshotName": "src-snap",
                "relationalDatabaseBundleId": "micro_1_0",
            }),
            "InvalidInputException",
        ),
        (
            "a bundle this service does not publish",
            json!({
                "relationalDatabaseName": fresh,
                "relationalDatabaseSnapshotName": "src-snap",
                "relationalDatabaseBundleId": "enormous_9_9",
            }),
            "InvalidInputException",
        ),
        (
            "a snapshot this account does not hold",
            json!({
                "relationalDatabaseName": fresh,
                "relationalDatabaseSnapshotName": "no-such-snap",
            }),
            "NotFoundException",
        ),
        (
            "both sources named at once",
            json!({
                "relationalDatabaseName": fresh,
                "relationalDatabaseSnapshotName": "src-snap",
                "sourceRelationalDatabaseName": "src-db",
                "useLatestRestorableTime": true,
            }),
            "InvalidInputException",
        ),
        (
            "no source named at all",
            json!({"relationalDatabaseName": fresh}),
            "InvalidInputException",
        ),
        (
            "both point-in-time members named",
            json!({
                "relationalDatabaseName": fresh,
                "sourceRelationalDatabaseName": "src-db",
                "restoreTime": 1_538_424_000_i64,
                "useLatestRestorableTime": true,
            }),
            "InvalidInputException",
        ),
        (
            "a point-in-time member together with a snapshot",
            json!({
                "relationalDatabaseName": fresh,
                "relationalDatabaseSnapshotName": "src-snap",
                "useLatestRestorableTime": true,
            }),
            "InvalidInputException",
        ),
        (
            "a source database with neither point-in-time member",
            json!({
                "relationalDatabaseName": fresh,
                "sourceRelationalDatabaseName": "src-db",
            }),
            "InvalidInputException",
        ),
        (
            "a point in time of a database with no automated backup taken",
            json!({
                "relationalDatabaseName": fresh,
                "sourceRelationalDatabaseName": "src-db",
                "restoreTime": 1_538_424_000_i64,
            }),
            "NotFoundException",
        ),
        (
            "the latest restorable time of a database this account does not hold",
            json!({
                "relationalDatabaseName": fresh,
                "sourceRelationalDatabaseName": "no-such-db",
                "useLatestRestorableTime": true,
            }),
            "NotFoundException",
        ),
        (
            "a zone this service does not publish",
            json!({
                "relationalDatabaseName": fresh,
                "relationalDatabaseSnapshotName": "src-snap",
                "availabilityZone": "us-east-1z",
            }),
            "InvalidInputException",
        ),
        (
            "a snapshot of another region",
            json!({
                "relationalDatabaseName": fresh,
                "relationalDatabaseSnapshotName": "src-snap",
                "availabilityZone": "us-west-2a",
            }),
            "InvalidInputException",
        ),
        (
            "a database name this account already holds",
            json!({
                "relationalDatabaseName": "src-db",
                "relationalDatabaseSnapshotName": "src-snap",
            }),
            "InvalidInputException",
        ),
    ];

    for (case, body, want_code) in cases {
        let (status, refused) = emulator
            .call("CreateRelationalDatabaseFromSnapshot", body)
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the request refused, got {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            want_code,
            "case {case}: expected {want_code}, got {refused:?}"
        );
        // A refused restore creates nothing and changes nothing it read.
        let (status, missing) = emulator
            .call(
                "GetRelationalDatabase",
                json!({"relationalDatabaseName": fresh}),
            )
            .await;
        assert_eq!(
            (status, error_code(&missing).as_str()),
            (400, "NotFoundException"),
            "case {case}: a refused restore stores nothing, got {missing:?}"
        );
        assert_eq!(
            read_database(&emulator, "src-db").await,
            source,
            "case {case}: a refused restore leaves the source database untouched"
        );
        assert_eq!(
            read_snapshot(&emulator, "src-snap").await,
            snapshot,
            "case {case}: a refused restore leaves the snapshot untouched"
        );
    }
}

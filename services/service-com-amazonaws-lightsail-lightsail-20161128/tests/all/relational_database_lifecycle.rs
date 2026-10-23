// SPDX-License-Identifier: CC0-1.0

//! The Lightsail managed relational database — create and read — driven over the
//! wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `CreateRelationalDatabase` persists ONE managed database
//! per `relationalDatabaseName` the request names — the name, zone, blueprint,
//! bundle, master credentials, time windows, accessibility and tags the request
//! carried verbatim, every server-assigned member (ARN, support code, creation
//! time, location, resource type, engine, engine version, hardware, state, master
//! endpoint, standby zone, backup retention, parameter status, certificate)
//! computed from the request plus this service's published catalogs and the
//! store's own allocators — so a later `GetRelationalDatabase` reports exactly
//! that record; the database is created `creating` and the clock, not a reader,
//! settles it into `available`, which is when it announces the endpoint a client
//! connects to; a request outside what Lightsail creates (a blueprint or bundle
//! the service does not publish, a zone no Lightsail region has, master
//! credentials outside the chosen ENGINE's own constraints, a malformed or
//! conflicting time window, a name the account already holds) is refused and
//! stores nothing; and a name that was never created is refused with the
//! service's `DoesNotExist` envelope.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The region a request that carries no SigV4 credential scope is served in
/// (`crate::state::DEFAULT_REGION`).
const DEFAULT_REGION: &str = "us-east-1";

/// The fewest minutes a backup or maintenance window may span, from the modelled
/// `preferredBackupWindow` / `preferredMaintenanceWindow` documentation: "Must be
/// at least 30 minutes."
const MIN_WINDOW_MINUTES: i64 = 30;

/// The days a weekly maintenance window may name, from the modelled
/// `preferredMaintenanceWindow` documentation: "Valid days: Mon, Tue, Wed, Thu,
/// Fri, Sat, Sun."
const MAINTENANCE_WINDOW_DAYS: &[&str] = &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Minutes in a day: the clock a daily backup window is measured on.
const MINUTES_PER_DAY: i64 = 24 * 60;

/// The DNS suffix a master endpoint address must carry: a suffix this emulator
/// owns, under `.local`, which RFC 6762 §3 reserves for link-local name
/// resolution — so the announced address can never reach a host off this
/// machine, where AWS's own `rds.amazonaws.com` name would.
const ENDPOINT_SUFFIX: &str = "rds.lightsail.local";

/// The port a database of `engine` must listen on: the engine's own default
/// (MySQL 3306, PostgreSQL 5432), the ports RDS documents as the default for each
/// engine under `CreateDBInstance$Port`. Lightsail exposes no port parameter, so
/// a database always listens on its engine's default.
fn want_port(engine: &str) -> i64 {
    if engine.contains("postgres") {
        5432
    } else {
        3306
    }
}

/// An engine identifier of exactly `chars` characters: a letter first, then
/// letters, as both `masterDatabaseName` and `masterUsername` require.
fn identifier_of(chars: usize) -> String {
    std::iter::once('d')
        .chain(std::iter::repeat('x'))
        .take(chars)
        .collect()
}

/// A password of exactly `chars` characters, drawn only from characters the
/// modelled `masterUserPassword` documentation admits (printable ASCII, no space
/// and none of `/`, `"`, `@`).
fn password_of(chars: usize) -> String {
    "Pw9#x!Zq".chars().cycle().take(chars).collect()
}

// ---------------------------------------------------------------------------
// The create/read table
// ---------------------------------------------------------------------------

/// One `CreateRelationalDatabase` request the service must serve.
///
/// The rows differ along everything the rule generalises over: the engine (and
/// so the per-engine credential limits, the version and the endpoint port), the
/// bundle (and so the hardware, and whether a standby zone is placed), whether
/// the request names an Availability Zone at all, whether the zone's region and
/// the signed region agree, which of the two time windows the request names, the
/// accessibility flag's three forms, the number of tags and the length of the
/// master credentials.
struct Row {
    case: &'static str,
    db_name: &'static str,
    /// The region the request is signed for, or `None` for a request that
    /// carries no credential scope at all.
    region: Option<&'static str>,
    /// The region the database must report: the region of the zone the request
    /// names, else the signed region, else [`DEFAULT_REGION`].
    reported_region: &'static str,
    availability_zone: Option<&'static str>,
    blueprint_id: &'static str,
    bundle_id: &'static str,
    master_database_name: String,
    master_username: String,
    master_user_password: Option<String>,
    preferred_backup_window: Option<&'static str>,
    preferred_maintenance_window: Option<&'static str>,
    publicly_accessible: Option<bool>,
    /// The tags the request carries, as `(key, value)` — a value of `None` is a
    /// key-only tag.
    tags: &'static [(&'static str, Option<&'static str>)],
}

/// The create cases. Blueprint ids and bundle ids name entries of this service's
/// own published catalogs, which every case reads back through
/// `GetRelationalDatabaseBlueprints` / `GetRelationalDatabaseBundles` rather than
/// hard-coding the engine, the version or the hardware.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "mysql on the cheapest plan, nothing optional at all",
            db_name: "db-one",
            region: None,
            reported_region: DEFAULT_REGION,
            availability_zone: None,
            blueprint_id: "mysql_8_4",
            bundle_id: "micro_1_0",
            master_database_name: "appdb".to_string(),
            master_username: "dbadmin".to_string(),
            master_user_password: None,
            preferred_backup_window: None,
            preferred_maintenance_window: None,
            publicly_accessible: None,
            tags: &[],
        },
        Row {
            case: "postgres 13, a named zone, public, two tags, both windows named",
            db_name: "db-two",
            region: Some("us-west-2"),
            reported_region: "us-west-2",
            availability_zone: Some("us-west-2b"),
            blueprint_id: "postgres_13",
            bundle_id: "medium_1_0",
            // 36 characters and 60 characters: both outside the MySQL limits
            // (64-character database name, 16-character user name, 41-character
            // password) and inside PostgreSQL's (63, 63, 128), so this row
            // passes only because the limits follow the ENGINE.
            master_database_name: identifier_of(63),
            master_username: identifier_of(60),
            master_user_password: Some(password_of(100)),
            preferred_backup_window: Some("02:00-02:30"),
            preferred_maintenance_window: Some("Sun:05:00-Sun:05:30"),
            publicly_accessible: Some(true),
            tags: &[("stage", Some("prod")), ("owner", Some("platform"))],
        },
        Row {
            case: "mysql on a high-availability plan, a backup window over midnight",
            db_name: "db_three",
            region: Some("eu-west-1"),
            reported_region: "eu-west-1",
            availability_zone: Some("eu-west-1a"),
            blueprint_id: "mysql_8_4",
            bundle_id: "micro_ha_1_0",
            master_database_name: "orders".to_string(),
            master_username: "root_user".to_string(),
            // The shortest password either engine admits.
            master_user_password: Some(password_of(8)),
            preferred_backup_window: Some("23:45-00:15"),
            preferred_maintenance_window: None,
            publicly_accessible: Some(false),
            tags: &[("key-only", None)],
        },
        Row {
            case: "postgres 17 on a high-availability plan, zone picked by the service",
            db_name: "db-four",
            region: Some("ap-northeast-1"),
            reported_region: "ap-northeast-1",
            availability_zone: None,
            blueprint_id: "postgres_17",
            bundle_id: "large_ha_1_0",
            master_database_name: "analytics_2".to_string(),
            master_username: "analyst".to_string(),
            master_user_password: Some(password_of(41)),
            preferred_backup_window: None,
            // A 90-minute window: a window is at least 30 minutes, not exactly
            // 30.
            preferred_maintenance_window: Some("Wed:12:00-Wed:13:30"),
            publicly_accessible: Some(true),
            tags: &[("app", Some("reports"))],
        },
        Row {
            case: "mysql at its own credential limits, in the zone's region not the signed one",
            db_name: "db-five",
            region: Some("eu-central-1"),
            reported_region: "us-east-2",
            availability_zone: Some("us-east-2a"),
            blueprint_id: "mysql_8_4",
            bundle_id: "small_1_0",
            // The upper bound of each MySQL limit: 64, 16 and 41.
            master_database_name: identifier_of(64),
            master_username: identifier_of(16),
            master_user_password: Some(password_of(41)),
            preferred_backup_window: None,
            preferred_maintenance_window: None,
            publicly_accessible: None,
            tags: &[],
        },
    ]
}

/// The `CreateRelationalDatabase` body `row` describes.
fn create_body(row: &Row) -> Value {
    let mut body = json!({
        "relationalDatabaseName": row.db_name,
        "relationalDatabaseBlueprintId": row.blueprint_id,
        "relationalDatabaseBundleId": row.bundle_id,
        "masterDatabaseName": row.master_database_name,
        "masterUsername": row.master_username,
    });
    if let Some(zone) = row.availability_zone {
        body["availabilityZone"] = json!(zone);
    }
    if let Some(password) = &row.master_user_password {
        body["masterUserPassword"] = json!(password);
    }
    if let Some(window) = row.preferred_backup_window {
        body["preferredBackupWindow"] = json!(window);
    }
    if let Some(window) = row.preferred_maintenance_window {
        body["preferredMaintenanceWindow"] = json!(window);
    }
    if let Some(publicly_accessible) = row.publicly_accessible {
        body["publiclyAccessible"] = json!(publicly_accessible);
    }
    if !row.tags.is_empty() {
        body["tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| match value {
                    Some(value) => json!({"key": key, "value": value}),
                    None => json!({"key": key}),
                })
                .collect(),
        );
    }
    body
}

/// The stored database `name`, as `GetRelationalDatabase` reports it.
async fn read_database(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetRelationalDatabase", json!({"relationalDatabaseName": name}))
        .await["relationalDatabase"]
        .clone()
}

/// Poll `GetRelationalDatabase` until it reports `want`, answering the database.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_state(emulator: &Emulator, name: &str, want: &str) -> Value {
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

/// The published blueprint `blueprint_id` names, as
/// `GetRelationalDatabaseBlueprints` reports it.
async fn published_blueprint(emulator: &Emulator, blueprint_id: &str) -> Value {
    let body = emulator
        .ok("GetRelationalDatabaseBlueprints", json!({}))
        .await;
    body["blueprints"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading the blueprint catalog: expected an array, got {body:?}"))
        .iter()
        .find(|blueprint| blueprint["blueprintId"].as_str() == Some(blueprint_id))
        .unwrap_or_else(|| {
            panic!("case reading the blueprint catalog: expected an entry for {blueprint_id}, got {body:?}")
        })
        .clone()
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

/// `hh24:mi` as a minute of the day, or `None` when it is not that form.
fn minute_of_day(time: &str) -> Option<i64> {
    let (hours, minutes) = time.split_once(':')?;
    if hours.len() != 2 || minutes.len() != 2 {
        return None;
    }
    let hours: i64 = hours.parse().ok()?;
    let minutes: i64 = minutes.parse().ok()?;
    (hours <= 23 && minutes <= 59).then_some(hours * 60 + minutes)
}

/// The `[start, end)` minute-of-day ranges a `hh24:mi-hh24:mi` window covers,
/// split where it runs over midnight, or `None` when it is not a well-formed
/// window of at least [`MIN_WINDOW_MINUTES`].
fn daily_ranges(window: &str) -> Option<Vec<(i64, i64)>> {
    let (start, end) = window.split_once('-')?;
    let start = minute_of_day(start)?;
    let end = minute_of_day(end)?;
    let span = (end - start).rem_euclid(MINUTES_PER_DAY);
    if span < MIN_WINDOW_MINUTES {
        return None;
    }
    Some(if start + span <= MINUTES_PER_DAY {
        vec![(start, start + span)]
    } else {
        vec![
            (start, MINUTES_PER_DAY),
            (0, start + span - MINUTES_PER_DAY),
        ]
    })
}

/// The same ranges for a `ddd:hh24:mi-ddd:hh24:mi` weekly window, taken on the
/// daily clock (the day is checked separately), or `None` when it is not a
/// well-formed window of at least [`MIN_WINDOW_MINUTES`] naming published days.
fn weekly_ranges(window: &str) -> Option<Vec<(i64, i64)>> {
    let (start, end) = window.split_once('-')?;
    let point = |value: &str| -> Option<(usize, i64)> {
        let (day, time) = value.split_once(':')?;
        let day = MAINTENANCE_WINDOW_DAYS
            .iter()
            .position(|name| name.eq_ignore_ascii_case(day))?;
        Some((day, minute_of_day(time)?))
    };
    let (start_day, start_minute) = point(start)?;
    let (end_day, end_minute) = point(end)?;
    let span = ((end_day as i64 * MINUTES_PER_DAY + end_minute)
        - (start_day as i64 * MINUTES_PER_DAY + start_minute))
        .rem_euclid(7 * MINUTES_PER_DAY);
    if span < MIN_WINDOW_MINUTES {
        return None;
    }
    Some(if start_minute + span <= MINUTES_PER_DAY {
        vec![(start_minute, start_minute + span)]
    } else {
        vec![
            (start_minute, MINUTES_PER_DAY),
            (0, (start_minute + span) % MINUTES_PER_DAY),
        ]
    })
}

/// Whether the daily backup window and the weekly maintenance window overlap in
/// time of day, which the modelled documentation forbids ("Must not conflict with
/// the preferred maintenance window"). A backup window recurs every day, so the
/// day the maintenance window names cannot keep them apart.
fn windows_conflict(backup: &[(i64, i64)], maintenance: &[(i64, i64)]) -> bool {
    backup.iter().any(|(backup_low, backup_high)| {
        maintenance
            .iter()
            .any(|(low, high)| backup_low < high && low < backup_high)
    })
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: `CreateRelationalDatabase` persists exactly the database the request
/// describes — caller input verbatim, every server-assigned member computed from
/// the request plus this service's published catalogs — and
/// `GetRelationalDatabase` reports that record back.
#[tokio::test]
async fn a_created_database_is_read_back_exactly() {
    for row in rows() {
        let emulator = Emulator::new();
        let Row {
            case,
            db_name,
            region,
            reported_region,
            ..
        } = &row;

        let created = emulator
            .ok_at(None, *region, "CreateRelationalDatabase", create_body(&row))
            .await;

        // The create answers ONE operation-log entry, left non-terminal until
        // the provisioning the clock owes the database is due.
        let operations = created["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {created:?}"));
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {created:?}"
        );
        let operation = &operations[0];
        for (member, want) in [
            ("operationType", "CreateRelationalDatabase"),
            ("resourceType", "RelationalDatabase"),
            ("resourceName", db_name),
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
            "case {case}: the create is still running when it answers, got {operation:?}"
        );
        assert!(
            operation["id"].as_str().is_some_and(is_uuid_shaped),
            "case {case}: expected a uuid-shaped operation id, got {operation:?}"
        );

        // A database is created `creating` and announces no endpoint yet: a
        // caller reading straight after the create sees the transient state.
        let creating = read_database(&emulator, db_name).await;
        assert_eq!(
            creating["state"].as_str(),
            Some("creating"),
            "case {case}: expected the database to be created `creating`, got {creating:?}"
        );
        assert!(
            creating["masterEndpoint"].is_null(),
            "case {case}: a database that is still being created accepts no connection, so it \
             announces no endpoint, got {creating:?}"
        );

        // The clock, not the reader, settles it.
        let database = wait_for_state(&emulator, db_name, "available").await;

        // What the caller sent, verbatim.
        for (member, want) in [
            ("name", db_name.to_string()),
            ("masterDatabaseName", row.master_database_name.clone()),
            ("masterUsername", row.master_username.clone()),
            (
                "relationalDatabaseBlueprintId",
                row.blueprint_id.to_string(),
            ),
            ("relationalDatabaseBundleId", row.bundle_id.to_string()),
        ] {
            assert_eq!(
                database[member].as_str(),
                Some(want.as_str()),
                "case {case}: expected {member} {want}, got {database:?}"
            );
        }
        assert_eq!(
            database["publiclyAccessible"].as_bool(),
            Some(row.publicly_accessible.unwrap_or(false)),
            "case {case}: expected the accessibility the request asked for, defaulting to the \
             account-only one, got {database:?}"
        );
        let want_tags: Vec<Value> = row
            .tags
            .iter()
            .map(|(key, value)| match value {
                Some(value) => json!({"key": key, "value": value}),
                None => json!({"key": key}),
            })
            .collect();
        assert_eq!(
            database["tags"],
            Value::Array(want_tags),
            "case {case}: expected the tags the request carried, got {database:?}"
        );

        // The engine and the version come from the published blueprint, the
        // hardware from the published bundle — never from the request's spelling.
        let blueprint = published_blueprint(&emulator, row.blueprint_id).await;
        for member in ["engine", "engineVersion"] {
            assert_eq!(
                database[member],
                blueprint[member],
                "case {case}: expected {member} to be the published blueprint's, got \
                 {database:?} against {blueprint:?}"
            );
        }
        let bundle = published_bundle(&emulator, row.bundle_id).await;
        for member in ["cpuCount", "diskSizeInGb", "ramSizeInGb"] {
            assert_eq!(
                database["hardware"][member],
                bundle[member],
                "case {case}: expected hardware.{member} to be the published bundle's, got \
                 {database:?} against {bundle:?}"
            );
        }

        // Identity: the ARN carries this service, the database's region and a
        // minted id, and the support code carries the same id under the account.
        let arn = database["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an arn, got {database:?}"));
        let want_prefix = format!("arn:aws:lightsail:{reported_region}:{ACCOUNT}:RelationalDatabase/");
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
        assert_eq!(
            database["resourceType"].as_str(),
            Some("RelationalDatabase"),
            "case {case}: expected the modelled resource type, got {database:?}"
        );
        assert!(
            database["createdAt"].as_f64().is_some_and(|at| at > 0.0),
            "case {case}: expected a creation timestamp, got {database:?}"
        );

        // Location: the zone the request named, else one of the region's own.
        assert_eq!(
            database["location"]["regionName"].as_str(),
            Some(*reported_region),
            "case {case}: expected the region of the zone the request named, else the signed \
             region, got {database:?}"
        );
        let zone = database["location"]["availabilityZone"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an availability zone, got {database:?}"));
        match row.availability_zone {
            Some(want) => assert_eq!(
                zone, want,
                "case {case}: expected the zone the request named, got {database:?}"
            ),
            None => assert!(
                zone.starts_with(reported_region) && zone.len() == reported_region.len() + 1,
                "case {case}: expected a zone of {reported_region}, got {zone}"
            ),
        }

        // A standby is placed only on a high-availability bundle, and never in
        // the primary's own zone.
        match database["secondaryAvailabilityZone"].as_str() {
            Some(secondary) => {
                assert!(
                    row.bundle_id.contains("_ha_"),
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
                !row.bundle_id.contains("_ha_"),
                "case {case}: a high-availability bundle must report its standby zone, got \
                 {database:?}"
            ),
        }

        // The endpoint a client connects to is announced once the database is
        // serving, on the engine's own port.
        let engine = database["engine"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            database["masterEndpoint"]["port"].as_i64(),
            Some(want_port(&engine)),
            "case {case}: expected the {engine} default port, got {database:?}"
        );
        let address = database["masterEndpoint"]["address"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a master endpoint address, got {database:?}"));
        assert!(
            address.ends_with(&format!(".{reported_region}.{ENDPOINT_SUFFIX}")),
            "case {case}: expected the endpoint in {reported_region} under {ENDPOINT_SUFFIX}, got \
             {address}"
        );
        assert!(
            address.starts_with("ls-"),
            "case {case}: expected the endpoint of the Lightsail-managed instance, got {address}"
        );
        // The announced address must never send the next client out of the
        // emulator: no AWS (or Lightsail) hostname, ever.
        assert!(
            !address.contains("amazonaws.com") && !address.contains("amazonlightsail.com"),
            "case {case}: the announced address must not name a host off this emulator, got \
             {address}"
        );

        // The windows: named verbatim, or defaulted to a well-formed window of
        // at least 30 minutes that does not conflict with the other one.
        let backup = database["preferredBackupWindow"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a backup window, got {database:?}"));
        let maintenance = database["preferredMaintenanceWindow"].as_str().unwrap_or_else(|| {
            panic!("case {case}: expected a maintenance window, got {database:?}")
        });
        if let Some(want) = row.preferred_backup_window {
            assert_eq!(
                backup, want,
                "case {case}: expected the backup window the request named, got {database:?}"
            );
        }
        if let Some(want) = row.preferred_maintenance_window {
            assert_eq!(
                maintenance, want,
                "case {case}: expected the maintenance window the request named, got {database:?}"
            );
        }
        let backup_ranges = daily_ranges(backup).unwrap_or_else(|| {
            panic!("case {case}: expected a hh24:mi-hh24:mi window of at least 30 minutes, got {backup}")
        });
        let maintenance_ranges = weekly_ranges(maintenance).unwrap_or_else(|| {
            panic!(
                "case {case}: expected a ddd:hh24:mi-ddd:hh24:mi window of at least 30 minutes, \
                 got {maintenance}"
            )
        });
        assert!(
            !windows_conflict(&backup_ranges, &maintenance_ranges),
            "case {case}: the backup window {backup} and the maintenance window {maintenance} \
             may not conflict"
        );

        // The rest of what the service assigns a new database.
        assert_eq!(
            database["backupRetentionEnabled"].as_bool(),
            Some(true),
            "case {case}: a new database retains automated backups until a caller disables it, \
             got {database:?}"
        );
        assert_eq!(
            database["parameterApplyStatus"].as_str(),
            Some("in-sync"),
            "case {case}: nothing is mid-update on a new database, got {database:?}"
        );
        assert!(
            database["caCertificateIdentifier"]
                .as_str()
                .is_some_and(|identifier| !identifier.is_empty()),
            "case {case}: expected the certificate authority of the database, got {database:?}"
        );
        assert_eq!(
            database["pendingMaintenanceActions"],
            json!([]),
            "case {case}: no maintenance action is pending on a new database, got {database:?}"
        );
        assert!(
            database["pendingModifiedValues"].is_null(),
            "case {case}: nothing is pending on a new database, got {database:?}"
        );
        assert!(
            database["latestRestorableTime"].is_null(),
            "case {case}: no backup has been taken yet, so there is no restorable point, got \
             {database:?}"
        );

        // A read changes nothing a later reader sees.
        let again = read_database(&emulator, db_name).await;
        assert_eq!(
            again, database,
            "case {case}: two reads of one database must answer the same record, got {again:?}"
        );
    }
}

/// RULE: the master credential limits are the ENGINE's, so one request is
/// accepted on the engine whose limits admit it and refused on the engine whose
/// limits do not — and a refused create stores nothing.
#[tokio::test]
async fn master_credentials_are_held_to_the_engines_own_limits() {
    /// One credential case: the member under test, the value, and the engines
    /// that accept it. The limits are the modelled
    /// `CreateRelationalDatabase` documentation's own: a MySQL master database
    /// name of 1 to 64 characters, user name of 1 to 16 and password of 8 to 41;
    /// a PostgreSQL database name of 1 to 63, user name of 1 to 63 and password
    /// of 8 to 128.
    struct Case {
        case: &'static str,
        member: &'static str,
        value: String,
        accepted_by_mysql: bool,
        accepted_by_postgres: bool,
    }

    let cases = vec![
        Case {
            case: "a 16-character user name",
            member: "masterUsername",
            value: identifier_of(16),
            accepted_by_mysql: true,
            accepted_by_postgres: true,
        },
        Case {
            case: "a 17-character user name",
            member: "masterUsername",
            value: identifier_of(17),
            accepted_by_mysql: false,
            accepted_by_postgres: true,
        },
        Case {
            case: "a 63-character user name",
            member: "masterUsername",
            value: identifier_of(63),
            accepted_by_mysql: false,
            accepted_by_postgres: true,
        },
        Case {
            case: "a 64-character user name",
            member: "masterUsername",
            value: identifier_of(64),
            accepted_by_mysql: false,
            accepted_by_postgres: false,
        },
        Case {
            case: "a 63-character database name",
            member: "masterDatabaseName",
            value: identifier_of(63),
            accepted_by_mysql: true,
            accepted_by_postgres: true,
        },
        Case {
            case: "a 64-character database name",
            member: "masterDatabaseName",
            value: identifier_of(64),
            accepted_by_mysql: true,
            accepted_by_postgres: false,
        },
        Case {
            case: "a 65-character database name",
            member: "masterDatabaseName",
            value: identifier_of(65),
            accepted_by_mysql: false,
            accepted_by_postgres: false,
        },
        Case {
            case: "a database name that does not begin with a letter",
            member: "masterDatabaseName",
            value: "9lives".to_string(),
            accepted_by_mysql: false,
            accepted_by_postgres: false,
        },
        Case {
            case: "a user name carrying a hyphen",
            member: "masterUsername",
            value: "db-admin".to_string(),
            accepted_by_mysql: false,
            accepted_by_postgres: false,
        },
        Case {
            case: "a user name carrying an underscore and digits",
            member: "masterUsername",
            value: "db_admin_9".to_string(),
            accepted_by_mysql: true,
            accepted_by_postgres: true,
        },
        Case {
            case: "a 7-character password",
            member: "masterUserPassword",
            value: password_of(7),
            accepted_by_mysql: false,
            accepted_by_postgres: false,
        },
        Case {
            case: "an 8-character password",
            member: "masterUserPassword",
            value: password_of(8),
            accepted_by_mysql: true,
            accepted_by_postgres: true,
        },
        Case {
            case: "a 41-character password",
            member: "masterUserPassword",
            value: password_of(41),
            accepted_by_mysql: true,
            accepted_by_postgres: true,
        },
        Case {
            case: "a 42-character password",
            member: "masterUserPassword",
            value: password_of(42),
            accepted_by_mysql: false,
            accepted_by_postgres: true,
        },
        Case {
            case: "a 128-character password",
            member: "masterUserPassword",
            value: password_of(128),
            accepted_by_mysql: false,
            accepted_by_postgres: true,
        },
        Case {
            case: "a 129-character password",
            member: "masterUserPassword",
            value: password_of(129),
            accepted_by_mysql: false,
            accepted_by_postgres: false,
        },
        Case {
            case: "a password carrying the forbidden @",
            member: "masterUserPassword",
            value: "Pass@word1".to_string(),
            accepted_by_mysql: false,
            accepted_by_postgres: false,
        },
        Case {
            case: "a password carrying a space",
            member: "masterUserPassword",
            value: "Pass word1".to_string(),
            accepted_by_mysql: false,
            accepted_by_postgres: false,
        },
    ];

    for case in cases {
        for (blueprint_id, accepted) in [
            ("mysql_8_4", case.accepted_by_mysql),
            ("postgres_17", case.accepted_by_postgres),
        ] {
            let emulator = Emulator::new();
            let name = "credentials-db";
            let mut body = json!({
                "relationalDatabaseName": name,
                "relationalDatabaseBlueprintId": blueprint_id,
                "relationalDatabaseBundleId": "micro_1_0",
                "masterDatabaseName": "appdb",
                "masterUsername": "dbadmin",
                "masterUserPassword": password_of(16),
            });
            body[case.member] = json!(case.value);

            let (status, response) = emulator.call("CreateRelationalDatabase", body).await;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {} on {blueprint_id}: expected it to be accepted, got {status} \
                     {response:?}",
                    case.case
                );
                let database = read_database(&emulator, name).await;
                assert_eq!(
                    database[case.member].as_str().map(str::to_string),
                    (case.member != "masterUserPassword").then(|| case.value.clone()),
                    "case {} on {blueprint_id}: expected the stored value to be what the \
                     request carried (and no member to report the password), got {database:?}",
                    case.case
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {} on {blueprint_id}: expected a 400 refusal, got {status} {response:?}",
                case.case
            );
            assert_eq!(
                error_code(&response),
                "InvalidInputException",
                "case {} on {blueprint_id}: expected the modelled InvalidInputException, got \
                 {response:?}",
                case.case
            );
            assert!(
                response["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(case.member),
                "case {} on {blueprint_id}: expected the refusal to name the member, got \
                 {response:?}",
                case.case
            );
            let (read_status, read_body) = emulator
                .call(
                    "GetRelationalDatabase",
                    json!({"relationalDatabaseName": name}),
                )
                .await;
            assert_eq!(
                read_status, 400,
                "case {} on {blueprint_id}: a refused create stores nothing, so the read must \
                 be refused too, got {read_status} {read_body:?}",
                case.case
            );
            assert_eq!(
                read_body["code"].as_str(),
                Some("DoesNotExist"),
                "case {} on {blueprint_id}: a refused create stores nothing, got {read_body:?}",
                case.case
            );
        }
    }
}

/// RULE: a request the model admits but the service does not create is refused
/// with the modelled `InvalidInputException` naming the member that is wrong, and
/// stores nothing.
#[tokio::test]
async fn a_request_outside_what_the_service_creates_is_refused() {
    /// One refusal case: the members to overwrite on a well-formed request, and
    /// the member the refusal must name.
    struct Case {
        case: &'static str,
        patch: Vec<(&'static str, Value)>,
        member: &'static str,
    }

    let cases = vec![
        Case {
            case: "a blueprint the catalog does not publish",
            patch: vec![(
                "relationalDatabaseBlueprintId",
                json!("mysql_5_6"),
            )],
            member: "relationalDatabaseBlueprintId",
        },
        Case {
            case: "a bundle the catalog does not publish",
            patch: vec![("relationalDatabaseBundleId", json!("nano_1_0"))],
            member: "relationalDatabaseBundleId",
        },
        Case {
            case: "a zone no Lightsail region has",
            patch: vec![("availabilityZone", json!("mars-central-1a"))],
            member: "availabilityZone",
        },
        Case {
            case: "a zone letter the region does not publish",
            patch: vec![("availabilityZone", json!("us-east-1z"))],
            member: "availabilityZone",
        },
        Case {
            case: "a backup window that is not hh24:mi-hh24:mi",
            patch: vec![("preferredBackupWindow", json!("16:00 to 16:30"))],
            member: "preferredBackupWindow",
        },
        Case {
            case: "a backup window naming an impossible time",
            patch: vec![("preferredBackupWindow", json!("24:00-24:30"))],
            member: "preferredBackupWindow",
        },
        Case {
            // `hh24:mi` is two digits and a colon: a signed component is not
            // that form, whatever the number it would read as.
            case: "a backup window whose hour carries a sign",
            patch: vec![("preferredBackupWindow", json!("-1:00-01:30"))],
            member: "preferredBackupWindow",
        },
        Case {
            case: "a backup window whose minute carries a sign",
            patch: vec![("preferredBackupWindow", json!("01:-5-02:00"))],
            member: "preferredBackupWindow",
        },
        Case {
            case: "a maintenance window whose hour carries a sign",
            patch: vec![(
                "preferredMaintenanceWindow",
                json!("Tue:-3:00-Tue:04:00"),
            )],
            member: "preferredMaintenanceWindow",
        },
        Case {
            case: "a backup window shorter than 30 minutes",
            patch: vec![("preferredBackupWindow", json!("16:00-16:15"))],
            member: "preferredBackupWindow",
        },
        Case {
            case: "a maintenance window naming no day",
            patch: vec![("preferredMaintenanceWindow", json!("17:00-17:30"))],
            member: "preferredMaintenanceWindow",
        },
        Case {
            case: "a maintenance window naming a day that does not exist",
            patch: vec![(
                "preferredMaintenanceWindow",
                json!("Fri2:17:00-Fri2:17:30"),
            )],
            member: "preferredMaintenanceWindow",
        },
        Case {
            case: "a maintenance window shorter than 30 minutes",
            patch: vec![(
                "preferredMaintenanceWindow",
                json!("Tue:17:00-Tue:17:20"),
            )],
            member: "preferredMaintenanceWindow",
        },
        Case {
            case: "two windows that conflict",
            patch: vec![
                ("preferredBackupWindow", json!("17:00-17:30")),
                (
                    "preferredMaintenanceWindow",
                    json!("Tue:17:15-Tue:17:45"),
                ),
            ],
            member: "preferredMaintenanceWindow",
        },
        Case {
            case: "two windows that conflict across midnight",
            patch: vec![
                ("preferredBackupWindow", json!("23:45-00:15")),
                (
                    "preferredMaintenanceWindow",
                    json!("Sat:00:00-Sat:00:30"),
                ),
            ],
            member: "preferredMaintenanceWindow",
        },
        Case {
            case: "more tags than the service accepts",
            patch: vec![(
                "tags",
                Value::Array(
                    (0..51)
                        .map(|index| json!({"key": format!("key-{index}")}))
                        .collect(),
                ),
            )],
            member: "tags",
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        let name = "refused-db";
        let mut body = json!({
            "relationalDatabaseName": name,
            "relationalDatabaseBlueprintId": "mysql_8_4",
            "relationalDatabaseBundleId": "micro_1_0",
            "masterDatabaseName": "appdb",
            "masterUsername": "dbadmin",
            "masterUserPassword": password_of(16),
        });
        for (member, value) in &case.patch {
            body[*member] = value.clone();
        }

        let (status, response) = emulator.call("CreateRelationalDatabase", body).await;
        assert_eq!(
            status, 400,
            "case {}: expected a 400 refusal, got {status} {response:?}",
            case.case
        );
        assert_eq!(
            error_code(&response),
            "InvalidInputException",
            "case {}: expected the modelled InvalidInputException, got {response:?}",
            case.case
        );
        assert!(
            response["message"]
                .as_str()
                .unwrap_or_default()
                .contains(case.member),
            "case {}: expected the refusal to name {}, got {response:?}",
            case.case,
            case.member
        );

        let (read_status, read_body) = emulator
            .call(
                "GetRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        assert_eq!(
            read_status, 400,
            "case {}: a refused create stores nothing, so the read must be refused too, got \
             {read_status} {read_body:?}",
            case.case
        );
        assert_eq!(
            read_body["code"].as_str(),
            Some("DoesNotExist"),
            "case {}: a refused create stores nothing, got {read_body:?}",
            case.case
        );
    }
}

/// RULE: a database name addresses ONE database, so creating one the account
/// already holds is refused and leaves the stored record untouched.
#[tokio::test]
async fn creating_a_database_that_exists_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let name = "existing-db";
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": name,
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": "micro_1_0",
                "masterDatabaseName": "appdb",
                "masterUsername": "dbadmin",
                "masterUserPassword": password_of(16),
            }),
        )
        .await;
    let before = wait_for_state(&emulator, name, "available").await;

    let (status, response) = emulator
        .call(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": name,
                "relationalDatabaseBlueprintId": "postgres_17",
                "relationalDatabaseBundleId": "large_1_0",
                "masterDatabaseName": "otherdb",
                "masterUsername": "otheradmin",
                "masterUserPassword": password_of(20),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case creating an existing database: expected a 400 refusal, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "InvalidInputException",
        "case creating an existing database: expected the modelled InvalidInputException, got \
         {response:?}"
    );

    let after = read_database(&emulator, name).await;
    assert_eq!(
        after, before,
        "case creating an existing database: expected the stored database to be untouched, got \
         {after:?}"
    );
}

/// RULE: a well-formed name that names no stored database is refused with the
/// service's `DoesNotExist` envelope — HTTP 400, `NotFoundException`, body code
/// `DoesNotExist` and a message naming the resource type and the requested name —
/// never a 501 and never a hollow 200.
#[tokio::test]
async fn an_unknown_relational_database_does_not_exist() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "present-db",
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": "micro_1_0",
                "masterDatabaseName": "appdb",
                "masterUsername": "dbadmin",
                "masterUserPassword": password_of(16),
            }),
        )
        .await;

    for name in ["absent-db", "present-db-2", "other_db"] {
        let (status, body) = emulator
            .call(
                "GetRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case reading {name}: expected a 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case reading {name}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case reading {name}: expected code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The RelationalDatabase does not exist: {name}").as_str()),
            "case reading {name}: expected the resource-type/name message, got {body:?}"
        );
    }
}

/// RULE: a defaulted window is DERIVED from the database's own name, so two
/// databases of different names get different defaults while one database
/// reports the same window on every read — and the defaults never conflict with
/// each other.
#[tokio::test]
async fn defaulted_windows_are_derived_from_the_database_name() {
    let emulator = Emulator::new();
    let names = ["window-one", "window-two", "window_three", "window-four"];
    let mut backups = Vec::new();
    for name in names {
        emulator
            .ok(
                "CreateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "relationalDatabaseBlueprintId": "postgres_16",
                    "relationalDatabaseBundleId": "small_1_0",
                    "masterDatabaseName": "appdb",
                    "masterUsername": "dbadmin",
                    "masterUserPassword": password_of(16),
                }),
            )
            .await;
        let database = wait_for_state(&emulator, name, "available").await;
        let backup = database["preferredBackupWindow"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name}: expected a backup window, got {database:?}"))
            .to_string();
        let maintenance = database["preferredMaintenanceWindow"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name}: expected a maintenance window, got {database:?}"))
            .to_string();
        let backup_ranges = daily_ranges(&backup)
            .unwrap_or_else(|| panic!("case {name}: expected a well-formed window, got {backup}"));
        let maintenance_ranges = weekly_ranges(&maintenance).unwrap_or_else(|| {
            panic!("case {name}: expected a well-formed window, got {maintenance}")
        });
        assert!(
            !windows_conflict(&backup_ranges, &maintenance_ranges),
            "case {name}: the defaulted windows {backup} and {maintenance} may not conflict"
        );

        let again = read_database(&emulator, name).await;
        assert_eq!(
            again["preferredBackupWindow"].as_str(),
            Some(backup.as_str()),
            "case {name}: a defaulted window is stable across reads, got {again:?}"
        );
        assert_eq!(
            again["preferredMaintenanceWindow"].as_str(),
            Some(maintenance.as_str()),
            "case {name}: a defaulted window is stable across reads, got {again:?}"
        );
        backups.push(backup);
    }
    assert!(
        backups.iter().collect::<std::collections::BTreeSet<_>>().len() > 1,
        "case the defaults over several names: expected the window to follow the name, got \
         {backups:?}"
    );
}

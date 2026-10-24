// SPDX-License-Identifier: CC0-1.0

//! `UpdateRelationalDatabase` — changing the attributes of one managed
//! relational database, driven over the wire.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.UpdateRelationalDatabase` and an
//! `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed.
//!
//! The rules under test, asserted over a SET of databases — both engines this
//! service publishes blueprints for, several engine versions, standard and
//! high-availability bundles of different sizes, in four Regions — rather than
//! over one call:
//!
//! * an attribute no `pendingModifiedValues` member can report — the two time
//!   windows, the public accessibility and the certificate authority — is
//!   written into the ONE persisted record, so the next `GetRelationalDatabase`
//!   reports it and every other member of the record is untouched;
//! * the three attributes the modelled
//!   `PendingModifiedRelationalDatabaseValues` CAN report — the master user
//!   password, the engine version a major-version blueprint update moves, and
//!   automated backup retention — are applied at once only when the request asks
//!   for that, and otherwise wait for the database's maintenance window: the
//!   record still reports the old values, `pendingModifiedValues` reports what is
//!   waiting (a password MASKED, never the secret itself), and the clock applies
//!   them without anybody calling again;
//! * asking for the value a database already carries changes nothing;
//! * the call logs exactly one `UpdateRelationalDatabase` operation against that
//!   database, terminal when every change is already in the record and
//!   non-terminal while one is waiting; and
//! * a request the service cannot serve — a name the account holds no database
//!   for, two members asking for opposite changes, a certificate authority or a
//!   blueprint the service does not publish, a blueprint of another engine or an
//!   earlier major version, a malformed or conflicting window, a password outside
//!   the engine's own limits — is refused with the modelled envelope and leaves
//!   the record exactly as it was.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::STATE_TRANSITION_MILLIS;

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// How long to wait for a clock-owed change before giving up.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// The certificate authority a database is created under, from the service's own
/// default.
const DEFAULT_CA_CERTIFICATE: &str = "rds-ca-rsa2048-g1";

/// The span the clock gives a change it owes, from the service's own constant.
///
/// A case that asserts the WAITING modification has to know whether its call
/// landed inside that span: measuring it is what keeps such a case exact on a
/// machine slow enough for the change to arrive first.
fn transition_window() -> Duration {
    Duration::from_millis(u64::try_from(STATE_TRANSITION_MILLIS).unwrap_or_default())
}

/// One database to update, and the attributes the cases set on it.
///
/// The rows differ along what the rules generalise over: the engine, the engine
/// version, the bundle's size and whether it is a high-availability plan (so the
/// record has a standby zone to preserve), the Region and Availability Zone the
/// database lives in, whether the catalog offers a later major version of that
/// engine to update to, which published certificate authority is named, the form
/// of each time window (one of them crossing midnight, one spanning an hour) and
/// the length of the master user password (the shortest the engine admits, the
/// longest, and one in between).
#[derive(Debug)]
struct Row {
    case: &'static str,
    blueprint_id: &'static str,
    bundle_id: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    /// A LATER major version of the SAME engine from the published catalog, when
    /// it offers one for this row's engine.
    upgrade_blueprint_id: Option<&'static str>,
    /// A blueprint of the OTHER engine the service publishes, which no update of
    /// this database may move it to.
    other_engine_blueprint_id: &'static str,
    /// A certificate authority the service publishes, from
    /// `RELATIONAL_DATABASE_CA_CERTIFICATE_IDENTIFIERS` — the identifiers "Using
    /// SSL/TLS to encrypt a connection to a DB instance" (Amazon RDS User Guide)
    /// lists for a DB instance.
    ca_certificate_identifier: &'static str,
    /// A daily backup window in the modelled `hh24:mi-hh24:mi` form.
    backup_window: &'static str,
    /// A weekly maintenance window in the modelled `ddd:hh24:mi-ddd:hh24:mi`
    /// form, clear of `backup_window`.
    maintenance_window: &'static str,
    /// How many characters the master user password these cases set carries —
    /// inside the range the row's engine admits.
    password_chars: usize,
}

/// The databases every case here updates.
const ROWS: &[Row] = &[
    Row {
        case: "mysql 8.4, smallest standard bundle, the shortest password the engine admits",
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_1_0",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        // The catalog publishes one MySQL major version, so there is no later
        // one for this row to update to.
        upgrade_blueprint_id: None,
        other_engine_blueprint_id: "postgres_17",
        ca_certificate_identifier: "rds-ca-rsa4096-g1",
        backup_window: "01:00-01:30",
        maintenance_window: "Tue:17:00-Tue:17:30",
        password_chars: 8,
    },
    Row {
        case: "postgres 13, large standard bundle, a backup window over midnight, a password \
               longer than MySQL would accept",
        blueprint_id: "postgres_13",
        bundle_id: "large_1_0",
        availability_zone: "us-west-2b",
        region: "us-west-2",
        upgrade_blueprint_id: Some("postgres_17"),
        other_engine_blueprint_id: "mysql_8_4",
        ca_certificate_identifier: "rds-ca-ecc384-g1",
        backup_window: "23:45-00:30",
        maintenance_window: "Sun:03:00-Sun:04:00",
        password_chars: 64,
    },
    Row {
        case: "postgres 17, micro high-availability bundle, so the record carries a standby zone",
        blueprint_id: "postgres_17",
        bundle_id: "micro_ha_1_0",
        availability_zone: "eu-west-3a",
        region: "eu-west-3",
        upgrade_blueprint_id: Some("postgres_18"),
        other_engine_blueprint_id: "mysql_8_4",
        ca_certificate_identifier: "rds-ca-2019",
        backup_window: "12:00-12:30",
        maintenance_window: "Thu:22:30-Thu:23:30",
        password_chars: 128,
    },
    Row {
        case: "mysql 8.4, medium high-availability bundle, the certificate authority it already \
               carries",
        blueprint_id: "mysql_8_4",
        bundle_id: "medium_ha_1_0",
        availability_zone: "ap-northeast-1c",
        region: "ap-northeast-1",
        upgrade_blueprint_id: None,
        other_engine_blueprint_id: "postgres_13",
        ca_certificate_identifier: DEFAULT_CA_CERTIFICATE,
        backup_window: "04:00-05:00",
        maintenance_window: "Mon:00:00-Mon:00:30",
        password_chars: 41,
    },
];

/// Names the account holds no database for, differing along the length, the case,
/// the digits and the separators. Every one satisfies the modelled `ResourceName`
/// pattern `^\w[\w\-]*\w$`, so the centralized walk passes it through to the
/// handler.
const MISSING_NAMES: &[&str] = &[
    "ab",
    "never-created",
    "Probe_7",
    "a-name-with-many-hyphen-separated-words-and-digits-0123456789",
];

/// A master user password of `chars` characters.
///
/// Built from printable ASCII other than a space, `/`, `"` and `@` — the
/// characters the modelled `masterUserPassword` documentation admits — so the
/// only thing a case varies is the LENGTH.
fn password_of(chars: usize) -> String {
    const ALPHABET: &[u8] = b"Pw9#x!Zq";
    (0..chars)
        .map(|index| char::from(ALPHABET[index % ALPHABET.len()]))
        .collect()
}

/// The `CreateRelationalDatabase` body for `row`, naming one database.
fn create_body(row: &Row, name: &str) -> Value {
    json!({
        "relationalDatabaseName": name,
        "relationalDatabaseBlueprintId": row.blueprint_id,
        "relationalDatabaseBundleId": row.bundle_id,
        "availabilityZone": row.availability_zone,
        "masterDatabaseName": "appdb",
        "masterUsername": "dbadmin",
        "masterUserPassword": "Pw9#x!ZqPw9#x!Zq",
    })
}

/// Create the database `name` for `row` and answer the emulator it lives in, once
/// the clock has settled it into `available`.
async fn with_available_database(row: &Row, name: &str) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok("CreateRelationalDatabase", create_body(row, name))
        .await;
    wait_for_state(&emulator, name, "available").await;
    emulator
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

/// The stored database `name`, once the clock has settled it into `state`.
async fn wait_for_state(emulator: &Emulator, name: &str, state: &str) -> Value {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let read = database(emulator, name).await;
        if read["state"].as_str() == Some(state) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {name}: the clock never settled the database into {state}, last read {read:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The stored database `name`, once the clock has applied every modification
/// that was waiting for its maintenance window.
async fn wait_for_no_pending(case: &str, emulator: &Emulator, name: &str) -> Value {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let read = database(emulator, name).await;
        if read["pendingModifiedValues"].is_null() {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the clock never applied the waiting modification, last read {read:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The engine version the published catalog serves the blueprint `blueprint_id`
/// at, read through `GetRelationalDatabaseBlueprints` rather than written here,
/// so a case about a version update names no version of its own.
async fn catalog_engine_version(emulator: &Emulator, blueprint_id: &str) -> String {
    let body = emulator
        .ok("GetRelationalDatabaseBlueprints", json!({}))
        .await;
    let blueprints = body["blueprints"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a blueprints array, got {body:?}"))
        .clone();
    blueprints
        .iter()
        .find(|blueprint| blueprint["blueprintId"].as_str() == Some(blueprint_id))
        .and_then(|blueprint| blueprint["engineVersion"].as_str())
        .unwrap_or_else(|| {
            panic!("expected the catalog to publish {blueprint_id}, got {blueprints:?}")
        })
        .to_string()
}

/// `record` without the members `members` names, so two reads can be compared on
/// everything else.
fn without(record: &Value, members: &[&str]) -> Value {
    let mut copy = record.clone();
    if let Some(object) = copy.as_object_mut() {
        for member in members {
            object.remove(*member);
        }
    }
    copy
}

/// Assert `operations` is exactly one `UpdateRelationalDatabase` log entry
/// against `name`, terminal exactly when `terminal`, answering its id.
fn assert_update_operation(
    case: &str,
    operations: &Value,
    name: &str,
    region: &str,
    terminal: bool,
) -> String {
    let entries = operations
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations list, got {operations:?}"));
    assert_eq!(
        entries.len(),
        1,
        "case {case}: an update acts on one database, so it logs one operation, got {operations:?}"
    );
    let operation = &entries[0];
    assert_eq!(
        operation["operationType"].as_str(),
        Some("UpdateRelationalDatabase"),
        "case {case}: expected operationType UpdateRelationalDatabase, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(name),
        "case {case}: expected resourceName {name}, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some("RelationalDatabase"),
        "case {case}: expected resourceType RelationalDatabase, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some(region),
        "case {case}: the entry is located where the database is, got {operation:?}"
    );
    let expected = if terminal {
        (Some("Succeeded"), Some(true))
    } else {
        (Some("Started"), Some(false))
    };
    assert_eq!(
        (
            operation["status"].as_str(),
            operation["isTerminal"].as_bool()
        ),
        expected,
        "case {case}: expected a {} entry, got {operation:?}",
        if terminal { "terminal" } else { "non-terminal" }
    );
    let id = operation["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a minted operation id, got {operation:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    id.to_string()
}

// ---------------------------------------------------------------------------
// The attributes that are written straight away
// ---------------------------------------------------------------------------

/// RULE: an attribute the modelled `pendingModifiedValues` cannot report — the
/// daily backup window, the weekly maintenance window, the public accessibility
/// and the certificate authority — is written into the one persisted record when
/// the call is answered, so the next read reports it and every other member of
/// the record is left exactly as it was.
#[tokio::test]
async fn an_attribute_that_cannot_be_reported_pending_is_written_straight_away() {
    for row in ROWS {
        let name = "updated-db";
        let emulator = with_available_database(row, name).await;
        let before = database(&emulator, name).await;
        assert_eq!(
            before["caCertificateIdentifier"].as_str(),
            Some(DEFAULT_CA_CERTIFICATE),
            "case {}: a new database carries the service's default certificate authority, \
             got {before:?}",
            row.case
        );

        let flipped = !before["publiclyAccessible"].as_bool().unwrap_or(false);
        let updated = emulator
            .ok(
                "UpdateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "preferredBackupWindow": row.backup_window,
                    "preferredMaintenanceWindow": row.maintenance_window,
                    "publiclyAccessible": flipped,
                    "caCertificateIdentifier": row.ca_certificate_identifier,
                }),
            )
            .await;
        // Nothing in this call waits for a maintenance window, so the entry it
        // logs is terminal.
        let operation_id =
            assert_update_operation(row.case, &updated["operations"], name, row.region, true);

        let after = database(&emulator, name).await;
        for (member, expected) in [
            ("preferredBackupWindow", json!(row.backup_window)),
            ("preferredMaintenanceWindow", json!(row.maintenance_window)),
            ("publiclyAccessible", json!(flipped)),
            (
                "caCertificateIdentifier",
                json!(row.ca_certificate_identifier),
            ),
        ] {
            assert_eq!(
                after[member], expected,
                "case {}: the update must persist {member}, expected {expected:?}, got {:?}",
                row.case, after[member]
            );
        }
        // Nothing is waiting, so nothing is reported as pending.
        assert!(
            after["pendingModifiedValues"].is_null(),
            "case {}: an update that applies everything at once leaves nothing pending, \
             got {after:?}",
            row.case
        );
        // Nothing else moved: the attributes this call did not name read back
        // identical, the master endpoint a client connects to above all.
        let touched = [
            "preferredBackupWindow",
            "preferredMaintenanceWindow",
            "publiclyAccessible",
            "caCertificateIdentifier",
        ];
        assert_eq!(
            without(&after, &touched),
            without(&before, &touched),
            "case {}: the update must leave every other member of the record untouched, \
             got {after:?}",
            row.case
        );
        for member in [
            "arn",
            "supportCode",
            "createdAt",
            "masterEndpoint",
            "masterUsername",
            "masterDatabaseName",
            "hardware",
            "engine",
            "engineVersion",
            "relationalDatabaseBlueprintId",
            "relationalDatabaseBundleId",
            "secondaryAvailabilityZone",
            "backupRetentionEnabled",
            "state",
            "location",
        ] {
            assert_eq!(
                after[member], before[member],
                "case {}: the update must preserve {member}, expected {:?}, got {:?}",
                row.case, before[member], after[member]
            );
        }

        // The entry the update logged is readable by its id, and it is the SAME
        // record the update answered with: one log, not two.
        let read_back = emulator
            .ok("GetOperation", json!({"operationId": operation_id}))
            .await;
        for member in [
            "id",
            "operationType",
            "resourceName",
            "resourceType",
            "status",
            "isTerminal",
            "createdAt",
            "location",
        ] {
            assert_eq!(
                read_back["operation"][member], updated["operations"][0][member],
                "case {}: GetOperation must report the entry the update logged, expected \
                 {member} {:?}, got {:?}",
                row.case, updated["operations"][0][member], read_back["operation"][member]
            );
        }

        // A second update changes only what it names: the windows stay as the
        // first call set them while the accessibility goes back.
        let again = emulator
            .ok(
                "UpdateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "publiclyAccessible": !flipped,
                }),
            )
            .await;
        let second_id =
            assert_update_operation(row.case, &again["operations"], name, row.region, true);
        assert_ne!(
            operation_id, second_id,
            "case {}: each update logs its own operation, got {operation_id} twice",
            row.case
        );
        let twice = database(&emulator, name).await;
        assert_eq!(
            twice["publiclyAccessible"].as_bool(),
            Some(!flipped),
            "case {}: the second update must persist publiclyAccessible, got {twice:?}",
            row.case
        );
        assert_eq!(
            without(&twice, &["publiclyAccessible"]),
            without(&after, &["publiclyAccessible"]),
            "case {}: the second update must change nothing it did not name, got {twice:?}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// The three attributes that can be left waiting
// ---------------------------------------------------------------------------

/// RULE: an attribute the modelled `PendingModifiedRelationalDatabaseValues`
/// carries — the master user password, the engine version a major-version
/// blueprint update moves, and automated backup retention — waits for the
/// database's maintenance window unless the request asks for it to be applied
/// immediately: the record still reports the value it had, `pendingModifiedValues`
/// reports what is waiting (a password masked, never the secret), the logged
/// operation is non-terminal, and the clock applies the change without anybody
/// calling again.
#[tokio::test]
async fn a_deferrable_attribute_waits_for_the_maintenance_window_and_the_clock_applies_it() {
    for row in ROWS {
        let name = "deferred-db";
        let emulator = with_available_database(row, name).await;
        let before = database(&emulator, name).await;
        let upgrade = row.upgrade_blueprint_id;
        let target_version = match upgrade {
            Some(blueprint_id) => Some(catalog_engine_version(&emulator, blueprint_id).await),
            None => None,
        };

        let mut body = json!({
            "relationalDatabaseName": name,
            "masterUserPassword": password_of(row.password_chars),
            "disableBackupRetention": true,
        });
        if let Some(blueprint_id) = upgrade {
            body["relationalDatabaseBlueprintId"] = json!(blueprint_id);
        }
        let started = Instant::now();
        let updated = emulator.ok("UpdateRelationalDatabase", body).await;
        let during = database(&emulator, name).await;

        // Asserted only when the read really did land before the clock applied
        // the modification.
        if started.elapsed() < transition_window() {
            assert_update_operation(row.case, &updated["operations"], name, row.region, false);
            let pending = &during["pendingModifiedValues"];
            // The password is a secret: a database read may say only THAT a new
            // one is waiting.
            assert_eq!(
                pending["masterUserPassword"].as_str(),
                Some("****"),
                "case {}: a waiting password is reported masked, got {during:?}",
                row.case
            );
            assert_eq!(
                pending["backupRetentionEnabled"].as_bool(),
                Some(false),
                "case {}: the waiting backup retention setting is what is reported, \
                 got {during:?}",
                row.case
            );
            assert_eq!(
                pending["engineVersion"].as_str(),
                target_version.as_deref(),
                "case {}: the waiting engine version is the one the target blueprint is \
                 served at, got {during:?}",
                row.case
            );
            // The record itself still reports what the database carries NOW.
            assert_eq!(
                during["backupRetentionEnabled"].as_bool(),
                Some(true),
                "case {}: a waiting modification must not be written into the record yet, \
                 got {during:?}",
                row.case
            );
            assert_eq!(
                without(&during, &["pendingModifiedValues"]),
                without(&before, &["pendingModifiedValues"]),
                "case {}: while a modification waits the record is the one the create \
                 stored, got {during:?}",
                row.case
            );
        }

        // The clock applies it: nobody called again.
        let after = wait_for_no_pending(row.case, &emulator, name).await;
        assert_eq!(
            after["backupRetentionEnabled"].as_bool(),
            Some(false),
            "case {}: the clock must apply the waiting backup retention change, got {after:?}",
            row.case
        );
        match (upgrade, target_version.as_deref()) {
            (Some(blueprint_id), Some(version)) => {
                assert_eq!(
                    (
                        after["relationalDatabaseBlueprintId"].as_str(),
                        after["engineVersion"].as_str()
                    ),
                    (Some(blueprint_id), Some(version)),
                    "case {}: the clock must move the database to the blueprint the update \
                     named, got {after:?}",
                    row.case
                );
                assert_eq!(
                    after["engine"], before["engine"],
                    "case {}: a blueprint update moves the major version, never the engine, \
                     got {after:?}",
                    row.case
                );
            }
            _ => {
                assert_eq!(
                    after["relationalDatabaseBlueprintId"], before["relationalDatabaseBlueprintId"],
                    "case {}: an update that named no blueprint must leave it alone, \
                     got {after:?}",
                    row.case
                );
            }
        }
        // Applying the modification rewrote nothing else.
        let moved = [
            "backupRetentionEnabled",
            "pendingModifiedValues",
            "relationalDatabaseBlueprintId",
            "engineVersion",
        ];
        assert_eq!(
            without(&after, &moved),
            without(&before, &moved),
            "case {}: applying the modification must leave every other member untouched, \
             got {after:?}",
            row.case
        );
    }
}

/// RULE: "When true, applies changes immediately" — an update that asks for that
/// writes the three deferrable attributes into the record before it answers, so
/// nothing is reported pending and the operation it logs is terminal.
#[tokio::test]
async fn apply_immediately_writes_the_deferrable_attributes_before_the_call_answers() {
    for row in ROWS {
        let name = "immediate-db";
        let emulator = with_available_database(row, name).await;
        let before = database(&emulator, name).await;
        let upgrade = row.upgrade_blueprint_id;
        let target_version = match upgrade {
            Some(blueprint_id) => Some(catalog_engine_version(&emulator, blueprint_id).await),
            None => None,
        };

        let mut body = json!({
            "relationalDatabaseName": name,
            "applyImmediately": true,
            "masterUserPassword": password_of(row.password_chars),
            "disableBackupRetention": true,
        });
        if let Some(blueprint_id) = upgrade {
            body["relationalDatabaseBlueprintId"] = json!(blueprint_id);
        }
        let updated = emulator.ok("UpdateRelationalDatabase", body).await;
        assert_update_operation(row.case, &updated["operations"], name, row.region, true);

        let after = database(&emulator, name).await;
        assert!(
            after["pendingModifiedValues"].is_null(),
            "case {}: applying immediately leaves nothing waiting, got {after:?}",
            row.case
        );
        assert_eq!(
            after["backupRetentionEnabled"].as_bool(),
            Some(false),
            "case {}: the backup retention change is in the record already, got {after:?}",
            row.case
        );
        if let (Some(blueprint_id), Some(version)) = (upgrade, target_version.as_deref()) {
            assert_eq!(
                (
                    after["relationalDatabaseBlueprintId"].as_str(),
                    after["engineVersion"].as_str()
                ),
                (Some(blueprint_id), Some(version)),
                "case {}: the blueprint update is in the record already, got {after:?}",
                row.case
            );
        }
        let moved = [
            "backupRetentionEnabled",
            "relationalDatabaseBlueprintId",
            "engineVersion",
        ];
        assert_eq!(
            without(&after, &moved),
            without(&before, &moved),
            "case {}: an immediate update must leave every other member untouched, got {after:?}",
            row.case
        );

        // Turning automated backup retention back ON is the same rule in the
        // other direction.
        emulator
            .ok(
                "UpdateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "applyImmediately": true,
                    "enableBackupRetention": true,
                }),
            )
            .await;
        let restored = database(&emulator, name).await;
        assert_eq!(
            restored["backupRetentionEnabled"].as_bool(),
            Some(true),
            "case {}: enableBackupRetention must turn retention back on, got {restored:?}",
            row.case
        );

        // A change an earlier call left waiting is applied too: once a call has
        // said to apply now, nothing is left waiting for the maintenance
        // window.
        let waiting = "flushed-db";
        let emulator = with_available_database(row, waiting).await;
        emulator
            .ok(
                "UpdateRelationalDatabase",
                json!({
                    "relationalDatabaseName": waiting,
                    "disableBackupRetention": true,
                }),
            )
            .await;
        emulator
            .ok(
                "UpdateRelationalDatabase",
                json!({
                    "relationalDatabaseName": waiting,
                    "applyImmediately": true,
                }),
            )
            .await;
        let flushed = database(&emulator, waiting).await;
        assert_eq!(
            (
                flushed["pendingModifiedValues"].is_null(),
                flushed["backupRetentionEnabled"].as_bool()
            ),
            (true, Some(false)),
            "case {}: applying immediately must leave nothing waiting and carry the earlier \
             change into the record, got {flushed:?}",
            row.case
        );
    }
}

/// RULE: "When true, the master user password is changed to a new strong password
/// generated by Lightsail" — the rotation is the service's own change, so it
/// waits for the maintenance window like any other password change, is reported
/// masked while it waits, and leaves every member a database read reports alone.
#[tokio::test]
async fn rotating_the_master_user_password_leaves_only_a_masked_pending_password() {
    for row in ROWS {
        let name = "rotated-db";
        let emulator = with_available_database(row, name).await;
        let before = database(&emulator, name).await;

        let started = Instant::now();
        let updated = emulator
            .ok(
                "UpdateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "rotateMasterUserPassword": true,
                }),
            )
            .await;
        let during = database(&emulator, name).await;
        if started.elapsed() < transition_window() {
            assert_update_operation(row.case, &updated["operations"], name, row.region, false);
            assert_eq!(
                during["pendingModifiedValues"],
                json!({"masterUserPassword": "****"}),
                "case {}: a rotation leaves exactly one masked password waiting, got {during:?}",
                row.case
            );
        }
        let after = wait_for_no_pending(row.case, &emulator, name).await;
        assert_eq!(
            after, before,
            "case {}: a password rotation changes no member a database read reports, \
             got {after:?}",
            row.case
        );
    }
}

/// RULE: an update asks for a CHANGE, so naming the value the database already
/// carries — the backup retention setting it has, or the blueprint it already
/// runs — changes nothing, leaves nothing waiting and logs a terminal operation.
#[tokio::test]
async fn asking_for_the_value_the_database_already_carries_changes_nothing() {
    for row in ROWS {
        let name = "unchanged-db";
        let emulator = with_available_database(row, name).await;
        let before = database(&emulator, name).await;

        for (case, body) in [
            (
                "backup retention that is already on",
                json!({
                    "relationalDatabaseName": name,
                    "enableBackupRetention": true,
                }),
            ),
            (
                "the blueprint the database already runs",
                json!({
                    "relationalDatabaseName": name,
                    "relationalDatabaseBlueprintId": row.blueprint_id,
                }),
            ),
            (
                "a request that names nothing but the database",
                json!({"relationalDatabaseName": name}),
            ),
        ] {
            let case = format!("{}, {case}", row.case);
            let updated = emulator.ok("UpdateRelationalDatabase", body).await;
            assert_update_operation(&case, &updated["operations"], name, row.region, true);
            let after = database(&emulator, name).await;
            assert_eq!(
                after, before,
                "case {case}: an update that asks for no change must leave the record \
                 identical, got {after:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// The requests this service refuses for `row`'s database `name`, each with the
/// wire error it answers and the fragments its message must name.
///
/// The rows span the classes the handler decides on: two members asking for
/// opposite changes, a certificate authority or a blueprint the service does not
/// publish, a blueprint of another engine or of an earlier major version, a
/// malformed window, a window shorter than the published minimum, two windows
/// that overlap, and a password outside the limits the row's engine publishes.
fn refusal_cases(row: &Row, name: &str) -> Vec<(String, Value, &'static str, Vec<String>)> {
    let engine_max = if row.blueprint_id.starts_with("postgres") {
        // The modelled `masterUserPassword` documentation: "PostgreSQL
        // Constraints: Must contain from 8 to 128 characters."
        128
    } else {
        // "MySQL Constraints: Must contain from 8 to 41 characters."
        41
    };
    let earlier_blueprint = if row.blueprint_id == "postgres_17" {
        Some("postgres_13")
    } else {
        None
    };

    let mut cases: Vec<(String, Value, &'static str, Vec<String>)> = vec![
        (
            "a password and a rotation at once".to_string(),
            json!({
                "relationalDatabaseName": name,
                "masterUserPassword": password_of(16),
                "rotateMasterUserPassword": true,
            }),
            "InvalidInputException",
            vec![
                "masterUserPassword".to_string(),
                "rotateMasterUserPassword".to_string(),
            ],
        ),
        (
            "backup retention turned on and off at once".to_string(),
            json!({
                "relationalDatabaseName": name,
                "enableBackupRetention": true,
                "disableBackupRetention": true,
            }),
            "InvalidInputException",
            vec![
                "enableBackupRetention".to_string(),
                "disableBackupRetention".to_string(),
            ],
        ),
        (
            "a certificate authority the service does not publish".to_string(),
            json!({
                "relationalDatabaseName": name,
                "caCertificateIdentifier": "rds-ca-9999",
            }),
            "InvalidInputException",
            vec![
                "caCertificateIdentifier".to_string(),
                "rds-ca-9999".to_string(),
            ],
        ),
        (
            "a published certificate authority spelled in another case".to_string(),
            json!({
                "relationalDatabaseName": name,
                "caCertificateIdentifier": "RDS-CA-RSA2048-G1",
            }),
            "InvalidInputException",
            vec!["caCertificateIdentifier".to_string()],
        ),
        (
            "a blueprint the catalog does not publish".to_string(),
            json!({
                "relationalDatabaseName": name,
                "relationalDatabaseBlueprintId": "mariadb_11",
            }),
            "InvalidInputException",
            vec![
                "relationalDatabaseBlueprintId".to_string(),
                "mariadb_11".to_string(),
            ],
        ),
        (
            "a blueprint of the other engine".to_string(),
            json!({
                "relationalDatabaseName": name,
                "relationalDatabaseBlueprintId": row.other_engine_blueprint_id,
            }),
            "InvalidInputException",
            vec![row.other_engine_blueprint_id.to_string()],
        ),
        (
            "a backup window that is not hh24:mi-hh24:mi".to_string(),
            json!({
                "relationalDatabaseName": name,
                "preferredBackupWindow": "16:00",
            }),
            "InvalidInputException",
            vec!["preferredBackupWindow".to_string()],
        ),
        (
            "a backup window with single-digit components".to_string(),
            json!({
                "relationalDatabaseName": name,
                "preferredBackupWindow": "1:00-1:40",
            }),
            "InvalidInputException",
            vec!["preferredBackupWindow".to_string()],
        ),
        (
            "a backup window shorter than the published minimum".to_string(),
            json!({
                "relationalDatabaseName": name,
                "preferredBackupWindow": "16:00-16:20",
            }),
            "InvalidInputException",
            vec!["preferredBackupWindow".to_string(), "30".to_string()],
        ),
        (
            "a maintenance window naming a day the service does not accept".to_string(),
            json!({
                "relationalDatabaseName": name,
                "preferredMaintenanceWindow": "Funday:10:00-Funday:10:30",
            }),
            "InvalidInputException",
            vec!["preferredMaintenanceWindow".to_string()],
        ),
        (
            "a maintenance window shorter than the published minimum".to_string(),
            json!({
                "relationalDatabaseName": name,
                "preferredMaintenanceWindow": "Tue:10:00-Tue:10:10",
            }),
            "InvalidInputException",
            vec!["preferredMaintenanceWindow".to_string(), "30".to_string()],
        ),
        (
            "two windows that overlap".to_string(),
            json!({
                "relationalDatabaseName": name,
                "preferredBackupWindow": "03:00-03:30",
                "preferredMaintenanceWindow": "Wed:03:15-Wed:03:45",
            }),
            "InvalidInputException",
            vec![
                "preferredBackupWindow".to_string(),
                "preferredMaintenanceWindow".to_string(),
            ],
        ),
        (
            "a password shorter than the engine accepts".to_string(),
            json!({
                "relationalDatabaseName": name,
                "masterUserPassword": password_of(7),
            }),
            "InvalidInputException",
            vec!["masterUserPassword".to_string()],
        ),
        (
            format!("a password one character past the {engine_max} this engine accepts"),
            json!({
                "relationalDatabaseName": name,
                "masterUserPassword": password_of(engine_max + 1),
            }),
            "InvalidInputException",
            vec!["masterUserPassword".to_string(), engine_max.to_string()],
        ),
        (
            "a password carrying a character the service forbids".to_string(),
            json!({
                "relationalDatabaseName": name,
                "masterUserPassword": "pass@word123",
            }),
            "InvalidInputException",
            vec!["masterUserPassword".to_string()],
        ),
    ];
    if let Some(blueprint_id) = earlier_blueprint {
        cases.push((
            "a blueprint of an earlier major version".to_string(),
            json!({
                "relationalDatabaseName": name,
                "relationalDatabaseBlueprintId": blueprint_id,
            }),
            "InvalidInputException",
            vec![blueprint_id.to_string()],
        ));
    }
    cases
}

/// RULE: a request the service cannot serve is refused with the modelled
/// envelope, and a refused update writes NOTHING — the record reads back exactly
/// as it was, with nothing left waiting.
#[tokio::test]
async fn a_request_the_service_cannot_serve_is_refused_and_writes_nothing() {
    for row in ROWS {
        let name = "refused-db";
        let emulator = with_available_database(row, name).await;
        let before = database(&emulator, name).await;

        for (case, body, expected_error, fragments) in refusal_cases(row, name) {
            let case = format!("{}, {case}", row.case);
            let (status, answer) = emulator.call("UpdateRelationalDatabase", body).await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, expected_error),
                "case {case}: expected the modelled {expected_error}, got {status} {answer:?}"
            );
            let message = answer["message"].as_str().unwrap_or_default();
            for fragment in &fragments {
                assert!(
                    message.contains(fragment.as_str()),
                    "case {case}: the refusal must name {fragment:?}, got {answer:?}"
                );
            }
            let after = database(&emulator, name).await;
            assert_eq!(
                after, before,
                "case {case}: a refused update must leave the record identical, got {after:?}"
            );
        }
    }
}

/// RULE: an update resolves `relationalDatabaseName` against the store, so a name
/// the account holds no database for is refused with the service's `DoesNotExist`
/// envelope and nothing is stored.
#[tokio::test]
async fn updating_an_unknown_database_answers_the_does_not_exist_envelope() {
    let emulator = Emulator::new();
    for name in MISSING_NAMES {
        let (status, body) = emulator
            .call(
                "UpdateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "publiclyAccessible": true,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {name:?}: expected the wire error NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {name:?}: expected the body code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The RelationalDatabase does not exist: {name}").as_str()),
            "case {name:?}: expected the message to name the resource type and the requested \
             name, got {body:?}"
        );
    }

    // Refusing stored nothing: a read of any of those names is still the same
    // envelope, so no record was created along the way.
    for name in MISSING_NAMES {
        let (status, body) = emulator
            .call(
                "GetRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (400, Some("DoesNotExist")),
            "case reading {name:?} after the refused update: expected the DoesNotExist \
             envelope, got {status} {body:?}"
        );
    }
}

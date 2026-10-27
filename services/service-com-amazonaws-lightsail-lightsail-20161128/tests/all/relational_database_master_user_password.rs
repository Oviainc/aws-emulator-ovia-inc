// SPDX-License-Identifier: CC0-1.0

//! `GetRelationalDatabaseMasterUserPassword` — reading ONE version of a managed
//! relational database's master user password, driven over the wire.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.GetRelationalDatabaseMasterUserPassword`
//! and an `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed.
//!
//! The rules under test, asserted over a SET of databases — both engines this
//! service publishes blueprints for, several engine versions, standard and
//! high-availability bundles in four Regions, a create that names its own
//! password (at the shortest length an engine admits, the longest and one in
//! between) and a create that names none — rather than over one call:
//!
//! * the answer is the version of the password the ONE persisted record holds:
//!   `CURRENT` the password the database is reached with now, `PREVIOUS` the
//!   version it replaced and `PENDING` the version a modification will rotate to
//!   `CURRENT`, each with the instant that version was created;
//! * a request that names no `passwordVersion` asks for `CURRENT` ("Default:
//!   CURRENT");
//! * a password change rotates the versions and moves no instant: the version
//!   reported as `PENDING` is the one later reported as `CURRENT`, under the
//!   createdAt it already had, and the version it replaced is then `PREVIOUS`;
//! * reading a password changes nothing — the same call answers the same version
//!   twice and leaves every member of the database read alone; and
//! * a request the service cannot serve — a name the account holds no database
//!   for, a version the database holds nothing for, a `passwordVersion` outside
//!   the modelled enum — is refused with the modelled envelope.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    MASKED_MASTER_USER_PASSWORD, STATE_TRANSITION_MILLIS,
};

use crate::instance_lifecycle::{error_code, Emulator};

/// How long to wait for a clock-owed change before giving up.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// The body member `code` Lightsail sets on a rejection whose cause is that what
/// the request names does not exist, and the wire error (awsJson `__type`) that
/// rejection carries.
const DOES_NOT_EXIST: &str = "DoesNotExist";
/// See [`DOES_NOT_EXIST`].
const NOT_FOUND: &str = "NotFoundException";
/// The wire error of the refusal the centralized modelled-constraint walk
/// answers for a member value outside the constraints the model declares, before
/// any handler runs.
const VALIDATION: &str = "ValidationException";

/// The `ResourceType` a managed relational database is reported under, which a
/// `DoesNotExist` message names.
const RESOURCE_TYPE: &str = "RelationalDatabase";

/// The fewest characters any engine this service publishes blueprints for admits
/// in a master user password, and the fewest any of them admits as an upper
/// bound.
///
/// SOURCE: the modelled `masterUserPassword` documentation — "MySQL Constraints:
/// Must contain from 8 to 41 characters", "PostgreSQL Constraints: Must contain
/// from 8 to 128 characters" — so a password the SERVICE generates has to sit
/// inside 8 to 41 characters to satisfy either engine.
const GENERATED_PASSWORD_BOUNDS: (usize, usize) = (8, 41);

/// The span the clock gives a change it owes, from the service's own constant.
///
/// A case that asserts the WAITING password version has to know whether its call
/// landed inside that span: measuring it is what keeps such a case exact on a
/// machine slow enough for the change to arrive first.
fn transition_window() -> Duration {
    Duration::from_millis(u64::try_from(STATE_TRANSITION_MILLIS).unwrap_or_default())
}

/// One database to read the master user password of, and the passwords the cases
/// set on it.
///
/// The rows differ along what the rules generalise over: the engine, the engine
/// version, the bundle's size and whether it is a high-availability plan, the
/// Region and Availability Zone the database lives in, whether the create names
/// a password of its own at all, and how long the passwords are — the shortest
/// the engine admits, the longest, and lengths in between.
#[derive(Debug)]
struct Row {
    case: &'static str,
    blueprint_id: &'static str,
    bundle_id: &'static str,
    availability_zone: &'static str,
    /// How many characters the password the create names carries, or `None` for
    /// a create that names no password at all.
    created_password_chars: Option<usize>,
    /// How many characters the password a later update names carries — inside
    /// the range this row's engine admits.
    replacement_chars: usize,
}

/// The databases every case here reads.
const ROWS: &[Row] = &[
    Row {
        case: "mysql 8.4, smallest standard bundle, created with the shortest password the \
               engine admits, replaced by the longest",
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_1_0",
        availability_zone: "us-east-1a",
        created_password_chars: Some(8),
        replacement_chars: 41,
    },
    Row {
        case: "postgres 13, large standard bundle, created with no password at all, replaced by \
               one longer than MySQL would accept",
        blueprint_id: "postgres_13",
        bundle_id: "large_1_0",
        availability_zone: "us-west-2b",
        created_password_chars: None,
        replacement_chars: 128,
    },
    Row {
        case: "postgres 17, micro high-availability bundle, created with a long password, \
               replaced by the shortest the engine admits",
        blueprint_id: "postgres_17",
        bundle_id: "micro_ha_1_0",
        availability_zone: "eu-west-3a",
        created_password_chars: Some(64),
        replacement_chars: 8,
    },
    Row {
        case: "mysql 8.4, medium high-availability bundle, created with a password in between",
        blueprint_id: "mysql_8_4",
        bundle_id: "medium_ha_1_0",
        availability_zone: "ap-northeast-1c",
        created_password_chars: Some(20),
        replacement_chars: 16,
    },
];

/// Names the account holds no database for, differing along the length, the
/// case, the digits and the separators. Every one satisfies the modelled
/// `ResourceName` pattern `^\w[\w\-]*\w$`, so the centralized walk passes it
/// through to the handler.
const MISSING_NAMES: &[&str] = &[
    "ab",
    "never-created",
    "Probe_7",
    "a-name-with-many-hyphen-separated-words-and-digits-0123456789",
];

/// The versions the modelled `RelationalDatabasePasswordVersion` enum publishes.
///
/// SOURCE: `smithy_query(query="shape",shape="RelationalDatabasePasswordVersion")`
/// — `CURRENT`, `PREVIOUS`, `PENDING`.
const PASSWORD_VERSIONS: &[&str] = &["CURRENT", "PREVIOUS", "PENDING"];

/// Values no `passwordVersion` member may carry: the published values in another
/// case, a value of another vocabulary, one with trailing space, and the empty
/// string.
const BAD_VERSIONS: &[&str] = &["current", "Current", "LATEST", "CURRENT ", ""];

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

/// Whether `value` is a password this service's engines accept: printable ASCII
/// other than a space, `/`, `"` and `@`, from the modelled `masterUserPassword`
/// documentation.
fn is_master_user_password(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_graphic() && !matches!(character, '/' | '"' | '@'))
}

/// The Region `row`'s database lives in: the Region of the Availability Zone it
/// names, which is that zone without its trailing zone letter.
fn region_of(row: &Row) -> &'static str {
    let zone = row.availability_zone;
    &zone[..zone.len() - 1]
}

/// The `CreateRelationalDatabase` body for `row`, naming one database.
fn create_body(row: &Row, name: &str) -> Value {
    let mut body = json!({
        "relationalDatabaseName": name,
        "relationalDatabaseBlueprintId": row.blueprint_id,
        "relationalDatabaseBundleId": row.bundle_id,
        "availabilityZone": row.availability_zone,
        "masterDatabaseName": "appdb",
        "masterUsername": "dbadmin",
    });
    if let Some(chars) = row.created_password_chars {
        body["masterUserPassword"] = json!(password_of(chars));
    }
    body
}

/// Create the database `name` for `row` and answer the emulator it lives in, once
/// the clock has settled it into `available`.
async fn with_available_database(row: &Row, name: &str) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok("CreateRelationalDatabase", create_body(row, name))
        .await;
    wait_for_available(&emulator, name).await;
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

/// The stored database `name`, once the clock has settled it into `available`.
async fn wait_for_available(emulator: &Emulator, name: &str) -> Value {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let read = database(emulator, name).await;
        if read["state"].as_str() == Some("available") {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {name}: the clock never settled the database into available, last read {read:?}"
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

/// The `GetRelationalDatabaseMasterUserPassword` request body for `name`, naming
/// `version` when one is given.
fn read_body(name: &str, version: Option<&str>) -> Value {
    let mut body = json!({"relationalDatabaseName": name});
    if let Some(version) = version {
        body["passwordVersion"] = json!(version);
    }
    body
}

/// The version `version` of `name`'s master user password, required to succeed.
///
/// `pub(crate)` so a sibling suite reads a database's master credentials through
/// this one call rather than through a second copy of it.
pub(crate) async fn read_password(emulator: &Emulator, name: &str, version: Option<&str>) -> Value {
    emulator
        .ok(
            "GetRelationalDatabaseMasterUserPassword",
            read_body(name, version),
        )
        .await
}

/// The password and the created instant of one answered version, each required
/// to be present.
fn version_of(case: &str, answered: &Value) -> (String, f64) {
    let password = answered["masterUserPassword"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a masterUserPassword, got {answered:?}"))
        .to_string();
    let created_at = answered["createdAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected an epoch-seconds createdAt, got {answered:?}"));
    assert!(
        is_master_user_password(&password),
        "case {case}: expected a password of the characters the engine admits, got {password:?}"
    );
    assert_ne!(
        password, MASKED_MASTER_USER_PASSWORD,
        "case {case}: the operation that answers the secret never answers the mask, \
         got {answered:?}"
    );
    assert!(
        created_at > 0.0,
        "case {case}: expected the instant the version was created, got {answered:?}"
    );
    (password, created_at)
}

/// Assert `body`/`status` is Lightsail's `DoesNotExist` envelope, naming every
/// fragment of `fragments` in its message.
fn assert_does_not_exist(case: &str, status: u16, body: &Value, fragments: &[&str]) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        NOT_FOUND,
        "case {case}: expected the wire error {NOT_FOUND}, got {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some(DOES_NOT_EXIST),
        "case {case}: expected the body code {DOES_NOT_EXIST}, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    for fragment in fragments {
        assert!(
            message.contains(fragment),
            "case {case}: expected the message to name {fragment:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The version the database holds
// ---------------------------------------------------------------------------

/// RULE: the answer is the CURRENT version of the password the ONE persisted
/// record holds — the secret the create named, or the strong one the service
/// generated for a create that named none — together with the instant that
/// version was created, which for a database nothing has modified is the instant
/// the database itself was created. A request that names no `passwordVersion`
/// asks for that same version ("Default: CURRENT").
#[tokio::test]
async fn the_current_version_is_the_password_the_database_was_created_with() {
    for row in ROWS {
        let name = "read-db";
        let emulator = with_available_database(row, name).await;
        let record = database(&emulator, name).await;

        let answered = read_password(&emulator, name, Some("CURRENT")).await;
        let (password, created_at) = version_of(row.case, &answered);
        match row.created_password_chars {
            Some(chars) => assert_eq!(
                password,
                password_of(chars),
                "case {}: the CURRENT version is the password the create named, got {answered:?}",
                row.case
            ),
            None => {
                let (min, max) = GENERATED_PASSWORD_BOUNDS;
                let length = password.chars().count();
                assert!(
                    (min..=max).contains(&length),
                    "case {}: a create that names no password is served a generated one inside \
                     {min} to {max} characters, got {length}",
                    row.case
                );
            }
        }
        assert_eq!(
            Some(created_at),
            record["createdAt"].as_f64(),
            "case {}: the CURRENT version of a database nothing has modified was created when \
             the database was, got {answered:?} against {record:?}",
            row.case
        );

        // "Default: CURRENT": naming no version asks for exactly that one.
        let defaulted = read_password(&emulator, name, None).await;
        assert_eq!(
            defaulted, answered,
            "case {}: a request that names no passwordVersion answers the CURRENT version, \
             got {defaulted:?} against {answered:?}",
            row.case
        );
    }
}

/// RULE: reading a password reports; it hands out no new one. The same call
/// answers the same version however often it is made, and leaves every member of
/// the database record exactly as it was.
#[tokio::test]
async fn reading_a_password_changes_nothing_for_the_next_caller() {
    for row in ROWS {
        let name = "reread-db";
        let emulator = with_available_database(row, name).await;
        let before = database(&emulator, name).await;

        let first = read_password(&emulator, name, Some("CURRENT")).await;
        for attempt in 0..3 {
            let again = read_password(&emulator, name, None).await;
            assert_eq!(
                again, first,
                "case {} read {attempt}: a read must answer the same version, got {again:?} \
                 against {first:?}",
                row.case
            );
        }
        let after = database(&emulator, name).await;
        assert_eq!(
            after, before,
            "case {}: reading a password changes no member a database read reports, got {after:?}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// Rotating the versions
// ---------------------------------------------------------------------------

/// RULE: a password change that asks to apply at once replaces the CURRENT
/// version with the one it names and makes the version it replaced PREVIOUS,
/// each keeping the instant it was created at — so the PREVIOUS version still
/// reports exactly what CURRENT reported before the change, and nothing is
/// PENDING.
#[tokio::test]
async fn a_password_applied_at_once_becomes_current_and_pushes_the_old_one_to_previous() {
    for row in ROWS {
        let name = "replaced-db";
        let emulator = with_available_database(row, name).await;
        let created = read_password(&emulator, name, Some("CURRENT")).await;
        let (old_password, old_created_at) = version_of(row.case, &created);

        let replacement = password_of(row.replacement_chars);
        emulator
            .ok(
                "UpdateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "masterUserPassword": replacement,
                    "applyImmediately": true,
                }),
            )
            .await;

        let current = read_password(&emulator, name, Some("CURRENT")).await;
        let (password, created_at) = version_of(row.case, &current);
        assert_eq!(
            password, replacement,
            "case {}: the CURRENT version is the password the update named, got {current:?}",
            row.case
        );
        assert!(
            created_at >= old_created_at,
            "case {}: the version the update created cannot predate the one it replaced, \
             got {created_at} against {old_created_at}",
            row.case
        );

        let previous = read_password(&emulator, name, Some("PREVIOUS")).await;
        assert_eq!(
            (
                previous["masterUserPassword"].as_str(),
                previous["createdAt"].as_f64()
            ),
            (Some(old_password.as_str()), Some(old_created_at)),
            "case {}: the PREVIOUS version is the replaced one, under the instant it already \
             carried, got {previous:?}",
            row.case
        );

        // The change is in the record, so nothing is waiting to rotate to
        // CURRENT any more.
        let (status, body) = emulator
            .call(
                "GetRelationalDatabaseMasterUserPassword",
                read_body(name, Some("PENDING")),
            )
            .await;
        assert_does_not_exist(row.case, status, &body, &["PENDING", name]);
    }
}

/// RULE: a password change left for the database's maintenance window is the
/// PENDING version — "the newest version of the password that will rotate to
/// CURRENT" — while CURRENT is still the password in force; when the clock
/// applies it, that same version becomes CURRENT under the instant it already
/// carried, the one it replaced becomes PREVIOUS, and "after the PENDING
/// password rotates to CURRENT, the PENDING password is no longer available".
#[tokio::test]
async fn a_waiting_password_is_the_pending_version_until_the_clock_rotates_it() {
    for row in ROWS {
        let name = "rotating-db";
        let emulator = with_available_database(row, name).await;
        let created = read_password(&emulator, name, Some("CURRENT")).await;
        let (old_password, old_created_at) = version_of(row.case, &created);

        // A rotation asks the service for a password of its own, so the case
        // names none: what it asserts is the VERSION, not a value it chose.
        let started = Instant::now();
        emulator
            .ok(
                "UpdateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "rotateMasterUserPassword": true,
                }),
            )
            .await;

        let mut waiting: Option<(String, f64)> = None;
        if started.elapsed() < transition_window() {
            let pending = read_password(&emulator, name, Some("PENDING")).await;
            let (pending_password, pending_created_at) = version_of(row.case, &pending);
            assert_ne!(
                pending_password, old_password,
                "case {}: a rotation mints a password of its own, got {pending:?}",
                row.case
            );
            // The secret is the PENDING version's own; the database read may say
            // only THAT one is waiting.
            let during = database(&emulator, name).await;
            if during["pendingModifiedValues"].is_object() {
                assert_eq!(
                    during["pendingModifiedValues"]["masterUserPassword"].as_str(),
                    Some(MASKED_MASTER_USER_PASSWORD),
                    "case {}: a waiting password is reported masked by a database read, \
                     got {during:?}",
                    row.case
                );
                let still_current = read_password(&emulator, name, Some("CURRENT")).await;
                assert_eq!(
                    still_current["masterUserPassword"].as_str(),
                    Some(old_password.as_str()),
                    "case {}: a waiting password has not replaced the one in force, \
                     got {still_current:?}",
                    row.case
                );
                waiting = Some((pending_password, pending_created_at));
            }
        }

        wait_for_no_pending(row.case, &emulator, name).await;

        let current = read_password(&emulator, name, Some("CURRENT")).await;
        let (password, created_at) = version_of(row.case, &current);
        assert_ne!(
            password, old_password,
            "case {}: the clock must apply the rotation, got {current:?}",
            row.case
        );
        if let Some((pending_password, pending_created_at)) = waiting {
            assert_eq!(
                (password.as_str(), created_at),
                (pending_password.as_str(), pending_created_at),
                "case {}: the version reported PENDING is the one that becomes CURRENT, under \
                 the instant it already carried, got {current:?}",
                row.case
            );
        }

        let previous = read_password(&emulator, name, Some("PREVIOUS")).await;
        assert_eq!(
            (
                previous["masterUserPassword"].as_str(),
                previous["createdAt"].as_f64()
            ),
            (Some(old_password.as_str()), Some(old_created_at)),
            "case {}: the version the rotation replaced becomes PREVIOUS, got {previous:?}",
            row.case
        );

        let (status, body) = emulator
            .call(
                "GetRelationalDatabaseMasterUserPassword",
                read_body(name, Some("PENDING")),
            )
            .await;
        assert_does_not_exist(row.case, status, &body, &["PENDING", name]);
    }
}

/// RULE: a change asks for a DIFFERENT password, so naming the one the database
/// already holds replaces nothing: the version it holds keeps both its secret
/// and the instant it was created at, and no version is pushed to PREVIOUS.
#[tokio::test]
async fn naming_the_password_the_database_already_holds_moves_no_version() {
    for row in ROWS {
        let name = "unchanged-password-db";
        let emulator = with_available_database(row, name).await;
        let before = read_password(&emulator, name, None).await;
        let (held, _) = version_of(row.case, &before);

        emulator
            .ok(
                "UpdateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "masterUserPassword": held,
                    "applyImmediately": true,
                }),
            )
            .await;

        let after = read_password(&emulator, name, None).await;
        assert_eq!(
            after, before,
            "case {}: the version is the one the database already held, under the instant it \
             already carried, got {after:?} against {before:?}",
            row.case
        );
        let (status, body) = emulator
            .call(
                "GetRelationalDatabaseMasterUserPassword",
                read_body(name, Some("PREVIOUS")),
            )
            .await;
        assert_does_not_exist(row.case, status, &body, &["PREVIOUS", name]);
    }
}

/// RULE: the password the service generates belongs to the database it was
/// generated for — two databases of one account are never handed the same one.
#[tokio::test]
async fn a_generated_password_is_the_database_s_own() {
    let row = &ROWS[1];
    let emulator = with_available_database(row, "generated-one").await;
    emulator
        .ok(
            "CreateRelationalDatabase",
            create_body(row, "generated-two"),
        )
        .await;
    wait_for_available(&emulator, "generated-two").await;

    let first = read_password(&emulator, "generated-one", None).await;
    let second = read_password(&emulator, "generated-two", None).await;
    let (first_password, _) = version_of("generated-one", &first);
    let (second_password, _) = version_of("generated-two", &second);
    assert_ne!(
        first_password, second_password,
        "two databases must not be handed the same generated password, got {first:?} \
         against {second:?}"
    );
}

/// RULE: a database restored from a snapshot is reached with the master user
/// password the snapshot captured, as the CURRENT version of the resource the
/// restore created — so it reports the source's secret under the restored
/// database's own creation instant, and has no PREVIOUS version.
#[tokio::test]
async fn a_restored_database_answers_with_the_password_the_snapshot_captured() {
    for row in ROWS {
        let source = "snapshot-src";
        let emulator = with_available_database(row, source).await;
        let expected = version_of(row.case, &read_password(&emulator, source, None).await).0;

        // The snapshot and the restore are signed for the Region the database
        // lives in: a snapshot is restorable only in its own Region.
        let region = region_of(row);
        emulator
            .ok_at(
                None,
                Some(region),
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": source,
                    "relationalDatabaseSnapshotName": "snapshot-one",
                }),
            )
            .await;
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        loop {
            let snapshot = emulator
                .ok(
                    "GetRelationalDatabaseSnapshot",
                    json!({"relationalDatabaseSnapshotName": "snapshot-one"}),
                )
                .await["relationalDatabaseSnapshot"]
                .clone();
            if snapshot["state"].as_str() == Some("available") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {}: the clock never settled the snapshot, last read {snapshot:?}",
                row.case
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        let restored = "snapshot-restored";
        emulator
            .ok_at(
                None,
                Some(region),
                "CreateRelationalDatabaseFromSnapshot",
                json!({
                    "relationalDatabaseName": restored,
                    "relationalDatabaseSnapshotName": "snapshot-one",
                }),
            )
            .await;
        let record = wait_for_available(&emulator, restored).await;

        let answered = read_password(&emulator, restored, None).await;
        let (password, created_at) = version_of(row.case, &answered);
        assert_eq!(
            password, expected,
            "case {}: a restored database is reached with the password the snapshot captured, \
             got {answered:?}",
            row.case
        );
        assert_eq!(
            Some(created_at),
            record["createdAt"].as_f64(),
            "case {}: the restored database's CURRENT version was created when that database \
             was, got {answered:?} against {record:?}",
            row.case
        );
        let (status, body) = emulator
            .call(
                "GetRelationalDatabaseMasterUserPassword",
                read_body(restored, Some("PREVIOUS")),
            )
            .await;
        assert_does_not_exist(row.case, status, &body, &["PREVIOUS", restored]);
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a version the database holds nothing for — PREVIOUS before any password
/// has been replaced, PENDING while no password change is waiting — is answered
/// with the service's `DoesNotExist` envelope, naming the version asked for and
/// the database it was asked of.
#[tokio::test]
async fn a_version_the_database_holds_nothing_for_does_not_exist() {
    for row in ROWS {
        let name = "one-version-db";
        let emulator = with_available_database(row, name).await;
        for version in ["PREVIOUS", "PENDING"] {
            let case = format!("{}, {version}", row.case);
            let (status, body) = emulator
                .call(
                    "GetRelationalDatabaseMasterUserPassword",
                    read_body(name, Some(version)),
                )
                .await;
            assert_does_not_exist(&case, status, &body, &[version, name]);
        }
        // The refusals changed nothing: the version the database does hold still
        // reads back.
        let still = read_password(&emulator, name, Some("CURRENT")).await;
        version_of(row.case, &still);
    }
}

/// RULE: a well-formed name the account holds no database for is the service's
/// `DoesNotExist` envelope, naming the resource type and the requested name —
/// whichever version the request asks for, and whether it names one at all.
#[tokio::test]
async fn a_name_the_account_holds_no_database_for_does_not_exist() {
    let emulator = Emulator::new();
    // One database exists, so the refusal is about the NAME rather than about an
    // empty store.
    emulator
        .ok("CreateRelationalDatabase", create_body(&ROWS[0], "held-db"))
        .await;

    for name in MISSING_NAMES {
        for version in [None, Some("CURRENT"), Some("PREVIOUS"), Some("PENDING")] {
            let case = format!("{name:?} at {version:?}");
            let (status, body) = emulator
                .call(
                    "GetRelationalDatabaseMasterUserPassword",
                    read_body(name, version),
                )
                .await;
            assert_does_not_exist(
                &case,
                status,
                &body,
                &[&format!("The {RESOURCE_TYPE} does not exist: {name}")],
            );
        }
    }
}

/// RULE: `passwordVersion` carries one of the values the modelled
/// `RelationalDatabasePasswordVersion` enum publishes; anything else is outside
/// the constraint the model declares, so it is refused at the boundary, naming
/// the member and the value set — never served, and never answered 501.
#[tokio::test]
async fn a_password_version_outside_the_modelled_enum_is_refused() {
    let row = &ROWS[0];
    let name = "enum-db";
    let emulator = with_available_database(row, name).await;

    for version in BAD_VERSIONS {
        let (status, body) = emulator
            .call(
                "GetRelationalDatabaseMasterUserPassword",
                read_body(name, Some(version)),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {version:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            VALIDATION,
            "case {version:?}: expected the wire error {VALIDATION}, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("passwordVersion"),
            "case {version:?}: expected the message to name the member, got {message:?}"
        );
    }

    // Every published value is served, which is what makes the refusals above
    // about the VALUE rather than about the member.
    for version in PASSWORD_VERSIONS {
        let (status, body) = emulator
            .call(
                "GetRelationalDatabaseMasterUserPassword",
                read_body(name, Some(version)),
            )
            .await;
        assert_ne!(
            error_code(&body),
            VALIDATION,
            "case {version:?}: a published version is never refused at the boundary, \
             got {status} {body:?}"
        );
        assert_ne!(
            status, 501,
            "case {version:?}: the operation is implemented, got {body:?}"
        );
    }
}

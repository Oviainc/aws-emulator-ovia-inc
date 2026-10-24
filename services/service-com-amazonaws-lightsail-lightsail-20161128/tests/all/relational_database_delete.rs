// SPDX-License-Identifier: CC0-1.0

//! `DeleteRelationalDatabase` — removing one managed relational database, with or
//! without the final snapshot of it, driven over the wire.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.DeleteRelationalDatabase` and an
//! `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed.
//!
//! The rules under test, asserted over a SET of databases — both engines this
//! service publishes blueprints for, several engine versions, standard and
//! high-availability bundles of different sizes, in four Regions, and every form
//! the two snapshot members take — rather than over one recorded call:
//!
//! * a delete removes exactly the ONE database the request names from the store,
//!   so a later read of that name is refused with the service's `DoesNotExist`
//!   envelope while every other database the account holds reads back untouched;
//! * `skipFinalSnapshot` decides whether a final snapshot is taken first, and its
//!   documented default is `false`, so a request that leaves the member out takes
//!   one too; the snapshot that is taken is the same point-in-time copy
//!   `CreateRelationalDatabaseSnapshot` makes — the source's engine, version, disk
//!   size, bundle, blueprint, ARN and location, read out of the record as it stood
//!   — and it OUTLIVES the database, in `creating` until the clock settles it;
//! * the two snapshot members have to agree: naming a final snapshot while
//!   skipping it, and skipping nothing while naming no snapshot, are both refused
//!   with the modelled 400 `InvalidInputException`, and a refused delete removes
//!   nothing;
//! * a final snapshot name the account already holds is refused the same way, so a
//!   delete that cannot take the snapshot it was asked for does not delete the
//!   data either; and
//! * the call logs one terminal `DeleteRelationalDatabase` entry against the
//!   database — plus the `CreateRelationalDatabaseSnapshot` entry the final
//!   snapshot earned, left non-terminal until that copy is due — each readable
//!   back by its id.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// How long to wait for a clock-owed transition before giving up.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// One database to delete, and the form the delete's snapshot members take.
///
/// The rows differ along everything the rule generalises over: the engine and its
/// version, the bundle's size and whether it is a high-availability plan (so the
/// record has a standby zone and a different disk size for the snapshot to
/// report), the Region and Availability Zone the database lives in, whether the
/// request carries `skipFinalSnapshot` at all and with which value, and the form
/// of the final snapshot's name.
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
    /// The `skipFinalSnapshot` value the delete carries, or `None` to leave the
    /// member out entirely — which asks for a final snapshot just the same, since
    /// the modelled member's documented default is `false`.
    skip_final_snapshot: Option<bool>,
    /// The name the final snapshot must be taken under, or `None` for a delete
    /// that skips it.
    final_snapshot_name: Option<&'static str>,
}

/// The databases every case here deletes.
const ROWS: &[Row] = &[
    Row {
        case: "mysql on the smallest standard bundle, skipFinalSnapshot left out entirely",
        db_name: "delete-mysql-default",
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_1_0",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        skip_final_snapshot: None,
        final_snapshot_name: Some("final-mysql-default"),
    },
    Row {
        case: "postgres 17 on the largest standard bundle, skipFinalSnapshot false",
        db_name: "delete_pg17_explicit",
        blueprint_id: "postgres_17",
        bundle_id: "large_1_0",
        availability_zone: "us-west-2b",
        region: "us-west-2",
        skip_final_snapshot: Some(false),
        final_snapshot_name: Some("final_pg17_explicit_0"),
    },
    Row {
        case: "postgres 13 on a micro high-availability bundle, skipFinalSnapshot true",
        db_name: "delete-pg13-skipped",
        blueprint_id: "postgres_13",
        bundle_id: "micro_ha_1_0",
        availability_zone: "eu-west-3a",
        region: "eu-west-3",
        skip_final_snapshot: Some(true),
        final_snapshot_name: None,
    },
    Row {
        case: "postgres 18 on a medium high-availability bundle, a final snapshot named with digits",
        db_name: "DeletePg18",
        blueprint_id: "postgres_18",
        bundle_id: "medium_ha_1_0",
        availability_zone: "ap-northeast-1c",
        region: "ap-northeast-1",
        skip_final_snapshot: Some(false),
        final_snapshot_name: Some("Final-Pg18-2026"),
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

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// The `CreateRelationalDatabase` body for `row`.
fn create_body(row: &Row) -> Value {
    json!({
        "relationalDatabaseName": row.db_name,
        "relationalDatabaseBlueprintId": row.blueprint_id,
        "relationalDatabaseBundleId": row.bundle_id,
        "availabilityZone": row.availability_zone,
        "masterDatabaseName": "appdata",
        "masterUsername": "dbmaster",
        "masterUserPassword": "Pw9#x!ZqPw9#x!Zq",
    })
}

/// The `DeleteRelationalDatabase` body for `row`: only the members the row names,
/// so a row that leaves `skipFinalSnapshot` out really does send a request without
/// it.
fn delete_body(row: &Row) -> Value {
    let mut body = json!({"relationalDatabaseName": row.db_name});
    if let Some(skip) = row.skip_final_snapshot {
        body["skipFinalSnapshot"] = json!(skip);
    }
    if let Some(name) = row.final_snapshot_name {
        body["finalRelationalDatabaseSnapshotName"] = json!(name);
    }
    body
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

/// Create the database `row` names and answer the record once the clock has
/// settled it into `available`.
async fn with_available_database(emulator: &Emulator, row: &Row) -> Value {
    emulator
        .ok("CreateRelationalDatabase", create_body(row))
        .await;
    wait_for_state(emulator, row.db_name, "available").await
}

/// Every snapshot `GetRelationalDatabaseSnapshots` reports, walking every page.
async fn all_snapshots(emulator: &Emulator) -> Vec<Value> {
    let mut all: Vec<Value> = Vec::new();
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
        let entries = body["relationalDatabaseSnapshots"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case listing snapshots at {token:?}: expected an array, got {body:?}")
            })
            .clone();
        all.extend(entries);
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    all
}

/// The listed snapshot `name`, which must be reported exactly once.
async fn snapshot(emulator: &Emulator, name: &str) -> Value {
    let all = all_snapshots(emulator).await;
    let matching: Vec<&Value> = all
        .iter()
        .filter(|entry| entry["name"].as_str() == Some(name))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case reading snapshot {name} back from the listing: expected exactly one entry, \
         got {matching:?}"
    );
    matching[0].clone()
}

/// The listed snapshot `name`, once the clock has settled it into `state`.
async fn wait_for_snapshot_state(emulator: &Emulator, name: &str, state: &str) -> Value {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let read = snapshot(emulator, name).await;
        if read["state"].as_str() == Some(state) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {name}: the clock never settled the snapshot into {state}, last read {read:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Assert reading `name` back is refused with the service's `DoesNotExist`
/// envelope: HTTP 400, wire error `NotFoundException`, body `code` `DoesNotExist`
/// and the resource-type/name message.
async fn assert_database_gone(emulator: &Emulator, case: &str, name: &str) {
    let (status, body) = emulator
        .call(
            "GetRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected a 400 refusal reading {name} back, got {status} {body:?}"
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
        Some(format!("The RelationalDatabase does not exist: {name}").as_str()),
        "case {case}: expected the resource-type/name message for {name}, got {body:?}"
    );
}

/// The entries of an `operations` list, which must be a list.
fn entries(case: &str, operations: &Value) -> Vec<Value> {
    operations
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations list, got {operations:?}"))
        .clone()
}

/// Assert `entry` is a log entry of `operation_type` against `resource_name` of
/// `resource_type`, in `region`, whose terminality is `terminal`; answer its id.
fn assert_entry(
    case: &str,
    entry: &Value,
    operation_type: &str,
    resource_type: &str,
    resource_name: &str,
    region: &str,
    terminal: bool,
) -> String {
    assert_eq!(
        (
            entry["operationType"].as_str(),
            entry["resourceType"].as_str(),
            entry["resourceName"].as_str(),
        ),
        (
            Some(operation_type),
            Some(resource_type),
            Some(resource_name)
        ),
        "case {case}: expected a {operation_type} entry against the {resource_type} \
         {resource_name}, got {entry:?}"
    );
    assert_eq!(
        entry["location"]["regionName"].as_str(),
        Some(region),
        "case {case}: the entry is located where its resource is, got {entry:?}"
    );
    let want_status = if terminal { "Succeeded" } else { "Started" };
    assert_eq!(
        (entry["status"].as_str(), entry["isTerminal"].as_bool()),
        (Some(want_status), Some(terminal)),
        "case {case}: expected a {want_status} entry with isTerminal {terminal}, got {entry:?}"
    );
    let id = entry["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a minted operation id, got {entry:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    id.to_string()
}

/// Assert `GetOperation` reports the entry the call answered with — one log, not
/// two.
async fn assert_operation_readable(emulator: &Emulator, case: &str, entry: &Value) {
    let id = entry["id"].as_str().unwrap_or_default();
    let read_back = emulator
        .ok("GetOperation", json!({"operationId": id}))
        .await;
    for member in [
        "id",
        "operationType",
        "resourceName",
        "resourceType",
        "operationDetails",
        "createdAt",
        "location",
    ] {
        assert_eq!(
            read_back["operation"][member], entry[member],
            "case {case}: GetOperation must report the entry the call logged, expected \
             {member} {:?}, got {:?}",
            entry[member], read_back["operation"][member]
        );
    }
}

// ---------------------------------------------------------------------------
// The delete itself
// ---------------------------------------------------------------------------

/// RULE: a delete removes the one database it names and takes the final snapshot
/// `skipFinalSnapshot` asks for first — a point-in-time copy of the record as it
/// stood, which outlives the database — logging one terminal
/// `DeleteRelationalDatabase` entry plus the snapshot's own create.
#[tokio::test]
async fn deleting_a_database_removes_it_and_takes_the_final_snapshot_it_was_asked_for() {
    for row in ROWS {
        let emulator = Emulator::new();
        // A second database the delete must leave alone, so "removes exactly one
        // record" is asserted rather than assumed.
        let bystander = Row {
            case: row.case,
            db_name: "bystander-db",
            final_snapshot_name: None,
            skip_final_snapshot: Some(true),
            ..*row
        };
        let before = with_available_database(&emulator, row).await;
        let bystander_before = with_available_database(&emulator, &bystander).await;

        let deleted = emulator
            .ok("DeleteRelationalDatabase", delete_body(row))
            .await;
        let logged = entries(row.case, &deleted["operations"]);

        // One entry per resource the call acted on: the final snapshot's create
        // when one was taken, then the database's own delete.
        let want_entries = if row.final_snapshot_name.is_some() { 2 } else { 1 };
        assert_eq!(
            logged.len(),
            want_entries,
            "case {}: expected {want_entries} log entries, got {logged:?}",
            row.case
        );
        if let Some(snapshot_name) = row.final_snapshot_name {
            // The copy is not restorable when the call returns, so its entry is
            // non-terminal and the clock owes it its `Succeeded` status.
            assert_entry(
                row.case,
                &logged[0],
                "CreateRelationalDatabaseSnapshot",
                "RelationalDatabaseSnapshot",
                snapshot_name,
                row.region,
                false,
            );
            assert_operation_readable(&emulator, row.case, &logged[0]).await;
        }
        // The record is gone as the call returns, so the delete's own entry is
        // terminal.
        let delete_entry = &logged[want_entries - 1];
        assert_entry(
            row.case,
            delete_entry,
            "DeleteRelationalDatabase",
            "RelationalDatabase",
            row.db_name,
            row.region,
            true,
        );
        assert_operation_readable(&emulator, row.case, delete_entry).await;

        // The database is gone, and only that one: the bystander reads back
        // identical.
        assert_database_gone(&emulator, row.case, row.db_name).await;
        assert_eq!(
            database(&emulator, "bystander-db").await,
            bystander_before,
            "case {}: the delete must leave every other database untouched",
            row.case
        );

        // Deleting it again is refused rather than reporting a deletion that did
        // not happen.
        let (status, body) = emulator
            .call("DeleteRelationalDatabase", delete_body(row))
            .await;
        assert_eq!(
            status, 400,
            "case {}: deleting a database twice is refused, got {status} {body:?}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {}: expected the modelled NotFoundException, got {body:?}",
            row.case
        );

        match row.final_snapshot_name {
            // The final snapshot is the same point-in-time copy
            // `CreateRelationalDatabaseSnapshot` makes, read out of the source as
            // it stood — and it survives the database it was taken of.
            Some(snapshot_name) => {
                let taken = snapshot(&emulator, snapshot_name).await;
                assert_eq!(
                    taken["fromRelationalDatabaseName"].as_str(),
                    Some(row.db_name),
                    "case {}: the snapshot names the database it was taken of, got {taken:?}",
                    row.case
                );
                for (member, want) in [
                    ("fromRelationalDatabaseArn", &before["arn"]),
                    ("engine", &before["engine"]),
                    ("engineVersion", &before["engineVersion"]),
                    (
                        "fromRelationalDatabaseBundleId",
                        &before["relationalDatabaseBundleId"],
                    ),
                    (
                        "fromRelationalDatabaseBlueprintId",
                        &before["relationalDatabaseBlueprintId"],
                    ),
                    ("sizeInGb", &before["hardware"]["diskSizeInGb"]),
                    ("location", &before["location"]),
                ] {
                    assert_eq!(
                        taken[member], *want,
                        "case {}: expected the snapshot's {member} to be the source's {want:?}, \
                         got {:?}",
                        row.case, taken[member]
                    );
                }
                assert_eq!(
                    taken["resourceType"].as_str(),
                    Some("RelationalDatabaseSnapshot"),
                    "case {}: expected the modelled resource type, got {taken:?}",
                    row.case
                );
                // `DeleteRelationalDatabase` carries no `tags` member, so the
                // final snapshot carries none — an EMPTY list, not a null.
                assert_eq!(
                    taken["tags"],
                    json!([]),
                    "case {}: expected no tags on the final snapshot, got {:?}",
                    row.case,
                    taken["tags"]
                );
                let support_code = taken["supportCode"].as_str().unwrap_or_default();
                assert!(
                    !support_code.is_empty() && taken["arn"].as_str().is_some_and(|arn| arn
                        .contains(&format!(":{}:", row.region))),
                    "case {}: expected a minted ARN in the source's region and a support code, \
                     got {taken:?}",
                    row.case
                );
                // The copy is taken by the clock, not by the read that reports it.
                let settled = wait_for_snapshot_state(&emulator, snapshot_name, "available").await;
                assert_eq!(
                    json!({
                        "from": settled["fromRelationalDatabaseName"].clone(),
                        "arn": settled["arn"].clone(),
                    }),
                    json!({
                        "from": taken["fromRelationalDatabaseName"].clone(),
                        "arn": taken["arn"].clone(),
                    }),
                    "case {}: settling the snapshot must not rewrite what it reports",
                    row.case
                );
            }
            // A delete that skipped the final snapshot took none: the only
            // snapshots in the store are the ones something else created, and
            // there are none here.
            None => assert_eq!(
                all_snapshots(&emulator).await,
                Vec::<Value>::new(),
                "case {}: a delete that skips the final snapshot creates no snapshot",
                row.case
            ),
        }
    }
}

/// RULE: a snapshot is a point-in-time copy that keeps reporting what its source
/// was, so one taken BEFORE a delete outlives the database just as the final
/// snapshot does — and the delete leaves it exactly as it was.
#[tokio::test]
async fn snapshots_taken_before_the_delete_outlive_the_database() {
    for row in ROWS {
        let emulator = Emulator::new();
        with_available_database(&emulator, row).await;
        emulator
            .ok(
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": row.db_name,
                    "relationalDatabaseSnapshotName": "earlier-snapshot",
                    "tags": [{"key": "env", "value": "prod"}],
                }),
            )
            .await;
        let before = wait_for_snapshot_state(&emulator, "earlier-snapshot", "available").await;

        emulator
            .ok("DeleteRelationalDatabase", delete_body(row))
            .await;
        assert_database_gone(&emulator, row.case, row.db_name).await;

        let after = snapshot(&emulator, "earlier-snapshot").await;
        assert_eq!(
            after, before,
            "case {}: the delete must leave an earlier snapshot of the database untouched, \
             got {after:?}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// What the service refuses
// ---------------------------------------------------------------------------

/// RULE: the two snapshot members have to agree — a final snapshot name may not be
/// given while the final snapshot is skipped, and one must be given while it is
/// not (the member's default being `false`) — and a refused delete removes
/// nothing.
#[tokio::test]
async fn a_delete_whose_snapshot_members_disagree_is_refused_and_deletes_nothing() {
    for row in ROWS {
        let emulator = Emulator::new();
        let before = with_available_database(&emulator, row).await;

        // Each row of this table is one way the two members can disagree: naming
        // a snapshot while skipping it (both forms of name), and skipping nothing
        // while naming none (the member false, and the member left out so its
        // documented default decides).
        let bodies = [
            (
                "a final snapshot named while skipFinalSnapshot is true",
                json!({
                    "relationalDatabaseName": row.db_name,
                    "skipFinalSnapshot": true,
                    "finalRelationalDatabaseSnapshotName": "unwanted-snapshot",
                }),
            ),
            (
                "a differently spelled final snapshot named while skipping it",
                json!({
                    "relationalDatabaseName": row.db_name,
                    "skipFinalSnapshot": true,
                    "finalRelationalDatabaseSnapshotName": "Unwanted_Snapshot_2",
                }),
            ),
            (
                "skipFinalSnapshot false with no final snapshot named",
                json!({
                    "relationalDatabaseName": row.db_name,
                    "skipFinalSnapshot": false,
                }),
            ),
            (
                "neither member given, so the documented default false applies",
                json!({"relationalDatabaseName": row.db_name}),
            ),
        ];
        for (case, body) in bodies {
            let case = format!("{}, {case}", row.case);
            let (status, answered) = emulator.call("DeleteRelationalDatabase", body).await;
            assert_eq!(
                status, 400,
                "case {case}: expected a 400 refusal, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "InvalidInputException",
                "case {case}: expected the modelled InvalidInputException, got {answered:?}"
            );
            assert!(
                answered["message"]
                    .as_str()
                    .is_some_and(|message| message
                        .contains("finalRelationalDatabaseSnapshotName")),
                "case {case}: the refusal must name the member that is wrong, got {answered:?}"
            );

            // Nothing was deleted and nothing was snapshotted.
            assert_eq!(
                database(&emulator, row.db_name).await,
                before,
                "case {case}: a refused delete must leave the database untouched"
            );
            assert_eq!(
                all_snapshots(&emulator).await,
                Vec::<Value>::new(),
                "case {case}: a refused delete must store no snapshot"
            );
        }
    }
}

/// RULE: a name the account holds no database for is refused with the service's
/// `DoesNotExist` envelope — never a hollow 200 — and nothing is stored, whether
/// or not the request asked for a final snapshot.
#[tokio::test]
async fn a_name_the_account_holds_no_database_for_is_refused() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    // A database that exists, so the store is not merely empty.
    let before = with_available_database(&emulator, row).await;

    for missing in MISSING_NAMES {
        for (case, body) in [
            (
                "asking for a final snapshot",
                json!({
                    "relationalDatabaseName": missing,
                    "finalRelationalDatabaseSnapshotName": "snapshot-of-nothing",
                }),
            ),
            (
                "skipping the final snapshot",
                json!({
                    "relationalDatabaseName": missing,
                    "skipFinalSnapshot": true,
                }),
            ),
        ] {
            let case = format!("{missing}, {case}");
            let (status, answered) = emulator.call("DeleteRelationalDatabase", body).await;
            assert_eq!(
                status, 400,
                "case {case}: expected a 400 refusal, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "NotFoundException",
                "case {case}: expected the modelled NotFoundException, got {answered:?}"
            );
            assert_eq!(
                answered["code"].as_str(),
                Some("DoesNotExist"),
                "case {case}: expected code DoesNotExist, got {answered:?}"
            );
            assert_eq!(
                answered["message"].as_str(),
                Some(format!("The RelationalDatabase does not exist: {missing}").as_str()),
                "case {case}: expected the resource-type/name message, got {answered:?}"
            );
            assert_eq!(
                all_snapshots(&emulator).await,
                Vec::<Value>::new(),
                "case {case}: a refused delete must store no snapshot"
            );
        }
    }
    assert_eq!(
        database(&emulator, row.db_name).await,
        before,
        "case a database the request never named: it must read back untouched"
    );
}

/// RULE: a snapshot name addresses ONE snapshot, so a final snapshot name the
/// account already holds is refused with the modelled `InvalidInputException` —
/// and because the snapshot cannot be taken, the database is not deleted either.
#[tokio::test]
async fn a_final_snapshot_name_already_taken_is_refused_and_deletes_nothing() {
    for row in ROWS {
        let emulator = Emulator::new();
        let before = with_available_database(&emulator, row).await;
        emulator
            .ok(
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": row.db_name,
                    "relationalDatabaseSnapshotName": "taken-name",
                }),
            )
            .await;
        let snapshot_before = snapshot(&emulator, "taken-name").await;

        let (status, answered) = emulator
            .call(
                "DeleteRelationalDatabase",
                json!({
                    "relationalDatabaseName": row.db_name,
                    "finalRelationalDatabaseSnapshotName": "taken-name",
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {}: expected a 400 refusal, got {status} {answered:?}",
            row.case
        );
        assert_eq!(
            error_code(&answered),
            "InvalidInputException",
            "case {}: expected the modelled InvalidInputException, got {answered:?}",
            row.case
        );

        // The data is still there, and so is the snapshot the name belongs to.
        assert_eq!(
            database(&emulator, row.db_name).await,
            before,
            "case {}: a delete that cannot take its final snapshot deletes nothing",
            row.case
        );
        assert_eq!(
            snapshot(&emulator, "taken-name").await,
            snapshot_before,
            "case {}: the refused delete must leave the snapshot that holds the name \
             untouched",
            row.case
        );

        // Naming a FREE snapshot name instead deletes the database, so the
        // refusal was about the name and nothing else.
        emulator
            .ok(
                "DeleteRelationalDatabase",
                json!({
                    "relationalDatabaseName": row.db_name,
                    "finalRelationalDatabaseSnapshotName": "free-name",
                }),
            )
            .await;
        assert_database_gone(&emulator, row.case, row.db_name).await;
        assert_eq!(
            snapshot(&emulator, "free-name").await["fromRelationalDatabaseName"].as_str(),
            Some(row.db_name),
            "case {}: the final snapshot names the database it was taken of",
            row.case
        );
    }
}

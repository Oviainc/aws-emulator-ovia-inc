// SPDX-License-Identifier: CC0-1.0

//! `StartRelationalDatabase` and `StopRelationalDatabase` — shutting one managed
//! relational database's engine down and bringing it back up, driven over the
//! wire.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rules under test, asserted over a SET of databases — both engines this
//! service publishes blueprints for, several engine versions, standard and
//! high-availability bundles of different sizes, in four Regions — rather than
//! over one recorded call:
//!
//! * a stop shuts the engine down IN PLACE ("Stops a specific database that is
//!   currently running"): the database reports the stopping state while the
//!   clock owes it `stopped`, and every other member of the one stored record —
//!   the ARN, the support code, the creation time, the hardware, the master
//!   credentials, the windows and the master endpoint a client reconnects to
//!   above all — reads back identical, because a stop re-provisions nothing;
//! * a start brings that same engine back up ("Starts a specific database from a
//!   stopped state"): the database reports the starting state while the clock
//!   owes it `available`, and the record is again the one it had before the stop;
//! * each call writes exactly its own transition plus one log entry per resource
//!   it acted on, answered non-terminal against that resource and readable back
//!   by its id until the clock settles it;
//! * a parameter value the running engine has not loaded is loaded when the
//!   engine comes back up, so a stop/start cycle leaves the database `in-sync`
//!   with the value the update set unchanged;
//! * a stop that names `relationalDatabaseSnapshotName` takes a point-in-time
//!   copy of the database BEFORE stopping it, reported with the source's engine,
//!   version and disk size, and a snapshot name the account already holds is
//!   refused with the modelled `InvalidInputException` without stopping anything;
//! * each call acts from one state only — a start from `stopped`, a stop from
//!   `available` — and from any other state is refused with the modelled 400
//!   `OperationFailureException` naming the database and the state it is in,
//!   leaving the record exactly as it was; and
//! * a name the account holds no database for is refused with the service's
//!   `DoesNotExist` envelope, never a hollow 200.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::STATE_TRANSITION_MILLIS;

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// How long to wait for a clock-owed transition before giving up.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// The span the clock gives a transition it owes, from the service's own
/// constant.
///
/// A case that asserts the TRANSIENT state has to know whether its call landed
/// inside that span: measuring it is what keeps such a case exact on a machine
/// slow enough for the transition to arrive first.
fn transition_window() -> Duration {
    Duration::from_millis(u64::try_from(STATE_TRANSITION_MILLIS).unwrap_or_default())
}

/// One database to stop and start again.
///
/// The rows differ along what the rules generalise over: the engine, the engine
/// version, the bundle's size and whether it is a high-availability plan (so the
/// record has a standby zone to preserve), the Region and Availability Zone the
/// database lives in, and which parameter is left waiting for the engine to come
/// back up.
#[derive(Debug)]
struct Row {
    case: &'static str,
    blueprint_id: &'static str,
    bundle_id: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    /// A parameter of THIS row's engine, from the set
    /// `GetRelationalDatabaseParameters` publishes for it, and a value inside the
    /// range published beside it.
    parameter_name: &'static str,
    parameter_value: &'static str,
}

/// The databases every case here stops and starts.
const ROWS: &[Row] = &[
    Row {
        case: "mysql, smallest standard bundle",
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_1_0",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        parameter_name: "wait_timeout",
        parameter_value: "120",
    },
    Row {
        case: "postgres 17, large standard bundle",
        blueprint_id: "postgres_17",
        bundle_id: "large_1_0",
        availability_zone: "us-west-2b",
        region: "us-west-2",
        parameter_name: "shared_buffers",
        parameter_value: "16384",
    },
    Row {
        case: "postgres 13, micro high-availability bundle, so the record carries a standby zone",
        blueprint_id: "postgres_13",
        bundle_id: "micro_ha_1_0",
        availability_zone: "eu-west-3a",
        region: "eu-west-3",
        parameter_name: "max_connections",
        parameter_value: "100",
    },
    Row {
        case: "mysql, medium high-availability bundle",
        blueprint_id: "mysql_8_4",
        bundle_id: "medium_ha_1_0",
        availability_zone: "ap-northeast-1c",
        region: "ap-northeast-1",
        parameter_name: "max_allowed_packet",
        parameter_value: "2048",
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

/// `record` without its `member`, so two reads can be compared on everything
/// else.
fn without(record: &Value, member: &str) -> Value {
    let mut copy = record.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove(member);
    }
    copy
}

/// The entry of `operation_type` in `operations`, asserted to be the only one of
/// its type and a non-terminal entry against `resource_name`, answering its id.
fn assert_logged(
    case: &str,
    operations: &Value,
    operation_type: &str,
    resource_name: &str,
    resource_type: &str,
    region: &str,
) -> String {
    let entries = operations
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations list, got {operations:?}"));
    let matching: Vec<&Value> = entries
        .iter()
        .filter(|entry| entry["operationType"].as_str() == Some(operation_type))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case}: expected exactly one {operation_type} entry, got {operations:?}"
    );
    let operation = matching[0];
    assert_eq!(
        (
            operation["resourceName"].as_str(),
            operation["resourceType"].as_str()
        ),
        (Some(resource_name), Some(resource_type)),
        "case {case}: expected the entry to name {resource_name} ({resource_type}), \
         got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some(region),
        "case {case}: the entry is located where the resource is, got {operation:?}"
    );
    // The work is not finished when the call returns, so the entry is answered
    // non-terminal and the clock owes it its `Succeeded` status.
    assert_eq!(
        (
            operation["status"].as_str(),
            operation["isTerminal"].as_bool()
        ),
        (Some("Started"), Some(false)),
        "case {case}: expected a non-terminal Started entry, got {operation:?}"
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

/// The `parameterApplyStatus` the database `name` reports.
async fn apply_status(emulator: &Emulator, name: &str) -> String {
    database(emulator, name).await["parameterApplyStatus"]
        .as_str()
        .unwrap_or("<absent>")
        .to_string()
}

/// Every snapshot `GetRelationalDatabaseSnapshots` reports, walking every page.
async fn all_snapshots(emulator: &Emulator) -> Vec<Value> {
    let mut snapshots = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => {
                emulator
                    .ok("GetRelationalDatabaseSnapshots", json!({"pageToken": token}))
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
                panic!("case listing snapshots: expected a snapshots array, got {body:?}")
            })
            .clone();
        snapshots.extend(page);
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return snapshots,
        }
    }
}

// ---------------------------------------------------------------------------
// The stop and the start themselves
// ---------------------------------------------------------------------------

/// RULE: a stop shuts the database's engine down in place and a start brings that
/// same engine back up — the database reports `stopping` while the clock owes it
/// `stopped` and `starting` while the clock owes it `available`, and every other
/// member of the one stored record, the master endpoint a client reconnects to
/// above all, reads back identical throughout — while each call logs one
/// non-terminal entry of its own operation type against that database.
#[tokio::test]
async fn stopping_and_starting_a_database_move_only_its_state() {
    for row in ROWS {
        let name = "cycled-db";
        let emulator = with_available_database(row, name).await;
        let serving = database(&emulator, name).await;
        // The endpoint the rule is about must be there to be preserved: a
        // database that is serving announces one.
        assert!(
            serving["masterEndpoint"]["address"].is_string(),
            "case {}: an available database announces a master endpoint, got {serving:?}",
            row.case
        );

        // The stop.
        let started = Instant::now();
        let stopped = emulator
            .ok(
                "StopRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        let stop_id = assert_logged(
            row.case,
            &stopped["operations"],
            "StopRelationalDatabase",
            name,
            "RelationalDatabase",
            row.region,
        );
        assert_eq!(
            stopped["operations"].as_array().map(Vec::len),
            Some(1),
            "case {}: a stop that was asked for no snapshot acts on one database, \
             so it logs one operation, got {stopped:?}",
            row.case
        );

        // Shutting an engine down is not instantaneous, so a caller acting
        // straight after the call sees the database stopping — asserted when this
        // read really did land before the clock owed it `stopped`.
        let during_stop = database(&emulator, name).await;
        if started.elapsed() < transition_window() {
            assert_eq!(
                during_stop["state"].as_str(),
                Some("stopping"),
                "case {}: a database whose engine is being shut down reports stopping, \
                 got {during_stop:?}",
                row.case
            );
        }

        // The stop re-provisions nothing: everything except the state reads back
        // identical, the endpoint included — a stopped database keeps the address
        // a client reconnects to.
        let halted = wait_for_state(&emulator, name, "stopped").await;
        assert_eq!(
            without(&halted, "state"),
            without(&serving, "state"),
            "case {}: the stop must leave every other member of the record \
             untouched, got {halted:?}",
            row.case
        );
        // Spelled out for the members the rule is about, so a failure says which.
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
            "secondaryAvailabilityZone",
            "preferredBackupWindow",
            "preferredMaintenanceWindow",
            "caCertificateIdentifier",
            "location",
        ] {
            assert_eq!(
                halted[member], serving[member],
                "case {}: the stop must preserve {member}, expected {:?}, got {:?}",
                row.case, serving[member], halted[member]
            );
        }

        // The start.
        let started = Instant::now();
        let restarted = emulator
            .ok(
                "StartRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        let start_id = assert_logged(
            row.case,
            &restarted["operations"],
            "StartRelationalDatabase",
            name,
            "RelationalDatabase",
            row.region,
        );
        assert_ne!(
            stop_id, start_id,
            "case {}: each call logs its own operation, got {stop_id} twice",
            row.case
        );
        assert_eq!(
            restarted["operations"].as_array().map(Vec::len),
            Some(1),
            "case {}: a start acts on one database, so it logs one operation, \
             got {restarted:?}",
            row.case
        );

        let during_start = database(&emulator, name).await;
        if started.elapsed() < transition_window() {
            assert_eq!(
                during_start["state"].as_str(),
                Some("starting"),
                "case {}: a database whose engine is coming back up reports starting, \
                 got {during_start:?}",
                row.case
            );
        }

        // The engine comes back up without anyone starting it twice: the
        // transition is the clock's, not the poll's — and the record is the one
        // the database had while it was serving.
        let serving_again = wait_for_state(&emulator, name, "available").await;
        assert_eq!(
            serving_again, serving,
            "case {}: once the engine is back up the record is what it was, \
             got {serving_again:?}",
            row.case
        );

        // Each entry is readable by its id, and is the SAME record the call
        // answered with: one log, not two. Only its status moves (the clock owes
        // it `Succeeded`), so the identity members are what must match.
        for (logged, id, source) in [
            (&stopped, &stop_id, "the stop"),
            (&restarted, &start_id, "the start"),
        ] {
            let read_back = emulator
                .ok("GetOperation", json!({"operationId": id}))
                .await;
            for member in [
                "id",
                "operationType",
                "resourceName",
                "resourceType",
                "createdAt",
                "location",
            ] {
                assert_eq!(
                    read_back["operation"][member], logged["operations"][0][member],
                    "case {}: GetOperation must report the entry {source} logged, \
                     expected {member} {:?}, got {:?}",
                    row.case, logged["operations"][0][member], read_back["operation"][member]
                );
            }
        }

        // And the cycle is repeatable: the database stops and starts again.
        emulator
            .ok(
                "StopRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        wait_for_state(&emulator, name, "stopped").await;
        emulator
            .ok(
                "StartRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        let cycled = wait_for_state(&emulator, name, "available").await;
        assert_eq!(
            cycled, serving,
            "case {}: a second stop/start cycle must leave the record identical too, \
             got {cycled:?}",
            row.case
        );
    }
}

/// RULE: a start's log entry is answered non-terminal, and the clock owes it its
/// terminal `Succeeded` status — which the next read of that entry reports,
/// without anyone polling for it more than once.
#[tokio::test]
async fn the_clock_settles_the_operations_a_stop_and_a_start_log() {
    let row = &ROWS[1];
    let name = "settling-db";
    let emulator = with_available_database(row, name).await;

    let stopped = emulator
        .ok(
            "StopRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    let stop_id = assert_logged(
        "the entry a stop logs",
        &stopped["operations"],
        "StopRelationalDatabase",
        name,
        "RelationalDatabase",
        row.region,
    );
    wait_for_state(&emulator, name, "stopped").await;
    let restarted = emulator
        .ok(
            "StartRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    let start_id = assert_logged(
        "the entry a start logs",
        &restarted["operations"],
        "StartRelationalDatabase",
        name,
        "RelationalDatabase",
        row.region,
    );

    for (case, id) in [
        ("the entry a stop logs", &stop_id),
        ("the entry a start logs", &start_id),
    ] {
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        let settled = loop {
            let body = emulator.ok("GetOperation", json!({"operationId": id})).await;
            if body["operation"]["status"].as_str() == Some("Succeeded") {
                break body["operation"].clone();
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: the clock never settled it, last {body:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        assert_eq!(
            (settled["isTerminal"].as_bool(), settled["id"].as_str()),
            (Some(true), Some(id.as_str())),
            "case {case}: a settled entry is terminal and keeps its id, got {settled:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the restart loads
// ---------------------------------------------------------------------------

/// RULE: an engine reads its whole parameter group as it comes up, so a value the
/// running engine had not loaded — one applied `pending-reboot` — is loaded when
/// a stopped database is started again: the database is `pending-reboot` until it
/// is serving once more and `in-sync` afterwards, with the VALUE the update set
/// unchanged.
#[tokio::test]
async fn starting_a_stopped_database_loads_the_parameter_values_waiting_for_a_restart() {
    for row in ROWS {
        let name = "parameter-cycle-db";
        let emulator = with_available_database(row, name).await;
        emulator
            .ok(
                "UpdateRelationalDatabaseParameters",
                json!({
                    "relationalDatabaseName": name,
                    "parameters": [{
                        "parameterName": row.parameter_name,
                        "parameterValue": row.parameter_value,
                        "applyMethod": "pending-reboot",
                    }],
                }),
            )
            .await;
        assert_eq!(
            apply_status(&emulator, name).await,
            "pending-reboot",
            "case {}: a value the running engine has not loaded leaves the database \
             pending-reboot",
            row.case
        );

        emulator
            .ok(
                "StopRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        wait_for_state(&emulator, name, "stopped").await;
        // A stopped engine has loaded nothing: the value is still waiting.
        assert_eq!(
            apply_status(&emulator, name).await,
            "pending-reboot",
            "case {}: a stopped database has not loaded the value yet",
            row.case
        );

        emulator
            .ok(
                "StartRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        wait_for_state(&emulator, name, "available").await;
        assert_eq!(
            apply_status(&emulator, name).await,
            "in-sync",
            "case {}: once the engine is back up it has loaded the value, so the \
             database is in-sync",
            row.case
        );

        // The start LOADS the value, it does not lose it: the parameter group
        // still reports what the update set.
        let body = emulator
            .ok(
                "GetRelationalDatabaseParameters",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        let parameters = body["parameters"]
            .as_array()
            .unwrap_or_else(|| panic!("case {}: expected a parameters array, got {body:?}", row.case))
            .clone();
        let parameter = parameters
            .iter()
            .find(|parameter| parameter["parameterName"].as_str() == Some(row.parameter_name))
            .unwrap_or_else(|| {
                panic!(
                    "case {}: expected an entry for {}, got {parameters:?}",
                    row.case, row.parameter_name
                )
            });
        assert_eq!(
            parameter["parameterValue"].as_str(),
            Some(row.parameter_value),
            "case {}: expected the value the update set on {}, got {parameter:?}",
            row.case,
            row.parameter_name
        );
    }
}

// ---------------------------------------------------------------------------
// The pre-stop snapshot
// ---------------------------------------------------------------------------

/// RULE: `relationalDatabaseSnapshotName` is "the name of your new database
/// snapshot to be created before stopping your database", so a stop that names
/// one takes a point-in-time copy of the database first — reported with the
/// source's engine, version, disk size and identity — and logs that creation as
/// its own entry beside the stop's.
#[tokio::test]
async fn a_stop_takes_the_pre_stop_snapshot_it_was_asked_for() {
    for row in ROWS {
        let name = "snapshotted-db";
        let snapshot_name = "pre-stop-copy";
        let emulator = with_available_database(row, name).await;
        let serving = database(&emulator, name).await;
        assert!(
            all_snapshots(&emulator).await.is_empty(),
            "case {}: the account holds no snapshot before the stop",
            row.case
        );

        let stopped = emulator
            .ok(
                "StopRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "relationalDatabaseSnapshotName": snapshot_name,
                }),
            )
            .await;
        // One entry per resource the call acted on: the database it stopped and
        // the snapshot it created.
        assert_logged(
            row.case,
            &stopped["operations"],
            "StopRelationalDatabase",
            name,
            "RelationalDatabase",
            row.region,
        );
        assert_logged(
            row.case,
            &stopped["operations"],
            "CreateRelationalDatabaseSnapshot",
            snapshot_name,
            "RelationalDatabaseSnapshot",
            row.region,
        );

        // The copy is of the database as it was still serving, and it is readable
        // back through the listing.
        let snapshots = all_snapshots(&emulator).await;
        assert_eq!(
            snapshots.len(),
            1,
            "case {}: the stop took exactly one snapshot, got {snapshots:?}",
            row.case
        );
        let snapshot = &snapshots[0];
        assert_eq!(
            snapshot["name"].as_str(),
            Some(snapshot_name),
            "case {}: the snapshot carries the name the request gave it, got {snapshot:?}",
            row.case
        );
        for (member, source) in [
            ("engine", "engine"),
            ("engineVersion", "engineVersion"),
            ("fromRelationalDatabaseName", "name"),
            ("fromRelationalDatabaseArn", "arn"),
            ("fromRelationalDatabaseBundleId", "relationalDatabaseBundleId"),
            (
                "fromRelationalDatabaseBlueprintId",
                "relationalDatabaseBlueprintId",
            ),
        ] {
            assert_eq!(
                snapshot[member], serving[source],
                "case {}: expected the snapshot's {member} to be the source's {source} \
                 {:?}, got {:?}",
                row.case, serving[source], snapshot[member]
            );
        }
        assert_eq!(
            snapshot["sizeInGb"], serving["hardware"]["diskSizeInGb"],
            "case {}: expected the snapshot to be the size of the database's disk {:?}, \
             got {:?}",
            row.case, serving["hardware"]["diskSizeInGb"], snapshot["sizeInGb"]
        );
        assert_eq!(
            snapshot["location"], serving["location"],
            "case {}: a snapshot is located where the database it was taken of is, \
             got {snapshot:?}",
            row.case
        );

        // The stop still happened, and the database starts again afterwards.
        wait_for_state(&emulator, name, "stopped").await;
        emulator
            .ok(
                "StartRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        wait_for_state(&emulator, name, "available").await;
    }
}

/// RULE: a pre-stop snapshot name the account already holds is refused with the
/// modelled `InvalidInputException` (HTTP 400), and the refusal writes nothing:
/// the database is left serving and the snapshot that was already there is
/// untouched.
#[tokio::test]
async fn a_pre_stop_snapshot_name_already_taken_is_refused_and_stops_nothing() {
    let row = &ROWS[0];
    let name = "twice-snapshotted-db";
    let snapshot_name = "taken-already";
    let emulator = with_available_database(row, name).await;
    emulator
        .ok(
            "CreateRelationalDatabaseSnapshot",
            json!({
                "relationalDatabaseName": name,
                "relationalDatabaseSnapshotName": snapshot_name,
            }),
        )
        .await;
    let before = all_snapshots(&emulator).await;

    let (status, body) = emulator
        .call(
            "StopRelationalDatabase",
            json!({
                "relationalDatabaseName": name,
                "relationalDatabaseSnapshotName": snapshot_name,
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "InvalidInputException".to_string()),
        "case a pre-stop snapshot name already taken: expected the modelled \
         InvalidInputException, got {status} {body:?}"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains(snapshot_name),
        "case a pre-stop snapshot name already taken: the refusal must name the \
         snapshot, got {body:?}"
    );

    // Nothing was written: the database is still serving and there is still one
    // snapshot.
    assert_eq!(
        database(&emulator, name).await["state"].as_str(),
        Some("available"),
        "case a pre-stop snapshot name already taken: a refused stop must leave the \
         database serving"
    );
    assert_eq!(
        all_snapshots(&emulator).await,
        before,
        "case a pre-stop snapshot name already taken: a refused stop must not touch \
         the snapshot that was there"
    );

    // `relationalDatabaseName` names the resource the call is about, so a name the
    // account holds no database for is answered the primary resource's
    // `DoesNotExist` envelope even when the snapshot name is one that is taken.
    let (status, body) = emulator
        .call(
            "StopRelationalDatabase",
            json!({
                "relationalDatabaseName": "never-created",
                "relationalDatabaseSnapshotName": snapshot_name,
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body), body["code"].as_str()),
        (400, "NotFoundException".to_string(), Some("DoesNotExist")),
        "case an unknown database with a snapshot name already taken: expected the \
         database's DoesNotExist envelope, got {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: each call acts from one state only — a start from `stopped` ("Starts a
/// specific database from a stopped state. To restart a database, use the reboot
/// relational database operation") and a stop from `available` ("Stops a specific
/// database that is currently running") — so a call from any other state is
/// refused with the modelled `OperationFailureException` (HTTP 400) naming the
/// database and the state it is in, and the record is left exactly as it was.
#[tokio::test]
async fn a_start_or_a_stop_from_a_state_it_cannot_act_from_is_refused() {
    // A serving database cannot be started: it is already up, and a running
    // database is restarted with `RebootRelationalDatabase`.
    let row = &ROWS[0];
    let name = "already-serving";
    let emulator = with_available_database(row, name).await;
    let before = database(&emulator, name).await;
    let (status, body) = emulator
        .call(
            "StartRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "OperationFailureException".to_string()),
        "case starting an available database: expected the modelled \
         OperationFailureException, got {status} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(name) && message.contains("available"),
        "case starting an available database: the refusal must name the database and \
         the state that is in the way, got {body:?}"
    );
    assert_eq!(
        database(&emulator, name).await,
        before,
        "case starting an available database: a refused start must leave the record \
         exactly as it was"
    );

    // A stopped database cannot be stopped again.
    emulator
        .ok(
            "StopRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    let halted = wait_for_state(&emulator, name, "stopped").await;
    let (status, body) = emulator
        .call(
            "StopRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "OperationFailureException".to_string()),
        "case stopping a stopped database: expected the modelled \
         OperationFailureException, got {status} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(name) && message.contains("stopped"),
        "case stopping a stopped database: the refusal must name the database and the \
         state that is in the way, got {body:?}"
    );
    assert_eq!(
        database(&emulator, name).await,
        halted,
        "case stopping a stopped database: a refused stop must leave the record \
         exactly as it was"
    );

    // A database that is still being provisioned has no engine to stop, and none
    // to start either.
    let name = "still-creating";
    let emulator = Emulator::new();
    let started = Instant::now();
    emulator
        .ok("CreateRelationalDatabase", create_body(row, name))
        .await;
    let creating = database(&emulator, name).await;

    // The START is refused whatever the clock has done by now: a start acts from
    // `stopped`, and this database is either `creating` or — once the clock has
    // settled it — `available`. So this case asserts the refusal unconditionally,
    // and the state the message names is whichever of the two the call found.
    let (status, body) = emulator
        .call(
            "StartRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "OperationFailureException".to_string()),
        "case starting a database that is still being created: expected the modelled \
         OperationFailureException, got {status} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default().to_string();
    assert!(
        message.contains(name)
            && (message.contains("creating") || message.contains("available")),
        "case starting a database that is still being created: the refusal must name \
         the database and the state that is in the way, got {body:?}"
    );
    if started.elapsed() < transition_window() {
        assert_eq!(
            creating["state"].as_str(),
            Some("creating"),
            "case starting a database that is still being created: expected creating, \
             got {creating:?}"
        );
        assert!(
            message.contains("creating"),
            "case starting a database that is still being created: the refusal names \
             the state the call found, got {body:?}"
        );
    }

    // The STOP is refused while the database is still being provisioned, and
    // accepted once the clock has settled it into `available` — so this case
    // asserts one or the other, never nothing, deciding on the state the read
    // before the call reported together with whether the call landed inside the
    // transition window.
    let before_stop = database(&emulator, name).await;
    let (status, body) = emulator
        .call(
            "StopRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    if before_stop["state"].as_str() == Some("creating") && started.elapsed() < transition_window()
    {
        assert_eq!(
            (status, error_code(&body)),
            (400, "OperationFailureException".to_string()),
            "case stopping a database that is still being created: expected the \
             modelled OperationFailureException, got {status} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(name) && message.contains("creating"),
            "case stopping a database that is still being created: the refusal must \
             name the database and the state that is in the way, got {body:?}"
        );
    } else {
        assert_eq!(
            status, 200,
            "case stopping a database the clock had already settled: a stop of an \
             available database is accepted, got {status} {body:?}"
        );
        wait_for_state(&emulator, name, "stopped").await;
    }

    // A database whose engine is restarting is in no state to be stopped either:
    // the stop arrives while the reboot the first call started is still owed. Once
    // the clock has brought the engine back up the same stop is accepted, so this
    // case asserts one or the other.
    let name = "rebooting-db";
    let emulator = with_available_database(&ROWS[2], name).await;
    let started = Instant::now();
    emulator
        .ok(
            "RebootRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    let (status, body) = emulator
        .call(
            "StopRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    if started.elapsed() < transition_window() {
        assert_eq!(
            (status, error_code(&body)),
            (400, "OperationFailureException".to_string()),
            "case stopping a restarting database: expected the modelled \
             OperationFailureException, got {status} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(name) && message.contains("rebooting"),
            "case stopping a restarting database: the refusal must name the database \
             and the state that is in the way, got {body:?}"
        );
        // The restart the reboot started is still owed, and arrives.
        wait_for_state(&emulator, name, "available").await;
    } else {
        assert_eq!(
            status, 200,
            "case stopping a database whose restart had already finished: the stop is \
             accepted, got {status} {body:?}"
        );
        wait_for_state(&emulator, name, "stopped").await;
    }
}

/// RULE: both calls resolve `relationalDatabaseName` against the store, so a name
/// the account holds no database for is refused with the service's `DoesNotExist`
/// envelope and nothing is stored.
#[tokio::test]
async fn starting_or_stopping_an_unknown_database_answers_the_does_not_exist_envelope() {
    let emulator = Emulator::new();
    for operation in ["StartRelationalDatabase", "StopRelationalDatabase"] {
        for name in MISSING_NAMES {
            let (status, body) = emulator
                .call(operation, json!({"relationalDatabaseName": name}))
                .await;
            assert_eq!(
                status, 400,
                "case {operation} {name:?}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "NotFoundException",
                "case {operation} {name:?}: expected the wire error NotFoundException, \
                 got {body:?}"
            );
            assert_eq!(
                body["code"].as_str(),
                Some("DoesNotExist"),
                "case {operation} {name:?}: expected the body code DoesNotExist, got {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(format!("The RelationalDatabase does not exist: {name}").as_str()),
                "case {operation} {name:?}: expected the message to name the resource \
                 type and the requested name, got {body:?}"
            );
        }
    }

    // Refusing stored nothing: a read of any of those names is still the same
    // envelope, and the account holds no snapshot either.
    for name in MISSING_NAMES {
        let (status, body) = emulator
            .call(
                "GetRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "NotFoundException".to_string()),
            "case {name:?}: a refused start or stop must store nothing, got {status} {body:?}"
        );
    }
    assert!(
        all_snapshots(&emulator).await.is_empty(),
        "case a refused start or stop: no snapshot is created along the way"
    );
}

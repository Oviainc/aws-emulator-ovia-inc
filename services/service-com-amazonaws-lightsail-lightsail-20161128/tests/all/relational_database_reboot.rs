// SPDX-License-Identifier: CC0-1.0

//! `RebootRelationalDatabase` — restarting one managed relational database's
//! engine, driven over the wire.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.RebootRelationalDatabase` and an
//! `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed.
//!
//! The rules under test, asserted over a SET of databases — both engines this
//! service publishes blueprints for, several engine versions, standard and
//! high-availability bundles of different sizes, in four Regions — rather than
//! over one recorded call:
//!
//! * a reboot restarts the database's engine IN PLACE: the database reports the
//!   restarting state while the clock owes it its serving state back, and every
//!   other member of the one stored record — the ARN, the support code, the
//!   creation time, the hardware, the master credentials, the windows and the
//!   master endpoint a client connects to above all — reads back identical,
//!   because a restart re-provisions nothing;
//! * the call's writes are that transition, the restart it makes the engine load
//!   its parameter group with, and the one `RebootRelationalDatabase` entry it
//!   adds to the operation log, which is answered non-terminal against that
//!   database and is readable back by its id until the clock settles it;
//! * a parameter value the running engine has not loaded — one applied
//!   `pending-reboot` — is what the restart loads, so a database holding one is
//!   `pending-reboot` until its engine is back up and `in-sync` afterwards, with
//!   the VALUE it was set to unchanged;
//! * a database with no running engine to restart (one still being created, or
//!   one already restarting) is refused with the modelled 400
//!   `OperationFailureException` naming the database and the state it is in, and
//!   the record is left exactly as it was; and
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

/// One database to reboot, and the parameter value its restart has to load.
///
/// The rows differ along what the rule generalises over: the engine, the engine
/// version, the bundle's size and whether it is a high-availability plan (so the
/// record has a standby zone to preserve), the Region and Availability Zone the
/// database lives in, and which parameter is left waiting for the restart and how
/// it was held back.
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
    /// The `applyMethod` the update names, or `None` to let the parameter's own
    /// apply type decide — which for a parameter the engine reads only at
    /// startup is `pending-reboot`.
    apply_method: Option<&'static str>,
}

/// The databases every case here reboots.
const ROWS: &[Row] = &[
    Row {
        case: "mysql, smallest standard bundle, a dynamic parameter held back to the restart",
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_1_0",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        parameter_name: "wait_timeout",
        parameter_value: "120",
        apply_method: Some("pending-reboot"),
    },
    Row {
        case: "postgres 17, large standard bundle, a static parameter",
        blueprint_id: "postgres_17",
        bundle_id: "large_1_0",
        availability_zone: "us-west-2b",
        region: "us-west-2",
        parameter_name: "shared_buffers",
        parameter_value: "16384",
        apply_method: None,
    },
    Row {
        case: "postgres 13, micro high-availability bundle, so the record carries a standby zone",
        blueprint_id: "postgres_13",
        bundle_id: "micro_ha_1_0",
        availability_zone: "eu-west-3a",
        region: "eu-west-3",
        parameter_name: "max_connections",
        parameter_value: "100",
        apply_method: None,
    },
    Row {
        case: "mysql, medium high-availability bundle, another dynamic parameter held back",
        blueprint_id: "mysql_8_4",
        bundle_id: "medium_ha_1_0",
        availability_zone: "ap-northeast-1c",
        region: "ap-northeast-1",
        parameter_name: "max_allowed_packet",
        parameter_value: "2048",
        apply_method: Some("pending-reboot"),
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

/// The `parameterApplyStatus` the database `name` reports.
async fn apply_status(emulator: &Emulator, name: &str) -> String {
    database(emulator, name).await["parameterApplyStatus"]
        .as_str()
        .unwrap_or("<absent>")
        .to_string()
}

/// The parameter `parameter_name` of the database `name`, as
/// `GetRelationalDatabaseParameters` reports it.
async fn parameter_of(emulator: &Emulator, name: &str, parameter_name: &str) -> Value {
    let body = emulator
        .ok(
            "GetRelationalDatabaseParameters",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    let parameters = body["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading {parameter_name} of {name}: expected a parameters array, got {body:?}"))
        .clone();
    parameters
        .iter()
        .find(|parameter| parameter["parameterName"].as_str() == Some(parameter_name))
        .unwrap_or_else(|| {
            panic!("case reading {parameter_name} of {name}: expected an entry, got {parameters:?}")
        })
        .clone()
}

/// Assert `operations` is exactly one non-terminal `RebootRelationalDatabase` log
/// entry against `name`, answering its id.
fn assert_reboot_operation(case: &str, operations: &Value, name: &str, region: &str) -> String {
    let entries = operations
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations list, got {operations:?}"));
    assert_eq!(
        entries.len(),
        1,
        "case {case}: a reboot acts on one database, so it logs one operation, got {operations:?}"
    );
    let operation = &entries[0];
    assert_eq!(
        operation["operationType"].as_str(),
        Some("RebootRelationalDatabase"),
        "case {case}: expected operationType RebootRelationalDatabase, got {operation:?}"
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
    // The restart is not finished when the call returns, so the entry is answered
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

// ---------------------------------------------------------------------------
// The reboot itself
// ---------------------------------------------------------------------------

/// RULE: a reboot restarts the database's engine in place — the database reports
/// `rebooting` while the clock owes it `available` back, and every other member of
/// the one stored record, the master endpoint a client connects to above all,
/// reads back identical — while the call logs one non-terminal
/// `RebootRelationalDatabase` operation against that database.
#[tokio::test]
async fn rebooting_a_database_restarts_it_without_re_provisioning_anything() {
    for row in ROWS {
        let name = "rebooted-db";
        let emulator = with_available_database(row, name).await;
        let before = database(&emulator, name).await;
        // The endpoint the rule is about must be there to be preserved: a
        // database that is serving announces one.
        assert!(
            before["masterEndpoint"]["address"].is_string(),
            "case {}: an available database announces a master endpoint, got {before:?}",
            row.case
        );

        let started = Instant::now();
        let rebooted = emulator
            .ok(
                "RebootRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        let first_id = assert_reboot_operation(row.case, &rebooted["operations"], name, row.region);

        // The restart is not instantaneous, so a caller acting straight after the
        // call sees the database restarting — asserted when this read really did
        // land before the clock owed the database its serving state back.
        let during = database(&emulator, name).await;
        if started.elapsed() < transition_window() {
            assert_eq!(
                during["state"].as_str(),
                Some("rebooting"),
                "case {}: a database whose engine is restarting reports rebooting, got {during:?}",
                row.case
            );
        }
        // A restart re-provisions nothing: everything except the state reads back
        // identical, the endpoint included — a restarting database keeps the
        // address a client connects to.
        assert_eq!(
            without(&during, "state"),
            without(&before, "state"),
            "case {}: the reboot must leave every other member of the record \
             untouched, got {during:?}",
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
                during[member], before[member],
                "case {}: the reboot must preserve {member}, expected {:?}, got {:?}",
                row.case, before[member], during[member]
            );
        }

        // The engine comes back up without anyone starting it: the transition is
        // the clock's, not the poll's.
        let after = wait_for_state(&emulator, name, "available").await;
        assert_eq!(
            after, before,
            "case {}: once the engine is back up the record is what it was, got {after:?}",
            row.case
        );

        // The entry the reboot logged is readable by its id, and it is the SAME
        // record the reboot answered with: one log, not two. Only its status moves
        // (the clock owes it `Succeeded`), so the identity members are what must
        // match.
        let read_back = emulator
            .ok("GetOperation", json!({"operationId": first_id}))
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
                read_back["operation"][member], rebooted["operations"][0][member],
                "case {}: GetOperation must report the entry the reboot logged, \
                 expected {member} {:?}, got {:?}",
                row.case, rebooted["operations"][0][member], read_back["operation"][member]
            );
        }

        // Rebooting the database again is accepted the same way and logs a
        // DISTINCT entry.
        let again = emulator
            .ok(
                "RebootRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        let second_id = assert_reboot_operation(
            &format!("{}, rebooted twice", row.case),
            &again["operations"],
            name,
            row.region,
        );
        assert_ne!(
            first_id, second_id,
            "case {}: each reboot logs its own operation, got {first_id} twice",
            row.case
        );
        let twice = wait_for_state(&emulator, name, "available").await;
        assert_eq!(
            twice, before,
            "case {}: a second reboot must leave the record identical too, got {twice:?}",
            row.case
        );
    }
}

/// RULE: a reboot's log entry is answered non-terminal, and the clock owes it its
/// terminal `Succeeded` status — which the next read of that entry reports,
/// without anyone polling for it more than once.
#[tokio::test]
async fn the_clock_settles_the_reboot_it_logged() {
    let row = &ROWS[1];
    let name = "settling-db";
    let emulator = with_available_database(row, name).await;

    let rebooted = emulator
        .ok(
            "RebootRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    let id = assert_reboot_operation(
        "the entry a reboot logs",
        &rebooted["operations"],
        name,
        row.region,
    );

    let deadline = Instant::now() + SETTLE_TIMEOUT;
    let settled = loop {
        let body = emulator.ok("GetOperation", json!({"operationId": id})).await;
        if body["operation"]["status"].as_str() == Some("Succeeded") {
            break body["operation"].clone();
        }
        assert!(
            Instant::now() < deadline,
            "case the entry a reboot logs: the clock never settled it, last {body:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(
        (settled["isTerminal"].as_bool(), settled["id"].as_str()),
        (Some(true), Some(id.as_str())),
        "case the entry a reboot logs: a settled entry is terminal and keeps its id, \
         got {settled:?}"
    );
}

// ---------------------------------------------------------------------------
// What the restart loads
// ---------------------------------------------------------------------------

/// RULE: a parameter value the running engine has not loaded is loaded by the
/// restart — "Parameters marked with a pending-reboot apply type are applied only
/// after the database is rebooted using the reboot relational database operation"
/// — so a database holding one reports `parameterApplyStatus` `pending-reboot`
/// until its engine is back up and `in-sync` afterwards, while the VALUE the
/// update set, and the apply method it was set with, are reported unchanged
/// throughout.
#[tokio::test]
async fn a_reboot_loads_the_parameter_values_that_were_waiting_for_one() {
    for row in ROWS {
        let name = "parameter-reboot-db";
        let emulator = with_available_database(row, name).await;

        let mut entry = json!({
            "parameterName": row.parameter_name,
            "parameterValue": row.parameter_value,
        });
        if let Some(apply_method) = row.apply_method {
            entry["applyMethod"] = json!(apply_method);
        }
        emulator
            .ok(
                "UpdateRelationalDatabaseParameters",
                json!({"relationalDatabaseName": name, "parameters": [entry]}),
            )
            .await;

        // The update reaches the parameter group at once; what it leaves behind is
        // a running engine that has not read it.
        assert_eq!(
            apply_status(&emulator, name).await,
            "pending-reboot",
            "case {}: a value the running engine has not loaded leaves the database \
             pending-reboot",
            row.case
        );

        let started = Instant::now();
        emulator
            .ok(
                "RebootRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;

        // While the engine is restarting it still has not loaded the value: both
        // move when the engine is back up — never because something was read —
        // asserted when this read really did land inside the transition window.
        let during = database(&emulator, name).await;
        if started.elapsed() < transition_window() {
            assert_eq!(
                (
                    during["state"].as_str(),
                    during["parameterApplyStatus"].as_str()
                ),
                (Some("rebooting"), Some("pending-reboot")),
                "case {}: a restarting engine has not loaded the value yet, got {during:?}",
                row.case
            );
        }

        wait_for_state(&emulator, name, "available").await;
        assert_eq!(
            apply_status(&emulator, name).await,
            "in-sync",
            "case {}: once the engine is back up it has loaded the value, so the \
             database is in-sync",
            row.case
        );

        // The restart LOADS the value, it does not lose it: the parameter group
        // still reports what the update set, with the method it was set with.
        let parameter = parameter_of(&emulator, name, row.parameter_name).await;
        assert_eq!(
            parameter["parameterValue"].as_str(),
            Some(row.parameter_value),
            "case {}: expected the value the update set on {}, got {parameter:?}",
            row.case,
            row.parameter_name
        );
        assert_eq!(
            parameter["applyMethod"].as_str(),
            Some("pending-reboot"),
            "case {}: the apply method the value was set with is reported from then \
             on, got {parameter:?}",
            row.case
        );

        // And nothing is left waiting: a second reboot finds an engine that is
        // already in sync and leaves it there.
        emulator
            .ok(
                "RebootRelationalDatabase",
                json!({"relationalDatabaseName": name}),
            )
            .await;
        wait_for_state(&emulator, name, "available").await;
        assert_eq!(
            apply_status(&emulator, name).await,
            "in-sync",
            "case {}: a database with nothing waiting stays in-sync across a reboot",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: there has to be a running engine to restart, so a database that is still
/// being created — or one whose engine is already restarting — is refused with the
/// modelled `OperationFailureException` (HTTP 400) naming the database and the
/// state it is in, and the record is left exactly as it was.
#[tokio::test]
async fn rebooting_a_database_with_no_running_engine_is_refused() {
    // A database that is still being provisioned: the clock owes it `available`,
    // so a read straight after the create still reports `creating`.
    let row = &ROWS[0];
    let name = "still-creating";
    let emulator = Emulator::new();
    let started = Instant::now();
    emulator
        .ok("CreateRelationalDatabase", create_body(row, name))
        .await;
    let before = database(&emulator, name).await;

    let (status, body) = emulator
        .call(
            "RebootRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    // The refusal is what a call that arrived while the database was still being
    // provisioned gets; a call that arrived after the clock had already settled it
    // is a reboot of an available database, and is accepted.
    if started.elapsed() < transition_window() {
        assert_eq!(
            before["state"].as_str(),
            Some("creating"),
            "case a database that is still being created: expected creating, got {before:?}"
        );
        assert_eq!(
            (status, error_code(&body)),
            (400, "OperationFailureException".to_string()),
            "case a database that is still being created: expected the modelled \
             OperationFailureException, got {status} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(name) && message.contains("creating"),
            "case a database that is still being created: the refusal must name the \
             database and the state that is in the way, got {body:?}"
        );
    } else {
        assert_eq!(
            status, 200,
            "case a database the clock had already settled: a reboot of an available \
             database is accepted, got {status} {body:?}"
        );
    }

    // The refusal changed nothing: the database still settles into `available` on
    // its own, and the record is the one the create stored.
    let settled = wait_for_state(&emulator, name, "available").await;
    for member in ["arn", "supportCode", "createdAt", "hardware", "engine"] {
        assert_eq!(
            settled[member], before[member],
            "case a database that is still being created: a refused reboot must not \
             touch {member}, expected {:?}, got {:?}",
            before[member], settled[member]
        );
    }

    // A database whose engine is already restarting: the second call arrives while
    // the first restart is still owed.
    let name = "already-rebooting";
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
            "RebootRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await;
    if started.elapsed() < transition_window() {
        assert_eq!(
            (status, error_code(&body)),
            (400, "OperationFailureException".to_string()),
            "case a database that is already restarting: expected the modelled \
             OperationFailureException, got {status} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(name) && message.contains("rebooting"),
            "case a database that is already restarting: the refusal must name the \
             database and the state that is in the way, got {body:?}"
        );
    } else {
        assert_eq!(
            status, 200,
            "case a database whose first restart had already finished: the second \
             reboot is accepted, got {status} {body:?}"
        );
    }
    // The restart the first call started is still owed, and arrives.
    wait_for_state(&emulator, name, "available").await;
}

/// RULE: a reboot resolves `relationalDatabaseName` against the store, so a name
/// the account holds no database for is refused with the service's `DoesNotExist`
/// envelope and nothing is stored.
#[tokio::test]
async fn rebooting_an_unknown_database_answers_the_does_not_exist_envelope() {
    let emulator = Emulator::new();
    for name in MISSING_NAMES {
        let (status, body) = emulator
            .call(
                "RebootRelationalDatabase",
                json!({"relationalDatabaseName": name}),
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
            "case {name:?}: expected the message to name the resource type and the \
             requested name, got {body:?}"
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
            "case reading {name:?} after the refused reboot: expected the DoesNotExist \
             envelope, got {status} {body:?}"
        );
    }
}

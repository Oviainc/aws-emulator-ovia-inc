// SPDX-License-Identifier: CC0-1.0

//! `UpdateRelationalDatabaseParameters` — setting runtime parameters on one
//! managed database's parameter group, driven over the wire.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rules under test, asserted over a SET of engines, parameters, values and
//! apply methods rather than over one recorded call:
//!
//! * an entry of `parameters` sets that parameter of the database's own ENGINE
//!   on that one database, so the value is what every later
//!   `GetRelationalDatabaseParameters` reports for it, and the apply method it
//!   was set with is reported beside it — the method the request named, or the
//!   one the parameter's apply type implies when it named none (`immediate` for
//!   a parameter the running engine can take a change on, `pending-reboot` for
//!   one it reads only at startup);
//! * a value the running engine has not loaded puts the database in
//!   `parameterApplyStatus` `pending-reboot`, and a database whose every applied
//!   value is loaded stays `in-sync`;
//! * one call applies every entry it carries, in the order it carries them, and
//!   later calls accumulate on the same parameter group;
//! * a request the service cannot apply — a parameter the engine does not offer
//!   (another engine's included), one it reports unmodifiable, a value outside
//!   the parameter's data type or published value set, `immediate` on a parameter
//!   that can only change at a reboot, an apply method that is neither of the
//!   two, a missing name or value, an empty list, or more entries than can be
//!   modified in one request — is refused with the modelled 400
//!   `InvalidInputException` naming the entry and member at fault, and changes
//!   NOTHING; and
//! * a name that names no database is refused with the service's `DoesNotExist`
//!   envelope.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` has; the database create
//! and the paged parameter walk are the ones `relational_database_parameters.rs`
//! already has.

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};
use crate::relational_database_parameters::{create_database, walk};

/// The bundle every database here is created on: the cheapest standard plan.
/// Which plan a database runs on decides what its memory-sized parameters come
/// to, not which parameters it offers or which of them may be set.
const BUNDLE_ID: &str = "micro_1_0";

/// The apply method a parameter nothing has set reports, and the one an update
/// that names no `applyMethod` is applied with: the one the parameter's apply
/// type admits, since a change to a parameter the engine reads only at startup
/// can only land at the next reboot.
fn default_apply_method(apply_type: &str) -> &'static str {
    if apply_type == "dynamic" {
        "immediate"
    } else {
        "pending-reboot"
    }
}

/// Create the database `name` on `blueprint_id` and answer the emulator it lives
/// in.
async fn with_database(name: &str, blueprint_id: &str) -> Emulator {
    let emulator = Emulator::new();
    create_database(&emulator, name, blueprint_id, BUNDLE_ID).await;
    emulator
}

/// Every parameter `GetRelationalDatabaseParameters` reports for `name`, walking
/// every page.
async fn parameters_of(emulator: &Emulator, name: &str) -> Vec<Value> {
    walk(emulator, json!({"relationalDatabaseName": name}))
        .await
        .0
}

/// The reported parameter `parameter_name` of the database `name`.
async fn parameter_of(emulator: &Emulator, name: &str, parameter_name: &str) -> Value {
    let parameters = parameters_of(emulator, name).await;
    parameters
        .iter()
        .find(|parameter| parameter["parameterName"].as_str() == Some(parameter_name))
        .unwrap_or_else(|| {
            panic!("case reading {parameter_name} of {name}: expected an entry, got {parameters:?}")
        })
        .clone()
}

/// The `parameterApplyStatus` the database `name` reports.
async fn apply_status(emulator: &Emulator, name: &str) -> String {
    emulator
        .ok(
            "GetRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await["relationalDatabase"]["parameterApplyStatus"]
        .as_str()
        .unwrap_or("<absent>")
        .to_string()
}

/// Call `UpdateRelationalDatabaseParameters` on `name` with `parameters`.
async fn update(emulator: &Emulator, name: &str, parameters: Value) -> (u16, Value) {
    emulator
        .call(
            "UpdateRelationalDatabaseParameters",
            json!({"relationalDatabaseName": name, "parameters": parameters}),
        )
        .await
}

/// One `parameters` entry naming `parameter_name` and setting `value`, with
/// `apply_method` when one is given.
fn entry(parameter_name: &str, value: &str, apply_method: Option<&str>) -> Value {
    let mut entry = json!({"parameterName": parameter_name, "parameterValue": value});
    if let Some(method) = apply_method {
        entry["applyMethod"] = json!(method);
    }
    entry
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: an applied parameter is what every later read reports — the value, and
/// the apply method the request named or the one its apply type implies — and a
/// value the running engine has not loaded is what moves the database's
/// `parameterApplyStatus` to `pending-reboot`.
#[tokio::test]
async fn an_applied_parameter_is_read_back_with_its_apply_method() {
    /// One update the service must apply. The rows differ along the engine, the
    /// parameter's apply type and data type, the value (each published bound
    /// among them, and a fractional one on the float parameter) and which of the
    /// three forms `applyMethod` takes.
    struct Row {
        case: &'static str,
        blueprint_id: &'static str,
        parameter_name: &'static str,
        value: &'static str,
        requested_method: Option<&'static str>,
        want_method: &'static str,
        want_status: &'static str,
    }

    let rows = [
        Row {
            case: "a dynamic flag with no apply method",
            blueprint_id: "mysql_8_4",
            parameter_name: "autocommit",
            value: "0",
            requested_method: None,
            want_method: "immediate",
            want_status: "in-sync",
        },
        Row {
            case: "a dynamic flag applied immediately",
            blueprint_id: "mysql_8_4",
            parameter_name: "slow_query_log",
            value: "1",
            requested_method: Some("immediate"),
            want_method: "immediate",
            want_status: "in-sync",
        },
        Row {
            case: "a dynamic parameter held back to the next reboot",
            blueprint_id: "mysql_8_4",
            parameter_name: "max_connections",
            value: "500",
            requested_method: Some("pending-reboot"),
            want_method: "pending-reboot",
            want_status: "pending-reboot",
        },
        Row {
            case: "an integer at its published low bound",
            blueprint_id: "mysql_8_4",
            parameter_name: "max_connections",
            value: "1",
            requested_method: None,
            want_method: "immediate",
            want_status: "in-sync",
        },
        Row {
            case: "an integer at its published high bound",
            blueprint_id: "mysql_8_4",
            parameter_name: "max_connections",
            value: "100000",
            requested_method: None,
            want_method: "immediate",
            want_status: "in-sync",
        },
        Row {
            case: "an integer at another parameter's high bound",
            blueprint_id: "mysql_8_4",
            parameter_name: "auto_increment_increment",
            value: "65535",
            requested_method: None,
            want_method: "immediate",
            want_status: "in-sync",
        },
        Row {
            case: "a fractional value on a float parameter",
            blueprint_id: "mysql_8_4",
            parameter_name: "long_query_time",
            value: "0.5",
            requested_method: None,
            want_method: "immediate",
            want_status: "in-sync",
        },
        Row {
            case: "a free-text parameter with no published value set",
            blueprint_id: "mysql_8_4",
            parameter_name: "time_zone",
            value: "Europe/Berlin",
            requested_method: None,
            want_method: "immediate",
            want_status: "in-sync",
        },
        Row {
            case: "a static parameter with no apply method",
            blueprint_id: "postgres_17",
            parameter_name: "max_connections",
            value: "500",
            requested_method: None,
            want_method: "pending-reboot",
            want_status: "pending-reboot",
        },
        Row {
            case: "a static integer at its published low bound",
            blueprint_id: "postgres_17",
            parameter_name: "shared_buffers",
            value: "16",
            requested_method: None,
            want_method: "pending-reboot",
            want_status: "pending-reboot",
        },
        Row {
            case: "a static parameter on an older major version",
            blueprint_id: "postgres_13",
            parameter_name: "max_connections",
            value: "262143",
            requested_method: Some("pending-reboot"),
            want_method: "pending-reboot",
            want_status: "pending-reboot",
        },
        Row {
            case: "a negative value a published range admits",
            blueprint_id: "postgres_17",
            parameter_name: "log_min_duration_statement",
            value: "-1",
            requested_method: Some("immediate"),
            want_method: "immediate",
            want_status: "in-sync",
        },
        Row {
            case: "a dynamic integer on the other engine",
            blueprint_id: "postgres_17",
            parameter_name: "work_mem",
            value: "65536",
            requested_method: None,
            want_method: "immediate",
            want_status: "in-sync",
        },
        Row {
            case: "a free-text parameter on the other engine",
            blueprint_id: "postgres_17",
            parameter_name: "timezone",
            value: "America/New_York",
            requested_method: None,
            want_method: "immediate",
            want_status: "in-sync",
        },
    ];

    for row in rows {
        let name = "applied-db";
        let emulator = with_database(name, row.blueprint_id).await;
        let before = parameters_of(&emulator, name).await;
        assert_eq!(
            apply_status(&emulator, name).await,
            "in-sync",
            "case {}: a database nothing has set a parameter on is in-sync",
            row.case
        );

        let (status, body) = update(
            &emulator,
            name,
            json!([entry(row.parameter_name, row.value, row.requested_method)]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: the update must succeed, got {body:?}",
            row.case
        );

        // The call answers exactly one operation, and the parameter group holds
        // the new value by the time it does — so that operation is terminal.
        let operations = body["operations"].as_array().unwrap_or_else(|| {
            panic!(
                "case {}: expected an operations array, got {body:?}",
                row.case
            )
        });
        assert_eq!(
            operations.len(),
            1,
            "case {}: expected one operation, got {body:?}",
            row.case
        );
        let operation = &operations[0];
        assert!(
            operation["id"].as_str().is_some_and(is_uuid_shaped),
            "case {}: expected a uuid-shaped operation id, got {operation:?}",
            row.case
        );
        assert_eq!(
            (
                operation["operationType"].as_str(),
                operation["resourceType"].as_str(),
                operation["resourceName"].as_str(),
                operation["status"].as_str(),
                operation["isTerminal"].as_bool(),
            ),
            (
                Some("UpdateRelationalDatabaseParameters"),
                Some("RelationalDatabase"),
                Some(name),
                Some("Succeeded"),
                Some(true),
            ),
            "case {}: expected the modelled log entry for this call, got {operation:?}",
            row.case
        );

        let applied = parameter_of(&emulator, name, row.parameter_name).await;
        assert_eq!(
            applied["parameterValue"].as_str(),
            Some(row.value),
            "case {}: expected the applied value, got {applied:?}",
            row.case
        );
        assert_eq!(
            applied["applyMethod"].as_str(),
            Some(row.want_method),
            "case {}: expected applyMethod {}, got {applied:?}",
            row.case,
            row.want_method
        );
        // Setting a value changes neither what the engine publishes about the
        // parameter nor how it is classified.
        for member in ["applyType", "dataType", "description", "allowedValues"] {
            let before = before
                .iter()
                .find(|parameter| {
                    parameter["parameterName"].as_str() == Some(row.parameter_name)
                })
                .unwrap_or_else(|| {
                    panic!(
                        "case {}: expected {} in the published set, got {before:?}",
                        row.case, row.parameter_name
                    )
                });
            assert_eq!(
                applied[member], before[member],
                "case {}: expected {member} untouched by the update, got {applied:?}",
                row.case
            );
        }
        assert_eq!(
            apply_status(&emulator, name).await,
            row.want_status,
            "case {}: expected parameterApplyStatus {}",
            row.case,
            row.want_status
        );

        // Only the named parameter moved: every other entry still reports
        // exactly what it did before the call, in the same order.
        let after = parameters_of(&emulator, name).await;
        assert_eq!(
            after.len(),
            before.len(),
            "case {}: an update adds no parameter and drops none, got {after:?}",
            row.case
        );
        for (before, after) in before.iter().zip(after.iter()) {
            if before["parameterName"].as_str() == Some(row.parameter_name) {
                continue;
            }
            assert_eq!(
                before, after,
                "case {}: expected every other parameter untouched, got {after:?}",
                row.case
            );
        }

        // Reading is a read: a second walk answers the same parameters.
        let again = parameters_of(&emulator, name).await;
        assert_eq!(
            again, after,
            "case {}: two reads must answer the same parameters, got {again:?}",
            row.case
        );
    }
}

/// RULE: a parameter nothing has set reports the apply method its apply type
/// implies, and setting one is what changes that method — asserted over every
/// parameter of both engines rather than over the rows above.
#[tokio::test]
async fn an_unset_parameter_reports_the_method_its_apply_type_implies() {
    for blueprint_id in ["mysql_8_4", "postgres_17"] {
        let name = "unset-db";
        let emulator = with_database(name, blueprint_id).await;
        for parameter in parameters_of(&emulator, name).await {
            let apply_type = parameter["applyType"].as_str().unwrap_or("<absent>");
            assert_eq!(
                parameter["applyMethod"].as_str(),
                Some(default_apply_method(apply_type)),
                "case {blueprint_id}/{:?}: an unset parameter reports the method its applyType \
                 admits, got {parameter:?}",
                parameter["parameterName"]
            );
        }
    }
}

/// RULE: one call applies every entry it carries, in the order it carries them,
/// and later calls accumulate on the same parameter group — so the last value
/// written to a parameter is the one a read reports.
#[tokio::test]
async fn updates_apply_in_order_and_accumulate() {
    let name = "accumulate-db";
    let emulator = with_database(name, "postgres_17").await;

    // Three entries in one call, over both apply types.
    let (status, body) = update(
        &emulator,
        name,
        json!([
            entry("autovacuum", "0", None),
            entry("work_mem", "8192", None),
            entry("shared_buffers", "32768", None),
        ]),
    )
    .await;
    assert_eq!(status, 200, "case one call, three entries: got {body:?}");
    for (parameter_name, want_value, want_method) in [
        ("autovacuum", "0", "immediate"),
        ("work_mem", "8192", "immediate"),
        ("shared_buffers", "32768", "pending-reboot"),
    ] {
        let applied = parameter_of(&emulator, name, parameter_name).await;
        assert_eq!(
            (
                applied["parameterValue"].as_str(),
                applied["applyMethod"].as_str()
            ),
            (Some(want_value), Some(want_method)),
            "case one call, three entries: expected {parameter_name} = {want_value} \
             ({want_method}), got {applied:?}"
        );
    }
    assert_eq!(
        apply_status(&emulator, name).await,
        "pending-reboot",
        "case one call, three entries: one value waits for a reboot"
    );

    // A later call changes only what it names, and the parameter it names takes
    // the new value.
    let (status, body) = update(&emulator, name, json!([entry("work_mem", "16384", None)])).await;
    assert_eq!(status, 200, "case a later call: got {body:?}");
    let applied = parameter_of(&emulator, name, "work_mem").await;
    assert_eq!(
        applied["parameterValue"].as_str(),
        Some("16384"),
        "case a later call: expected the newest value, got {applied:?}"
    );
    let untouched = parameter_of(&emulator, name, "autovacuum").await;
    assert_eq!(
        untouched["parameterValue"].as_str(),
        Some("0"),
        "case a later call: expected the earlier call's value to stand, got {untouched:?}"
    );

    // Two entries for ONE parameter in one call: the list is applied in order,
    // so the last entry is the one that stands.
    let (status, body) = update(
        &emulator,
        name,
        json!([
            entry("work_mem", "32768", None),
            entry("work_mem", "65536", None),
        ]),
    )
    .await;
    assert_eq!(status, 200, "case one parameter twice: got {body:?}");
    let applied = parameter_of(&emulator, name, "work_mem").await;
    assert_eq!(
        applied["parameterValue"].as_str(),
        Some("65536"),
        "case one parameter twice: expected the last entry to stand, got {applied:?}"
    );
}

/// RULE: a request the service cannot apply is refused with the modelled 400
/// `InvalidInputException`, whose message names the entry and the member that are
/// wrong — and it changes nothing: every parameter still reports what it did and
/// the database is still `in-sync`.
#[tokio::test]
async fn a_request_the_service_cannot_apply_is_refused_and_changes_nothing() {
    /// One refusal case: the blueprint to create the database on, the
    /// `parameters` list to send, and the text the refusal must name.
    struct Case {
        case: &'static str,
        blueprint_id: &'static str,
        parameters: Value,
        names: &'static str,
    }

    let cases = vec![
        Case {
            case: "a parameter the engine does not offer",
            blueprint_id: "mysql_8_4",
            parameters: json!([entry("no_such_parameter", "1", None)]),
            names: "parameters[0].parameterName",
        },
        Case {
            case: "a parameter of the OTHER engine",
            blueprint_id: "mysql_8_4",
            parameters: json!([entry("shared_buffers", "16384", None)]),
            names: "parameters[0].parameterName",
        },
        Case {
            case: "a MySQL parameter on a PostgreSQL database",
            blueprint_id: "postgres_17",
            parameters: json!([entry("max_allowed_packet", "16384", None)]),
            names: "parameters[0].parameterName",
        },
        Case {
            case: "a parameter the engine reports unmodifiable",
            blueprint_id: "mysql_8_4",
            parameters: json!([entry("allow-suspicious-udfs", "1", None)]),
            names: "parameters[0].parameterName",
        },
        Case {
            case: "immediate on a parameter the engine reads only at startup",
            blueprint_id: "postgres_17",
            parameters: json!([entry("max_connections", "500", Some("immediate"))]),
            names: "parameters[0].applyMethod",
        },
        Case {
            case: "an apply method that is neither of the two",
            blueprint_id: "mysql_8_4",
            parameters: json!([entry("autocommit", "0", Some("whenever"))]),
            names: "parameters[0].applyMethod",
        },
        Case {
            case: "a flag value outside its published value set",
            blueprint_id: "mysql_8_4",
            parameters: json!([entry("autocommit", "2", None)]),
            names: "parameters[0].parameterValue",
        },
        Case {
            case: "a non-numeric value on an integer parameter",
            blueprint_id: "mysql_8_4",
            parameters: json!([entry("max_connections", "many", None)]),
            names: "parameters[0].parameterValue",
        },
        Case {
            case: "a fractional value on an integer parameter",
            blueprint_id: "mysql_8_4",
            parameters: json!([entry("max_connections", "12.5", None)]),
            names: "parameters[0].parameterValue",
        },
        Case {
            case: "a non-numeric value on a float parameter",
            blueprint_id: "mysql_8_4",
            parameters: json!([entry("long_query_time", "slow", None)]),
            names: "parameters[0].parameterValue",
        },
        Case {
            case: "an integer one below its published low bound",
            blueprint_id: "mysql_8_4",
            parameters: json!([entry("max_connections", "0", None)]),
            names: "parameters[0].parameterValue",
        },
        Case {
            case: "an integer one above its published high bound",
            blueprint_id: "mysql_8_4",
            parameters: json!([entry("max_connections", "100001", None)]),
            names: "parameters[0].parameterValue",
        },
        Case {
            case: "an integer one below a published negative low bound",
            blueprint_id: "postgres_17",
            parameters: json!([entry("log_min_duration_statement", "-2", None)]),
            names: "parameters[0].parameterValue",
        },
        Case {
            case: "an integer one above a published high bound on the other engine",
            blueprint_id: "postgres_17",
            parameters: json!([entry("max_connections", "262144", None)]),
            names: "parameters[0].parameterValue",
        },
        Case {
            case: "an empty value",
            blueprint_id: "postgres_17",
            parameters: json!([entry("timezone", "", None)]),
            names: "parameters[0].parameterValue",
        },
        Case {
            case: "an entry naming no parameter",
            blueprint_id: "mysql_8_4",
            parameters: json!([{"parameterValue": "0"}]),
            names: "parameters[0].parameterName",
        },
        Case {
            case: "an entry carrying no value",
            blueprint_id: "mysql_8_4",
            parameters: json!([{"parameterName": "autocommit"}]),
            names: "parameters[0].parameterValue",
        },
        Case {
            case: "a bad entry after a good one",
            blueprint_id: "mysql_8_4",
            parameters: json!([
                entry("autocommit", "0", None),
                entry("max_connections", "0", None),
            ]),
            names: "parameters[1].parameterValue",
        },
        Case {
            case: "an empty list",
            blueprint_id: "mysql_8_4",
            parameters: json!([]),
            names: "parameters",
        },
        Case {
            case: "more entries than can be modified in one request",
            blueprint_id: "mysql_8_4",
            parameters: Value::Array((0..21).map(|_| entry("autocommit", "0", None)).collect()),
            names: "parameters",
        },
    ];

    for case in cases {
        let name = "refused-db";
        let emulator = with_database(name, case.blueprint_id).await;
        let before = parameters_of(&emulator, name).await;

        let (status, body) = update(&emulator, name, case.parameters.clone()).await;
        assert_eq!(
            status, 400,
            "case {}: expected a 400 refusal, got {status} {body:?}",
            case.case
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {}: expected the modelled InvalidInputException, got {body:?}",
            case.case
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(case.names),
            "case {}: expected the refusal to name {}, got {body:?}",
            case.case,
            case.names
        );

        let after = parameters_of(&emulator, name).await;
        assert_eq!(
            after, before,
            "case {}: a refused update changes nothing, got {after:?}",
            case.case
        );
        assert_eq!(
            apply_status(&emulator, name).await,
            "in-sync",
            "case {}: a refused update leaves the database in-sync",
            case.case
        );
    }
}

/// RULE: a well-formed name that names no stored database is refused with the
/// service's `DoesNotExist` envelope — HTTP 400, `NotFoundException`, body code
/// `DoesNotExist` and a message naming the resource type and the requested name —
/// never a 501 and never a hollow 200.
#[tokio::test]
async fn parameters_of_an_unknown_database_cannot_be_updated() {
    let name = "present-db";
    let emulator = with_database(name, "mysql_8_4").await;

    for unknown in ["absent-db", "Present-db", "present_db"] {
        let (status, body) = update(&emulator, unknown, json!([entry("autocommit", "0", None)])).await;
        assert_eq!(
            status, 400,
            "case {unknown}: expected the 400 DoesNotExist envelope, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {unknown}: expected NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {unknown}: expected the DoesNotExist code, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The RelationalDatabase does not exist: {unknown}").as_str()),
            "case {unknown}: expected the resource-type/name message, got {body:?}"
        );
    }

    // The database that DOES exist was left alone by every refusal above.
    assert_eq!(
        apply_status(&emulator, name).await,
        "in-sync",
        "case a refused update on another name: the stored database is untouched"
    );
}

/// RULE: a parameter group belongs to ONE database, so setting a parameter on one
/// leaves every other database of the same engine reporting what it did.
#[tokio::test]
async fn a_parameter_group_belongs_to_one_database() {
    let emulator = Emulator::new();
    for name in ["group-one", "group-two"] {
        create_database(&emulator, name, "mysql_8_4", BUNDLE_ID).await;
    }

    let before = parameters_of(&emulator, "group-two").await;
    let (status, body) = update(
        &emulator,
        "group-one",
        json!([entry("wait_timeout", "120", None)]),
    )
    .await;
    assert_eq!(status, 200, "case two databases: got {body:?}");

    let applied = parameter_of(&emulator, "group-one", "wait_timeout").await;
    assert_eq!(
        applied["parameterValue"].as_str(),
        Some("120"),
        "case two databases: expected the value on the database it was set on, got {applied:?}"
    );
    let other = parameters_of(&emulator, "group-two").await;
    assert_eq!(
        other, before,
        "case two databases: expected the other database untouched, got {other:?}"
    );
    assert_eq!(
        apply_status(&emulator, "group-two").await,
        "in-sync",
        "case two databases: the other database is still in-sync"
    );
}

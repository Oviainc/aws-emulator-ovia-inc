// SPDX-License-Identifier: CC0-1.0

//! The per-table column cap and what it is graded BEFORE, driven end to end over the
//! service's own `awsJson1_0` wire protocol through the shared
//! [`crate::keyspace_table_lifecycle::Emulator`] harness, so what is pinned is the
//! emulator's observable behaviour.
//!
//! The rule under test, exercised over a SET of inputs rather than one example: a refusal
//! that depends on the REQUEST ALONE — here, how many columns the request would leave a
//! table's schema holding, against the per-table column cap — is decided before any
//! per-column scan over that same list and before every refusal that depends on what the
//! store holds. So an oversized column list answers the modelled
//! `ServiceQuotaExceededException` whatever ELSE is wrong with the columns in it or
//! with the resources the request names, and the two operations that carry a column list,
//! `CreateTable` and `UpdateTable`, grade the same pair of faults in the same order.
//!
//! Its consequences, also pinned here: a request within the cap still gets the error its
//! own fault earns (the cap refusal orders those faults, it does not swallow them), and a
//! refused request stores nothing.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, error_code, minimal_schema, settle_wait, Emulator,
};

/// The per-table column cap this suite is about. The number is the emulator's own bound
/// (`service_..._keyspacesservice::state::MAX_COLUMNS_PER_TABLE`), not a modelled one:
/// `SchemaDefinition$allColumns` and `UpdateTableRequest$addColumns` are both modelled
/// `len 1..` with NO maximum.
const CAP: usize = keyspaces::MAX_COLUMNS_PER_TABLE;

/// The modelled quota refusal an oversized column list earns, on the client-error status
/// every refusal of this RPC service travels with
/// (`service_..._keyspacesservice::scaffolded::CLIENT_ERROR_STATUS`).
const QUOTA: (u16, &str) = (400, "ServiceQuotaExceededException");

/// The resulting column counts every case is run at: a small list, the cap exactly, one
/// past the cap, and well past it.
fn totals() -> [usize; 4] {
    [4, CAP, CAP + 1, CAP + 8]
}

/// `count` well-formed `int` columns, named apart from the partition key.
fn int_columns(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| json!({"name": format!("c{index}"), "type": "int"}))
        .collect()
}

/// A second fault planted in the column list a request carries, beside its size.
struct Fault {
    /// The case name, which also names the table each row uses.
    name: &'static str,
    /// Plant the fault in the request's generated columns (at least two of them).
    plant: fn(&mut [Value]),
    /// The status and error code this fault earns on its OWN, i.e. once the list is
    /// within the cap; `None` when the list is well-formed and the request is accepted.
    alone: Option<(u16, &'static str)>,
}

/// The rows: the faults a column list can carry alongside being oversized, each with the
/// error it answers on its own. The type rows span where in the list the bad type sits
/// and whether it is named bare or inside a type constructor.
fn faults() -> Vec<Fault> {
    vec![
        Fault {
            name: "well_formed",
            plant: |_| {},
            alone: None,
        },
        Fault {
            name: "unresolvable_type_first",
            plant: |columns| columns[0]["type"] = json!("nosuchtype"),
            alone: Some((400, "ResourceNotFoundException")),
        },
        Fault {
            name: "unresolvable_type_last",
            plant: |columns| {
                let last = columns.len() - 1;
                columns[last]["type"] = json!("nosuchtype");
            },
            alone: Some((400, "ResourceNotFoundException")),
        },
        Fault {
            name: "unresolvable_type_in_constructor",
            plant: |columns| columns[0]["type"] = json!("frozen<nosuchtype>"),
            alone: Some((400, "ResourceNotFoundException")),
        },
        Fault {
            name: "duplicate_column_name",
            plant: |columns| {
                let repeated = columns[0]["name"].clone();
                columns[1]["name"] = repeated;
            },
            alone: Some((400, "ValidationException")),
        },
    ]
}

/// What a request leaving the schema with `total` columns and carrying `fault` must
/// answer: the quota past the cap, whatever the fault, and otherwise the fault's own
/// error.
fn expected(total: usize, fault: &Fault) -> Option<(u16, &'static str)> {
    match total > CAP {
        true => Some(QUOTA),
        false => fault.alone,
    }
}

/// The `message` member of a failure body.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message member>")
        .to_string()
}

/// Assert a refusal is the COLUMN quota and not another of this service's quotas, which
/// share its error code: the message names the cap it is about.
fn assert_names_the_column_cap(case: &str, body: &Value) {
    let message = message(body);
    assert!(
        message.contains(&CAP.to_string()) && message.contains("column"),
        "case {case:?}: the quota refusal must name the column cap ({CAP}), got {message:?}"
    );
}

/// `GetTable` for one table, as a status and body.
async fn get_table(emulator: &Emulator, keyspace: &str, table: &str) -> (u16, Value) {
    emulator
        .call(
            "GetTable",
            json!({"keyspaceName": keyspace, "tableName": table}),
        )
        .await
}

/// How many columns `GetTable` reports for a table.
fn reported_column_count(body: &Value) -> usize {
    body.pointer("/schemaDefinition/allColumns")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

#[tokio::test]
async fn an_oversized_column_list_is_the_quota_error_whatever_else_is_wrong_with_it() {
    let emulator = Emulator::start();
    let keyspace = "col_cap_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );

    // `CreateTable`: `allColumns` carries the partition key plus the generated columns,
    // so a request leaving the schema with `total` columns generates `total - 1`.
    for fault in faults() {
        for total in totals() {
            let case = format!("CreateTable {} at {total} columns", fault.name);
            let table = format!("create_{}_{total}", fault.name);
            let mut generated = int_columns(total - 1);
            (fault.plant)(&mut generated);
            let mut all_columns = vec![json!({"name": "pk", "type": "text"})];
            all_columns.extend(generated);
            let got = emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": table,
                        "schemaDefinition": {
                            "allColumns": all_columns,
                            "partitionKeys": [{"name": "pk"}],
                        },
                    }),
                )
                .await;

            match expected(total, &fault) {
                None => {
                    assert_ok(&case, got);
                    let (status, body) = get_table(&emulator, keyspace, &table).await;
                    let reported = reported_column_count(&body);
                    assert_eq!(
                        (status, reported),
                        (200, total),
                        "case {case:?}: the created table must report its {total} columns, \
                         got {status} with {reported}"
                    );
                }
                Some((status, code)) => {
                    let body = got.1.clone();
                    assert_failure(&case, got, status, code);
                    if code == QUOTA.1 {
                        assert_names_the_column_cap(&case, &body);
                    }
                    let (read_status, read_body) = get_table(&emulator, keyspace, &table).await;
                    assert_eq!(
                        (read_status, error_code(&read_body).as_str()),
                        (400, "ResourceNotFoundException"),
                        "case {case:?}: a refused create must store no table, got \
                         {read_status} {read_body}"
                    );
                }
            }
        }
    }

    // `UpdateTable`: the same pair of faults, on tables that already hold the two columns
    // of the default schema, so a request leaving the schema with `total` columns adds
    // `total - 2`. Every target table is created first and settled once, because a table
    // can only be updated from `ACTIVE`.
    for fault in faults() {
        for total in totals() {
            let table = format!("update_{}_{total}", fault.name);
            assert_ok(
                &table,
                emulator
                    .call(
                        "CreateTable",
                        json!({
                            "keyspaceName": keyspace,
                            "tableName": table,
                            "schemaDefinition": minimal_schema("pk"),
                        }),
                    )
                    .await,
            );
        }
    }
    tokio::time::sleep(settle_wait()).await;

    for fault in faults() {
        for total in totals() {
            let table = format!("update_{}_{total}", fault.name);
            let case = format!("UpdateTable {} at {total} columns", fault.name);
            let mut added = int_columns(total - 2);
            (fault.plant)(&mut added);
            let got = emulator
                .call(
                    "UpdateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": table,
                        "addColumns": added,
                    }),
                )
                .await;

            match expected(total, &fault) {
                None => {
                    assert_ok(&case, got);
                    let (status, body) = get_table(&emulator, keyspace, &table).await;
                    let reported = reported_column_count(&body);
                    assert_eq!(
                        (status, reported),
                        (200, total),
                        "case {case:?}: the updated table must report its {total} columns, \
                         got {status} with {reported}"
                    );
                }
                Some((status, code)) => {
                    let body = got.1.clone();
                    assert_failure(&case, got, status, code);
                    if code == QUOTA.1 {
                        assert_names_the_column_cap(&case, &body);
                    }
                    let (read_status, read_body) = get_table(&emulator, keyspace, &table).await;
                    let reported = reported_column_count(&read_body);
                    assert_eq!(
                        (read_status, reported),
                        (200, 2),
                        "case {case:?}: a refused update must leave the schema as it was, \
                         got {read_status} with {reported}"
                    );
                }
            }
        }
    }
}

/// The other half of the rule: the cap depends on the request alone, so it is graded
/// ahead of every refusal that depends on what the store holds — the keyspace that is not
/// there, the table name already taken — while a request within the cap still earns those
/// refusals.
#[tokio::test]
async fn the_column_cap_is_graded_ahead_of_the_refusals_that_depend_on_stored_state() {
    let emulator = Emulator::start();
    let keyspace = "col_cap_state_ks";
    let taken = "already_there";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    assert_ok(
        "create the table whose name is then taken",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": taken,
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );

    /// One stored-state fault: the names to send, and the refusal it earns while the
    /// column list stays within the cap.
    struct Row {
        name: &'static str,
        keyspace_name: &'static str,
        table_name: &'static str,
        within_cap: (u16, &'static str),
    }

    let rows = [
        Row {
            name: "keyspace_that_does_not_exist",
            keyspace_name: "col_cap_no_such_ks",
            table_name: "any_tbl",
            within_cap: (400, "ResourceNotFoundException"),
        },
        Row {
            name: "table_name_already_taken",
            keyspace_name: keyspace,
            table_name: taken,
            within_cap: (400, "ConflictException"),
        },
    ];

    for row in rows {
        for total in totals() {
            let case = format!("{} at {total} columns", row.name);
            let mut all_columns = vec![json!({"name": "pk", "type": "text"})];
            all_columns.extend(int_columns(total - 1));
            let got = emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": row.keyspace_name,
                        "tableName": row.table_name,
                        "schemaDefinition": {
                            "allColumns": all_columns,
                            "partitionKeys": [{"name": "pk"}],
                        },
                    }),
                )
                .await;
            let body = got.1.clone();
            let (status, code) = match total > CAP {
                true => QUOTA,
                false => row.within_cap,
            };
            assert_failure(&case, got, status, code);
            if code == QUOTA.1 {
                assert_names_the_column_cap(&case, &body);
            }
        }
    }
}

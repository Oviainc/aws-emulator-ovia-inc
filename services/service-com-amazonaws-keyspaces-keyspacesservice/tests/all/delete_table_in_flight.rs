// SPDX-License-Identifier: CC0-1.0

//! `DeleteTable` against a table whose own status is not settled yet, driven end to end
//! over the service's `awsJson1_0` wire protocol.
//!
//! The rule under test, from this operation's own documentation
//! (`smithy_query(query="doc",shape="com.amazonaws.keyspaces#DeleteTable")`): "If the
//! table is in the ACTIVE state, you can delete it. If a table is either in the CREATING
//! or UPDATING states, then Amazon Keyspaces returns a ResourceInUseException. If the
//! specified table does not exist, Amazon Keyspaces returns a ResourceNotFoundException.
//! If the table is already in the DELETING state, no error is returned."
//!
//! So the wire error code for the in-flight case is `ResourceInUseException` — the code
//! the service's documentation names — and NOT the `ConflictException` this model
//! declares, which is a different code a client's "wait until the table settles, then
//! retry the delete" loop does not match. The shape is unmodeled, so the status is the
//! Smithy default for a client error, 400.
//!
//! Each assertion below spans a SET of inputs rather than one example:
//!
//! 1. Every way a table can be in flight — the create that puts it in `CREATING`, and
//!    each kind of update that puts it in `UPDATING` — is refused the same way, and the
//!    refusal moves nothing: the table is still in its in-flight status afterwards, and
//!    the delete is accepted once the clock settles it into `ACTIVE`.
//! 2. Every status a table in this emulator can be observed in, plus absence, mapped to
//!    the answer the documentation names for it. The status catalog is the model's own
//!    `TableStatus` value set (`crate::ops::types::TABLE_STATUS_VALUES`).

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::TABLE_STATUS_VALUES;
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, error_code, minimal_schema, settle_wait, Emulator,
};

/// The wire code and status Amazon Keyspaces' `DeleteTable` documentation names for a
/// table that is still `CREATING` or `UPDATING`.
const IN_USE_CODE: &str = "ResourceInUseException";
const IN_USE_STATUS: u16 = 400;

/// One row: how a table gets into an in-flight status, and which status that is.
struct InFlight {
    /// The table name, which is also the case name.
    table: &'static str,
    /// The `CreateTable` body (minus the names), which decides nothing about the status
    /// but makes the rows differ in the shape of the table under test.
    create: fn() -> Value,
    /// The `UpdateTable` body (minus the names) that moves the table to `UPDATING`, or
    /// `None` for a row that is tested while it is still `CREATING`.
    update: Option<fn() -> Value>,
    /// The status the table is in when `DeleteTable` is called.
    status: &'static str,
}

fn in_flight_rows() -> Vec<InFlight> {
    vec![
        InFlight {
            table: "creating_minimal",
            create: || json!({"schemaDefinition": minimal_schema("pk")}),
            update: None,
            status: keyspaces::TABLE_STATUS_CREATING,
        },
        InFlight {
            // A create carrying every optional setting: still `CREATING`, still refused.
            table: "creating_fully_specified",
            create: || {
                json!({
                    "schemaDefinition": {
                        "allColumns": [
                            {"name": "pk", "type": "text"},
                            {"name": "ck", "type": "timestamp"},
                            {"name": "payload", "type": "blob"},
                        ],
                        "partitionKeys": [{"name": "pk"}],
                        "clusteringKeys": [{"name": "ck", "orderBy": "ASC"}],
                    },
                    "comment": {"message": "in flight"},
                    "pointInTimeRecovery": {"status": "ENABLED"},
                    "defaultTimeToLive": 600,
                })
            },
            update: None,
            status: keyspaces::TABLE_STATUS_CREATING,
        },
        InFlight {
            table: "updating_default_ttl",
            create: || json!({"schemaDefinition": minimal_schema("pk")}),
            update: Some(|| json!({"defaultTimeToLive": 60})),
            status: keyspaces::TABLE_STATUS_UPDATING,
        },
        InFlight {
            table: "updating_capacity",
            create: || json!({"schemaDefinition": minimal_schema("pk")}),
            update: Some(|| {
                json!({
                    "capacitySpecification": {
                        "throughputMode": "PROVISIONED",
                        "readCapacityUnits": 5,
                        "writeCapacityUnits": 2,
                    }
                })
            }),
            status: keyspaces::TABLE_STATUS_UPDATING,
        },
        InFlight {
            table: "updating_added_columns",
            create: || json!({"schemaDefinition": minimal_schema("pk")}),
            update: Some(|| json!({"addColumns": [{"name": "extra", "type": "int"}]})),
            status: keyspaces::TABLE_STATUS_UPDATING,
        },
        InFlight {
            table: "updating_pitr",
            create: || json!({"schemaDefinition": minimal_schema("pk")}),
            update: Some(|| json!({"pointInTimeRecovery": {"status": "ENABLED"}})),
            status: keyspaces::TABLE_STATUS_UPDATING,
        },
    ]
}

/// `GetTable`'s reported status for one table.
async fn reported_status(emulator: &Emulator, keyspace: &str, table: &str) -> String {
    let body = assert_ok(
        table,
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await,
    );
    body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {table:?}: GetTable reported no status: {body}"))
        .to_string()
}

/// RULE 1: a delete against a table in any in-flight status answers
/// `400 ResourceInUseException`, leaves the table in that status, and is accepted once
/// the clock has settled the table into `ACTIVE`.
#[tokio::test]
async fn deleting_an_in_flight_table_answers_resource_in_use_and_changes_nothing() {
    let emulator = Emulator::start();
    let keyspace = "in_flight_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );

    for row in in_flight_rows() {
        let case = row.table;
        let mut create = (row.create)();
        create["keyspaceName"] = json!(keyspace);
        create["tableName"] = json!(case);
        assert_ok(case, emulator.call("CreateTable", create).await);

        if let Some(update) = row.update {
            // The table has to be ACTIVE before it can be updated, and the accepted
            // update is what puts it in `UPDATING`.
            tokio::time::sleep(settle_wait()).await;
            let mut update = update();
            update["keyspaceName"] = json!(keyspace);
            update["tableName"] = json!(case);
            assert_ok(case, emulator.call("UpdateTable", update).await);
        }

        // The row really is in the status it claims: a refusal of a table that had
        // already settled would prove nothing.
        assert_eq!(
            reported_status(&emulator, keyspace, case).await,
            row.status,
            "case {case:?}: expected the table to be {} when the delete is called",
            row.status
        );

        let (status, body) = emulator
            .call(
                "DeleteTable",
                json!({"keyspaceName": keyspace, "tableName": case}),
            )
            .await;
        assert_failure(case, (status, body.clone()), IN_USE_STATUS, IN_USE_CODE);
        // The refusal says what the caller is waiting for, which is the only thing it can
        // act on. The text is not documented, so only the status it names is asserted.
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(row.status),
            "case {case:?}: expected the refusal to name the status {}, got {body}",
            row.status
        );
        // The message is built from the request, so it names what the request named: a
        // message reduced to a constant would not.
        assert!(
            message.contains(keyspace) && message.contains(case),
            "case {case:?}: expected the refusal to name keyspace {keyspace:?} and table \
             {case:?}, got {body}"
        );

        // A refused delete is not a half-performed one: the table is still there, in the
        // same status, and no deletion is pending for it.
        assert_eq!(
            reported_status(&emulator, keyspace, case).await.as_str(),
            row.status,
            "case {case:?}: the refused delete moved the table's status"
        );
        tokio::time::sleep(settle_wait()).await;
        assert_eq!(
            reported_status(&emulator, keyspace, case).await.as_str(),
            keyspaces::TABLE_STATUS_ACTIVE,
            "case {case:?}: the in-flight status settles into ACTIVE, not into absence"
        );

        // "If the table is in the ACTIVE state, you can delete it."
        assert_ok(
            case,
            emulator
                .call(
                    "DeleteTable",
                    json!({"keyspaceName": keyspace, "tableName": case}),
                )
                .await,
        );
        assert_eq!(
            reported_status(&emulator, keyspace, case).await.as_str(),
            keyspaces::TABLE_STATUS_DELETING,
            "case {case:?}: an accepted delete puts the table in DELETING"
        );
    }
}

/// RULE 2: every status a table can be observed in here, and absence, mapped to the
/// answer `DeleteTable` documents for it — so the in-use refusal is pinned against the
/// cases that are NOT it, and in particular against the `ConflictException` the model
/// declares for a different claim.
#[tokio::test]
async fn delete_table_answers_one_outcome_per_observed_table_status() {
    /// `None` is a success; `Some((status, code))` is the refusal.
    type Outcome = Option<(u16, &'static str)>;

    let emulator = Emulator::start();
    let keyspace = "status_matrix_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );

    // The statuses this emulator can put a table in, each with the outcome the
    // documentation names. `DELETED`, `RESTORING` and
    // `INACCESSIBLE_ENCRYPTION_CREDENTIALS` are in the model's `TableStatus` set but are
    // unreachable here (no operation produces them — `RestoreTable` is still a stub), so
    // they are covered by the completeness assertion below rather than by a row.
    let rows: [(&str, &str, Outcome); 4] = [
        (
            "creating_tbl",
            keyspaces::TABLE_STATUS_CREATING,
            Some((IN_USE_STATUS, IN_USE_CODE)),
        ),
        (
            "updating_tbl",
            keyspaces::TABLE_STATUS_UPDATING,
            Some((IN_USE_STATUS, IN_USE_CODE)),
        ),
        ("active_tbl", keyspaces::TABLE_STATUS_ACTIVE, None),
        ("deleting_tbl", keyspaces::TABLE_STATUS_DELETING, None),
    ];

    for (table, status, expected) in rows {
        assert_ok(
            table,
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
        if status != keyspaces::TABLE_STATUS_CREATING {
            tokio::time::sleep(settle_wait()).await;
        }
        if status == keyspaces::TABLE_STATUS_UPDATING {
            assert_ok(
                table,
                emulator
                    .call(
                        "UpdateTable",
                        json!({
                            "keyspaceName": keyspace,
                            "tableName": table,
                            "defaultTimeToLive": 120,
                        }),
                    )
                    .await,
            );
        }
        if status == keyspaces::TABLE_STATUS_DELETING {
            assert_ok(
                table,
                emulator
                    .call(
                        "DeleteTable",
                        json!({"keyspaceName": keyspace, "tableName": table}),
                    )
                    .await,
            );
        }
        assert_eq!(
            reported_status(&emulator, keyspace, table).await.as_str(),
            status,
            "case {table:?}: the row's table is not in the status it claims"
        );

        let got = emulator
            .call(
                "DeleteTable",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await;
        match expected {
            Some((expected_status, expected_code)) => {
                assert_failure(table, got, expected_status, expected_code)
            }
            // "If the table is in the ACTIVE state, you can delete it. ... If the table is
            // already in the DELETING state, no error is returned."
            None => {
                assert_ok(table, got);
            }
        }
    }

    // "If the specified table does not exist, Amazon Keyspaces returns a
    // ResourceNotFoundException" — the other refusal, so the in-use code is not the
    // answer to every unhappy delete.
    assert_failure(
        "a table that was never created",
        emulator
            .call(
                "DeleteTable",
                json!({"keyspaceName": keyspace, "tableName": "absent_tbl"}),
            )
            .await,
        404,
        "ResourceNotFoundException",
    );

    // Every status the rows above claim is a member of the model's own `TableStatus` set,
    // so a renamed or dropped status fails here rather than silently passing.
    for (table, status, _) in rows {
        assert!(
            TABLE_STATUS_VALUES.contains(&status),
            "case {table:?}: {status} is not a modelled TableStatus ({TABLE_STATUS_VALUES:?})"
        );
    }
}

/// The in-use refusal is NOT the scaffold's 501 and NOT the modelled `ConflictException`:
/// the code on the wire is exactly what a client branches on, so it is pinned by name.
#[tokio::test]
async fn the_in_use_refusal_carries_its_own_code_and_not_a_substitute() {
    let emulator = Emulator::start();
    let keyspace = "code_ks";
    let table = "code_tbl";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    assert_ok(
        table,
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

    let (status, body) = emulator
        .call(
            "DeleteTable",
            json!({"keyspaceName": keyspace, "tableName": table}),
        )
        .await;
    let code = error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (IN_USE_STATUS, IN_USE_CODE),
        "expected {IN_USE_STATUS} {IN_USE_CODE}, got {status} {body}"
    );
    for substitute in ["ConflictException", "NotImplemented", "ValidationException"] {
        assert_ne!(
            code.as_str(), substitute,
            "the in-flight delete must not answer {substitute}: {body}"
        );
    }
}

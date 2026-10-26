// SPDX-License-Identifier: CC0-1.0

//! The status a table whose point-in-time restore is still running reports, driven end to
//! end over the service's own `awsJson1_0` wire protocol.
//!
//! The rule under test: an asynchronous operation reports the resource it is working on
//! in the member of the resource's modelled status value set that names THAT operation's
//! in-flight state, so the table `RestoreTable` is building reports
//! `TableStatus = RESTORING` — not the `CREATING` of a plain create — until the clock owes
//! it `ACTIVE`. The status catalog is the model's own `TableStatus` value set
//! (`crate::ops::types::TABLE_STATUS_VALUES`, `smithy_query(query="shape",
//! shape="TableStatus")`), which holds `RESTORING` beside `CREATING`.
//!
//! What follows from it, asserted here over a SET of inputs rather than one example:
//!
//! 1. Every accepted restore — whatever the source table's shape, whichever optional
//!    members the request carries, into the source's own keyspace or another one — reports
//!    `RESTORING` for its target table from the moment the call is accepted, and `ACTIVE`
//!    once the clock has settled it.
//! 2. A table in that status has a change of its own in flight, so it takes no further
//!    change and is no restorable source, and the keyspace holding it is not deletable —
//!    exactly as a table that is still `CREATING` or `UPDATING`.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::TABLE_STATUS_VALUES;
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, ensure_keyspace, epoch_seconds, error_code, minimal_schema,
    settle_wait, Emulator,
};

/// The keyspace the source tables of this suite live in.
const SOURCE_KEYSPACE: &str = "restoring_src_ks";

/// A second keyspace, so a row can restore ACROSS keyspaces as well as within one.
const TARGET_KEYSPACE: &str = "restoring_tgt_ks";

/// The wire code and status Amazon Keyspaces' `DeleteTable` documentation names for a
/// table whose own change is still in flight ("If a table is either in the CREATING or
/// UPDATING states, then Amazon Keyspaces returns a ResourceInUseException"). The shape
/// is unmodeled, so the status is the Smithy default for a client error.
const IN_USE_CODE: &str = "ResourceInUseException";
const IN_USE_STATUS: u16 = 400;

/// `GetTable`'s reported `status` for one table.
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
    body.get("status")
        .and_then(Value::as_str)
        .unwrap_or("<no status member>")
        .to_string()
}

/// Create `table` in `keyspace` with point-in-time recovery enabled (a table without a
/// restorable window cannot be restored at all) and wait for it to settle into `ACTIVE`.
async fn create_restorable_source(
    emulator: &Emulator,
    keyspace: &str,
    table: &str,
    schema: Value,
) {
    assert_ok(
        table,
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "schemaDefinition": schema,
                    "pointInTimeRecovery": {"status": "ENABLED"},
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;
}

/// One row: a source table to restore, the target it is restored to, and the optional
/// members the restore carries.
struct Row {
    /// The case name, which is also the source table's name.
    name: &'static str,
    /// The source table's schema, so the rows differ in the shape of what is restored.
    schema: fn() -> Value,
    /// The keyspace the restored table is created in — the source's own, or the other one.
    target_keyspace: &'static str,
    /// The restored table's name, so the rows differ in the length and form of it.
    target_table: &'static str,
    /// Whether the request names an explicit `restoreTimestamp` (the current instant,
    /// which is inside every enabled table's window) rather than leaving it out.
    explicit_timestamp: bool,
    /// The restore request's further optional members.
    extra: fn() -> Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            // The plainest restore there is: no optional member at all.
            name: "minimal",
            schema: || minimal_schema("pk"),
            target_keyspace: TARGET_KEYSPACE,
            target_table: "t",
            explicit_timestamp: false,
            extra: || json!({}),
        },
        Row {
            // A restore at an explicit point in the window, into the SOURCE's own
            // keyspace: the status does not depend on where the table lands.
            name: "same_keyspace_at_a_timestamp",
            schema: || minimal_schema("pk"),
            target_keyspace: SOURCE_KEYSPACE,
            target_table: "restored_in_place",
            explicit_timestamp: true,
            extra: || json!({}),
        },
        Row {
            // Overrides of the settings the operation documents as overridable, and a
            // target name at the maximum length the modelled `tableName` shape accepts
            // (`len 1..48`, pattern `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`) — the `"t"` row
            // above is that shape's minimum.
            name: "every_override",
            schema: || {
                json!({
                    "allColumns": [
                        {"name": "pk", "type": "text"},
                        {"name": "ck", "type": "timestamp"},
                        {"name": "shared", "type": "int"},
                    ],
                    "partitionKeys": [{"name": "pk"}],
                    "clusteringKeys": [{"name": "ck", "orderBy": "ASC"}],
                    "staticColumns": [{"name": "shared"}],
                })
            },
            target_keyspace: TARGET_KEYSPACE,
            target_table: "abcdefghijklabcdefghijklabcdefghijklabcdefghijkl",
            explicit_timestamp: true,
            extra: || {
                json!({
                    "capacitySpecificationOverride": {
                        "throughputMode": "PROVISIONED",
                        "readCapacityUnits": 7,
                        "writeCapacityUnits": 9,
                    },
                    "pointInTimeRecoveryOverride": {"status": "DISABLED"},
                    "tagsOverride": [{"key": "team", "value": "platform"}],
                })
            },
        },
    ]
}

/// RULE 1: every accepted restore reports `RESTORING` for the table it is building, and
/// `ACTIVE` once the clock owes it.
#[tokio::test]
async fn a_restore_in_flight_reports_the_modelled_restoring_status() {
    // The status this suite expects is a member of the model's own `TableStatus` value
    // set, and a DIFFERENT member from the one a plain create reports — so a renamed or
    // dropped status fails here rather than silently passing.
    assert!(
        TABLE_STATUS_VALUES.contains(&keyspaces::TABLE_STATUS_RESTORING),
        "case {:?}: {} is not a modelled TableStatus ({TABLE_STATUS_VALUES:?})",
        "the status catalog",
        keyspaces::TABLE_STATUS_RESTORING
    );
    assert_ne!(
        keyspaces::TABLE_STATUS_RESTORING,
        keyspaces::TABLE_STATUS_CREATING,
        "case {:?}: the restore status and the create status are two different members",
        "the status catalog"
    );

    let emulator = Emulator::start();
    ensure_keyspace(&emulator, SOURCE_KEYSPACE).await;
    ensure_keyspace(&emulator, TARGET_KEYSPACE).await;

    for row in rows() {
        create_restorable_source(&emulator, SOURCE_KEYSPACE, row.name, (row.schema)()).await;

        let mut request = json!({
            "sourceKeyspaceName": SOURCE_KEYSPACE,
            "sourceTableName": row.name,
            "targetKeyspaceName": row.target_keyspace,
            "targetTableName": row.target_table,
        });
        if row.explicit_timestamp {
            request["restoreTimestamp"] = json!(epoch_seconds(SystemTime::now()));
        }
        if let Some(members) = (row.extra)().as_object() {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        assert_ok(row.name, emulator.call("RestoreTable", request).await);

        let in_flight = reported_status(&emulator, row.target_keyspace, row.target_table).await;
        assert_eq!(
            in_flight,
            keyspaces::TABLE_STATUS_RESTORING,
            "case {:?}: expected a table whose restore is in flight to report {}, got \
             {in_flight}",
            row.name,
            keyspaces::TABLE_STATUS_RESTORING
        );

        tokio::time::sleep(settle_wait()).await;
        let settled = reported_status(&emulator, row.target_keyspace, row.target_table).await;
        assert_eq!(
            settled,
            keyspaces::TABLE_STATUS_ACTIVE,
            "case {:?}: expected the restored table to settle into {}, got {settled}",
            row.name,
            keyspaces::TABLE_STATUS_ACTIVE
        );
    }
}

/// RULE 2: while its restore is in flight the target table has a change of its own
/// running, so each call that needs a settled table is refused with the error that call
/// declares for an in-flight one, the refusals move nothing, and everything is accepted
/// once the clock has settled the restore.
#[tokio::test]
async fn a_table_whose_restore_is_in_flight_takes_no_change_and_blocks_its_keyspace() {
    let emulator = Emulator::start();
    ensure_keyspace(&emulator, SOURCE_KEYSPACE).await;
    ensure_keyspace(&emulator, TARGET_KEYSPACE).await;
    create_restorable_source(&emulator, SOURCE_KEYSPACE, "blocking_src", minimal_schema("pk"))
        .await;

    let target = "blocked_tgt";
    assert_ok(
        "accept the restore",
        emulator
            .call(
                "RestoreTable",
                json!({
                    "sourceKeyspaceName": SOURCE_KEYSPACE,
                    "sourceTableName": "blocking_src",
                    "targetKeyspaceName": TARGET_KEYSPACE,
                    "targetTableName": target,
                }),
            )
            .await,
    );

    // "If a table is either in the CREATING or UPDATING states, then Amazon Keyspaces
    // returns a ResourceInUseException": a table whose restore is still running is in
    // flight the same way, so the delete is refused with the documented code.
    assert_failure(
        "delete a table whose restore is in flight",
        emulator
            .call(
                "DeleteTable",
                json!({"keyspaceName": TARGET_KEYSPACE, "tableName": target}),
            )
            .await,
        IN_USE_STATUS,
        IN_USE_CODE,
    );

    // `UpdateTable` and `RestoreTable` both need a table whose status is `ACTIVE`, and
    // answer the modelled conflict for one whose action is already in progress.
    let (status, body) = emulator
        .call(
            "UpdateTable",
            json!({
                "keyspaceName": TARGET_KEYSPACE,
                "tableName": target,
                "defaultTimeToLive": 120,
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (409, "ConflictException"),
        "case {:?}: expected 409 ConflictException, got {status} {body}",
        "update a table whose restore is in flight"
    );
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains(keyspaces::TABLE_STATUS_RESTORING),
        "case {:?}: expected the refusal to name the status the caller is waiting on, got \
         {message:?}",
        "update a table whose restore is in flight"
    );

    assert_failure(
        "restore from a table whose own restore is in flight",
        emulator
            .call(
                "RestoreTable",
                json!({
                    "sourceKeyspaceName": TARGET_KEYSPACE,
                    "sourceTableName": target,
                    "targetKeyspaceName": TARGET_KEYSPACE,
                    "targetTableName": "second_hop",
                }),
            )
            .await,
        409,
        "ConflictException",
    );

    // "The DeleteKeyspace operation deletes a keyspace and all of its tables", so a
    // keyspace holding a table this emulator refuses to drop is not deletable yet either.
    assert_failure(
        "delete the keyspace holding a table whose restore is in flight",
        emulator
            .call("DeleteKeyspace", json!({"keyspaceName": TARGET_KEYSPACE}))
            .await,
        409,
        "ConflictException",
    );

    // None of those refusals moved the restore along.
    let still = reported_status(&emulator, TARGET_KEYSPACE, target).await;
    assert_eq!(
        still,
        keyspaces::TABLE_STATUS_RESTORING,
        "case {:?}: expected the refused calls to leave the table in {}, got {still}",
        "the refusals move nothing",
        keyspaces::TABLE_STATUS_RESTORING
    );

    // Once the clock owes the restore its `ACTIVE`, the same calls are accepted.
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        "update the restored table once it settled",
        emulator
            .call(
                "UpdateTable",
                json!({
                    "keyspaceName": TARGET_KEYSPACE,
                    "tableName": target,
                    "defaultTimeToLive": 120,
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        "delete the restored table once it settled",
        emulator
            .call(
                "DeleteTable",
                json!({"keyspaceName": TARGET_KEYSPACE, "tableName": target}),
            )
            .await,
    );
}

// SPDX-License-Identifier: CC0-1.0

//! `RestoreTable` resolving the schema it reproduces against the TARGET keyspace, driven
//! end to end over the service's own `awsJson1_0` wire protocol through the shared
//! [`crate::keyspace_table_lifecycle::Emulator`] harness — so what is pinned is the
//! emulator's observable behaviour, and a restore is only believed once `GetTable` reports
//! the restored table back.
//!
//! The rule under test, exercised over a SET of inputs rather than one example:
//!
//! A column's type may name only user-defined types that are "contained in the same
//! keyspace" (`ColumnDefinition$type` / `FieldDefinition$type`), so a schema is stored in
//! a keyspace only when every identifier in its column types that names neither a CQL
//! native type nor a type constructor is a type of THAT keyspace in status `ACTIVE`; any
//! other reference is the modelled `ResourceNotFoundException` ("UDT `<name>` does not
//! exist") and nothing is stored. A restore writes a schema read out of the SOURCE
//! keyspace into the TARGET keyspace, so the target keyspace is what its references are
//! resolved against, whichever keyspace the schema came from — and the schema that is
//! graded is the revision the selected timestamp picks, which is the one the restore would
//! store.
//!
//! The suites that pin the same rule for the other operations that write a schema are
//! `tests/all/column_types_and_auto_scaling.rs` (`CreateTable`),
//! `tests/all/update_table_add_columns_types.rs` (`UpdateTable`) and
//! `tests/all/type_lifecycle.rs` (`CreateType`).

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::column_types_and_auto_scaling::{assert_member, schema_with_type};
use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, ensure_keyspace, epoch_seconds, reported_schema, settle_wait,
    table_arn, Emulator,
};

/// The keyspace every source table lives in. It holds each type a source table's schema
/// refers to, so every source table below is one `CreateTable` accepts.
const SOURCE: &str = "rt_resolve_src_ks";

/// A target keyspace that holds no user-defined type at all.
const BARE_TARGET: &str = "rt_resolve_bare_ks";

/// A target keyspace that holds SOME of the source keyspace's type names.
const MIRROR_TARGET: &str = "rt_resolve_mirror_ks";

/// A target keyspace whose type is created but not yet settled.
const PENDING_TARGET: &str = "rt_resolve_pending_ks";

/// The names `keyspace` is given types under, in the form `CreateType` takes them. Both
/// forms `CreateType$typeName` documents are covered: an unquoted name, which the service
/// case-folds, and a double-quoted one, which it preserves verbatim.
///
/// Also the set a reference is expected to resolve against, so one statement decides both
/// what is created and what a restore into that keyspace may name.
fn types_of(keyspace: &str) -> &'static [&'static str] {
    match keyspace {
        SOURCE => &["address", "contact", "\"Addr\""],
        MIRROR_TARGET => &["address", "\"Addr\""],
        _ => &[],
    }
}

/// Create every type [`types_of`] gives `keyspace`. The caller settles them.
async fn create_types(emulator: &Emulator, keyspace: &str) {
    for type_name in types_of(keyspace) {
        assert_ok(
            type_name,
            emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": keyspace,
                        "typeName": type_name,
                        "fieldDefinitions": [{"name": "street", "type": "text"}],
                    }),
                )
                .await,
        );
    }
}

/// Create one source table whose single regular column declares `column_type`, with
/// point-in-time recovery enabled so it has a restorable window at all. The caller
/// settles it.
async fn create_source_table(emulator: &Emulator, table: &str, column_type: &str) {
    assert_ok(
        table,
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": SOURCE,
                    "tableName": table,
                    "schemaDefinition": schema_with_type(column_type),
                    "pointInTimeRecovery": {"status": "ENABLED"},
                }),
            )
            .await,
    );
}

/// A `RestoreTable` request from a source table of [`SOURCE`] into one target, plus
/// whatever `extra` members the case supplies.
fn restore_request(source: &str, target_keyspace: &str, target_table: &str, extra: Value) -> Value {
    let mut request = json!({
        "sourceKeyspaceName": SOURCE,
        "sourceTableName": source,
        "targetKeyspaceName": target_keyspace,
        "targetTableName": target_table,
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            request[member] = value.clone();
        }
    }
    request
}

/// `GetTable` for one table, as a `(status, body)` pair the caller grades.
async fn get_table(emulator: &Emulator, keyspace: &str, table: &str) -> (u16, Value) {
    emulator
        .call(
            "GetTable",
            json!({"keyspaceName": keyspace, "tableName": table}),
        )
        .await
}

/// Assert a restore was refused for `referenced` with the modelled not-found, and that it
/// created no table under the target name.
async fn assert_unresolvable(
    case: &str,
    emulator: &Emulator,
    request: Value,
    target_keyspace: &str,
    target_table: &str,
    referenced: &str,
) {
    let (status, body) = emulator.call("RestoreTable", request).await;
    assert_failure(case, (status, body.clone()), 400, "ResourceNotFoundException");
    assert_member(
        case,
        &body,
        "/message",
        &json!(format!("UDT {referenced} does not exist")),
    );
    assert_failure(
        case,
        get_table(emulator, target_keyspace, target_table).await,
        400,
        "ResourceNotFoundException",
    );
}

/// One row: a column type a source table declares, and the user-defined type its
/// references resolve to.
struct Row {
    /// The case name, which is also the source table's name.
    name: &'static str,
    /// The declared column type.
    column_type: &'static str,
    /// The FORMATTED name a refusal must report — an unquoted reference case-folded, a
    /// double-quoted one verbatim (`GetType$typeName`) — or `None` for a type expression
    /// built only from the native types and the type constructors, which refers to no
    /// user-defined type at all.
    referenced: Option<&'static str>,
}

/// The rows: the ways a column type reaches a user-defined type (bare, frozen, inside
/// each kind of parameterised type, case-folded, double-quoted), a type only the SOURCE
/// keyspace holds, and one type expression that refers to nothing — so the rule is
/// asserted across the class rather than on the one shape the report cited.
const ROWS: &[Row] = &[
    Row {
        name: "native_only",
        column_type: "map<text, int>",
        referenced: None,
    },
    Row {
        name: "frozen_udt",
        column_type: "frozen<address>",
        referenced: Some("address"),
    },
    Row {
        name: "bare_udt",
        column_type: "address",
        referenced: Some("address"),
    },
    Row {
        name: "list_of_udt",
        column_type: "list<frozen<address>>",
        referenced: Some("address"),
    },
    Row {
        name: "map_value_udt",
        column_type: "map<text, frozen<address>>",
        referenced: Some("address"),
    },
    Row {
        name: "tuple_with_udt",
        column_type: "tuple<int, frozen<address>>",
        referenced: Some("address"),
    },
    Row {
        name: "upper_case_udt",
        column_type: "frozen<ADDRESS>",
        referenced: Some("address"),
    },
    Row {
        name: "quoted_udt",
        column_type: "frozen<\"Addr\">",
        referenced: Some("\"Addr\""),
    },
    Row {
        name: "udt_only_the_source_holds",
        column_type: "frozen<contact>",
        referenced: Some("contact"),
    },
];

/// One accepted restore, issued and then read back once the clock has settled it.
struct Accepted {
    case: String,
    request: Value,
    target_keyspace: &'static str,
    target_table: String,
    column_type: &'static str,
}

/// THE RULE: every (column type, target keyspace) pair is accepted exactly when the
/// TARGET keyspace itself holds the type the restored schema names — including a restore
/// back into the source keyspace, which holds every one of them — and is otherwise the
/// modelled not-found naming that type, with no table created.
#[tokio::test]
async fn a_restored_schema_resolves_its_type_references_against_the_target_keyspace() {
    let emulator = Emulator::start();
    for keyspace in [SOURCE, BARE_TARGET, MIRROR_TARGET] {
        ensure_keyspace(&emulator, keyspace).await;
        create_types(&emulator, keyspace).await;
    }
    // One settle for every type: a column type may only name a type that is `ACTIVE`.
    tokio::time::sleep(settle_wait()).await;
    for row in ROWS {
        create_source_table(&emulator, row.name, row.column_type).await;
    }
    // And one settle for every source table: a restore reads a source only from `ACTIVE`.
    tokio::time::sleep(settle_wait()).await;

    let mut accepted: Vec<Accepted> = Vec::new();
    for (index, target_keyspace) in [BARE_TARGET, MIRROR_TARGET, SOURCE].into_iter().enumerate() {
        for row in ROWS {
            let case = format!("{} into {target_keyspace}", row.name);
            let target_table = format!("tgt{index}_{}", row.name);
            let request = restore_request(row.name, target_keyspace, &target_table, json!({}));
            // Unresolvable exactly when the schema names a type this keyspace does not
            // hold, whichever keyspace the schema was read out of.
            match row
                .referenced
                .filter(|referenced| !types_of(target_keyspace).contains(referenced))
            {
                Some(referenced) => {
                    assert_unresolvable(
                        &case,
                        &emulator,
                        request,
                        target_keyspace,
                        &target_table,
                        referenced,
                    )
                    .await
                }
                None => accepted.push(Accepted {
                    case,
                    request,
                    target_keyspace,
                    target_table,
                    column_type: row.column_type,
                }),
            }
        }
    }

    // "Any number of users can execute up to 4 concurrent restores (any type of restore)
    // in a given account", so the accepted restores are issued in chunks of that many and
    // each chunk is settled before the next — what is under test here is the schema
    // resolution, never the account's concurrency.
    for chunk in accepted.chunks(keyspaces::MAX_CONCURRENT_RESTORES) {
        for job in chunk {
            let restored = assert_ok(&job.case, emulator.call("RestoreTable", job.request.clone()).await);
            assert_member(
                &job.case,
                &restored,
                "/restoredTableARN",
                &json!(table_arn(job.target_keyspace, &job.target_table)),
            );
        }
        tokio::time::sleep(settle_wait()).await;
        for job in chunk {
            let got = assert_ok(
                &job.case,
                get_table(&emulator, job.target_keyspace, &job.target_table).await,
            );
            assert_eq!(
                got["status"],
                json!(keyspaces::TABLE_STATUS_ACTIVE),
                "case {:?}: expected the restored table to settle into {}, got {got}",
                job.case,
                keyspaces::TABLE_STATUS_ACTIVE
            );
            assert_eq!(
                got["schemaDefinition"],
                reported_schema(&schema_with_type(job.column_type)),
                "case {:?}: expected the restored schema to read back verbatim, got {got}",
                job.case
            );
        }
    }
}

/// THE RULE, for a type's STATUS: a reference resolves only to a type that is `ACTIVE`,
/// so a target keyspace whose type is still settling cannot take the schema yet, and the
/// very same restore is accepted once that type is `ACTIVE`.
#[tokio::test]
async fn a_restore_resolves_only_against_a_target_type_that_is_active() {
    let emulator = Emulator::start();
    for keyspace in [SOURCE, PENDING_TARGET] {
        ensure_keyspace(&emulator, keyspace).await;
    }
    create_types(&emulator, SOURCE).await;
    tokio::time::sleep(settle_wait()).await;
    create_source_table(&emulator, "src_udt", "frozen<address>").await;
    tokio::time::sleep(settle_wait()).await;

    // The target keyspace is given the type the schema names, and nothing waits for it:
    // it is still `CREATING` when the restore arrives.
    assert_ok(
        "create the pending type",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": PENDING_TARGET,
                    "typeName": "address",
                    "fieldDefinitions": [{"name": "street", "type": "text"}],
                }),
            )
            .await,
    );
    let request = restore_request("src_udt", PENDING_TARGET, "tgt_pending", json!({}));
    assert_unresolvable(
        "type_still_creating",
        &emulator,
        request.clone(),
        PENDING_TARGET,
        "tgt_pending",
        "address",
    )
    .await;

    // Once the type is `ACTIVE` the same request resolves and the table is restored.
    tokio::time::sleep(settle_wait()).await;
    let restored = assert_ok(
        "type_now_active",
        emulator.call("RestoreTable", request).await,
    );
    assert_member(
        "type_now_active",
        &restored,
        "/restoredTableARN",
        &json!(table_arn(PENDING_TARGET, "tgt_pending")),
    );
    tokio::time::sleep(settle_wait()).await;
    let got = assert_ok(
        "type_now_active",
        get_table(&emulator, PENDING_TARGET, "tgt_pending").await,
    );
    assert_eq!(
        got["schemaDefinition"],
        reported_schema(&schema_with_type("frozen<address>")),
        "case {:?}: expected the restored schema to read back verbatim, got {got}",
        "type_now_active"
    );
}

/// THE RULE, applied to the schema the restore would actually STORE: "the table's schema
/// data and TTL settings ... are restored based on the selected timestamp", so it is the
/// revision that timestamp picks whose references are resolved — an earlier revision that
/// names no user-defined type restores into a keyspace that holds none, while the current
/// revision of the same table does not.
#[tokio::test]
async fn a_restore_resolves_the_revision_the_selected_timestamp_picks() {
    let emulator = Emulator::start();
    for keyspace in [SOURCE, BARE_TARGET, MIRROR_TARGET] {
        ensure_keyspace(&emulator, keyspace).await;
        create_types(&emulator, keyspace).await;
    }
    tokio::time::sleep(settle_wait()).await;
    // Revision 1: a schema built only from native types.
    create_source_table(&emulator, "evolving", "int").await;
    tokio::time::sleep(settle_wait()).await;
    let before_the_udt_column = SystemTime::now();
    // Revision 2, the current one: a column of a type only the source keyspace holds.
    assert_ok(
        "add the udt column",
        emulator
            .call(
                "UpdateTable",
                json!({
                    "keyspaceName": SOURCE,
                    "tableName": "evolving",
                    "addColumns": [{"name": "home", "type": "frozen<address>"}],
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    // The current revision names a type the bare keyspace does not hold, so a restore of
    // it there is refused and creates nothing.
    assert_unresolvable(
        "current_revision_into_a_keyspace_without_the_type",
        &emulator,
        restore_request("evolving", BARE_TARGET, "tgt_current_bare", json!({})),
        BARE_TARGET,
        "tgt_current_bare",
        "address",
    )
    .await;

    // The revision in effect before that column was added names no user-defined type at
    // all, so the same pair of keyspaces takes it.
    assert_ok(
        "earlier_revision_into_a_keyspace_without_the_type",
        emulator
            .call(
                "RestoreTable",
                restore_request(
                    "evolving",
                    BARE_TARGET,
                    "tgt_earlier_bare",
                    json!({"restoreTimestamp": epoch_seconds(before_the_udt_column)}),
                ),
            )
            .await,
    );
    // And the current revision restores into a keyspace that does hold the type.
    assert_ok(
        "current_revision_into_a_keyspace_with_the_type",
        emulator
            .call(
                "RestoreTable",
                restore_request("evolving", MIRROR_TARGET, "tgt_current_mirror", json!({})),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    for (case, target_keyspace, target_table, expect_udt_column) in [
        (
            "earlier_revision_into_a_keyspace_without_the_type",
            BARE_TARGET,
            "tgt_earlier_bare",
            false,
        ),
        (
            "current_revision_into_a_keyspace_with_the_type",
            MIRROR_TARGET,
            "tgt_current_mirror",
            true,
        ),
    ] {
        let got = assert_ok(case, get_table(&emulator, target_keyspace, target_table).await);
        let columns = got
            .pointer("/schemaDefinition/allColumns")
            .cloned()
            .unwrap_or(Value::Null);
        let has_udt_column = columns
            .as_array()
            .is_some_and(|all| all.iter().any(|column| column["name"] == json!("home")));
        assert_eq!(
            has_udt_column, expect_udt_column,
            "case {case:?}: expected the restored schema to hold the user-defined column \
             ={expect_udt_column}, got {columns}"
        );
    }
}

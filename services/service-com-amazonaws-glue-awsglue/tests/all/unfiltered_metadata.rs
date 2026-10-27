// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Glue's Lake Formation application-integration partition reads:
//! `GetUnfilteredPartitionMetadata` and `GetUnfilteredPartitionsMetadata`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: these reads are served only to a query engine calling through a
//! vended query session, so a caller presenting ordinary credentials is refused with HTTP
//! 400 `AccessDeniedException` and an empty message; the gate is decided BEFORE any
//! catalog lookup, so the refusal is identical for a partition the catalog holds and for
//! one it never held — it leaks no not-found signal — and it changes nothing, so the
//! ungated `GetPartition` reports exactly what it reported before.
//!
//! The rows below differ along what the rule generalises over: which of the two operations
//! is called, whether the addressed database, table and partition exist, how many
//! partition keys and values are in play, which `SupportedPermissionTypes` the call names
//! (every value of the modelled `PermissionType` enum — `COLUMN_PERMISSION`,
//! `CELL_FILTER_PERMISSION`, `NESTED_PERMISSION`, `NESTED_CELL_PERMISSION`, from
//! `smithy_query shape=PermissionType`), and whether it carries a `QuerySessionContext`,
//! an `AuditContext` or a `Region`.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::UNFILTERED_PARTITION_METADATA_ACCESS_DENIED;
use smithy_server_runtime::MultiServiceRouter;

use crate::partitions::{batch_create, get_partition, partition_errors, seed_table};
use crate::wire::{call, emulator, error_code, error_message, ACCOUNT_ID};

/// The database and table every row addresses, and the one partition it holds.
const DATABASE: &str = "unfiltered_db";
const TABLE: &str = "unfiltered_table";
const STORED_VALUE: &str = "2024-01-01";

/// A partition value the table was never given a partition for.
const ABSENT_VALUE: &str = "1999-12-31";

/// Every value of the modelled `PermissionType` enum, which
/// `SupportedPermissionTypes` is a list of. Source: `smithy_query shape=PermissionType`.
const PERMISSION_TYPES: [&str; 4] = [
    "COLUMN_PERMISSION",
    "CELL_FILTER_PERMISSION",
    "NESTED_PERMISSION",
    "NESTED_CELL_PERMISSION",
];

/// The catalog, table and one stored partition every row is resolved against.
async fn seed(router: &MultiServiceRouter) {
    seed_table(router, DATABASE, TABLE, &["dt"]).await;
    let created = batch_create(
        router,
        DATABASE,
        TABLE,
        &[json!({ "Values": [STORED_VALUE] })],
    )
    .await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partition must be created, got Errors {:?}",
        created["Errors"]
    );
}

/// A single-partition read body addressing `values`, plus whatever `extra` adds.
fn one_partition_body(database: &str, table: &str, values: &[&str], extra: Value) -> Value {
    let mut body = json!({
        "CatalogId": ACCOUNT_ID,
        "DatabaseName": database,
        "TableName": table,
        "PartitionValues": values,
        "SupportedPermissionTypes": [PERMISSION_TYPES[0]],
    });
    merge(&mut body, extra);
    body
}

/// A list read body for one table, plus whatever `extra` adds.
fn many_partitions_body(database: &str, table: &str, extra: Value) -> Value {
    let mut body = json!({
        "CatalogId": ACCOUNT_ID,
        "DatabaseName": database,
        "TableName": table,
        "SupportedPermissionTypes": [PERMISSION_TYPES[0]],
    });
    merge(&mut body, extra);
    body
}

/// Copy every member of `extra` onto `body`, overwriting what it already carries.
fn merge(body: &mut Value, extra: Value) {
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
}

/// Assert that `operation` refuses `body` the way the gate does.
async fn assert_denied(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: an ordinary caller is refused, got {status} with {parsed}"
    );
    assert!(
        error_code(&parsed).ends_with("AccessDeniedException"),
        "case {case:?}: expected AccessDeniedException, got {}",
        error_code(&parsed)
    );
    assert_eq!(
        error_message(&parsed),
        UNFILTERED_PARTITION_METADATA_ACCESS_DENIED,
        "case {case:?}: the refusal carries no wording at all, got {:?}",
        error_message(&parsed)
    );
    // The same expectation spelled literally: the recorded refusal's message is the empty
    // string, so a change to the crate's constant cannot silently redefine what is pinned.
    assert_eq!(
        error_message(&parsed),
        "",
        "case {case:?}: the recorded refusal's message is empty, got {:?}",
        error_message(&parsed)
    );
}

// ---------------------------------------------------------------------------
// The gate is evaluated before, and regardless of, any catalog lookup
// ---------------------------------------------------------------------------

/// Every shape of call is refused the same way, whether or not what it addresses exists.
#[tokio::test]
async fn an_ordinary_caller_is_refused_whatever_the_catalog_holds() {
    let router = emulator();
    seed(&router).await;

    // (case, operation, the call that must be refused)
    let mut denied: Vec<(String, &str, Value)> = vec![
        (
            "a partition the table holds".to_string(),
            "GetUnfilteredPartitionMetadata",
            one_partition_body(DATABASE, TABLE, &[STORED_VALUE], json!({})),
        ),
        (
            "a partition the table never held".to_string(),
            "GetUnfilteredPartitionMetadata",
            one_partition_body(DATABASE, TABLE, &[ABSENT_VALUE], json!({})),
        ),
        (
            "a table the database does not hold".to_string(),
            "GetUnfilteredPartitionMetadata",
            one_partition_body(DATABASE, "no_such_table", &[STORED_VALUE], json!({})),
        ),
        (
            "a database the catalog does not hold".to_string(),
            "GetUnfilteredPartitionMetadata",
            one_partition_body("no_such_db", TABLE, &[STORED_VALUE], json!({})),
        ),
        (
            "more values than the table has partition keys".to_string(),
            "GetUnfilteredPartitionMetadata",
            one_partition_body(DATABASE, TABLE, &[STORED_VALUE, "us-east-1"], json!({})),
        ),
        (
            "a call carrying a vended-session context and an audit context".to_string(),
            "GetUnfilteredPartitionMetadata",
            one_partition_body(
                DATABASE,
                TABLE,
                &[STORED_VALUE],
                json!({
                    "QuerySessionContext": {
                        "QueryId": "11111111111111111111111111111111",
                        "QueryAuthorizationId": "22222222222222222222222222222222",
                        "ClusterId": "cluster-1",
                        "AdditionalContext": { "engine": "test" },
                    },
                    "AuditContext": { "AdditionalAuditContext": "context" },
                    "Region": "us-east-1",
                }),
            ),
        ),
        (
            "the list read of a table the catalog holds".to_string(),
            "GetUnfilteredPartitionsMetadata",
            many_partitions_body(DATABASE, TABLE, json!({})),
        ),
        (
            "the list read of a table the catalog does not hold".to_string(),
            "GetUnfilteredPartitionsMetadata",
            many_partitions_body(DATABASE, "no_such_table", json!({})),
        ),
        (
            "the list read with an expression, a page size and a session context".to_string(),
            "GetUnfilteredPartitionsMetadata",
            many_partitions_body(
                DATABASE,
                TABLE,
                json!({
                    "Expression": "dt = '2024-01-01'",
                    "MaxResults": 1,
                    "QuerySessionContext": {
                        "QueryAuthorizationId": "33333333333333333333333333333333",
                    },
                }),
            ),
        ),
    ];

    // The permission types a call may ask for change nothing about the gate: each value
    // of the modelled enum is refused the same way, on both operations.
    for permission_type in PERMISSION_TYPES {
        denied.push((
            format!("the single read asking for {permission_type}"),
            "GetUnfilteredPartitionMetadata",
            one_partition_body(
                DATABASE,
                TABLE,
                &[STORED_VALUE],
                json!({ "SupportedPermissionTypes": [permission_type] }),
            ),
        ));
        denied.push((
            format!("the list read asking for {permission_type}"),
            "GetUnfilteredPartitionsMetadata",
            many_partitions_body(
                DATABASE,
                TABLE,
                json!({ "SupportedPermissionTypes": [permission_type] }),
            ),
        ));
    }

    for (case, operation, body) in denied {
        assert_denied(&router, &case, operation, &body).await;
    }
}

/// The denied reads change nothing: the ungated `GetPartition` reports the stored
/// partition and refuses the absent one with its own modelled miss, both before and after
/// them.
#[tokio::test]
async fn a_denied_read_leaves_the_catalog_as_it_was() {
    let router = emulator();
    seed(&router).await;

    let before = partition_reads(&router).await;

    for (case, operation, body) in [
        (
            "the stored partition",
            "GetUnfilteredPartitionMetadata",
            one_partition_body(DATABASE, TABLE, &[STORED_VALUE], json!({})),
        ),
        (
            "the absent partition",
            "GetUnfilteredPartitionMetadata",
            one_partition_body(DATABASE, TABLE, &[ABSENT_VALUE], json!({})),
        ),
        (
            "the whole table",
            "GetUnfilteredPartitionsMetadata",
            many_partitions_body(DATABASE, TABLE, json!({})),
        ),
    ] {
        assert_denied(&router, case, operation, &body).await;
    }

    let after = partition_reads(&router).await;
    assert_eq!(
        before, after,
        "case {:?}: a denied read writes nothing, so the ungated reads answer the same before \
         and after; got {after:?} after {before:?}",
        "the ungated reads around the denied ones"
    );
}

/// What the ungated `GetPartition` answers for the stored and the absent values: the
/// status and the `Values` (or error code) of each, which a denied read must not change.
async fn partition_reads(router: &MultiServiceRouter) -> Vec<(u16, String)> {
    let mut answers = Vec::new();
    for values in [STORED_VALUE, ABSENT_VALUE] {
        let (status, parsed) = get_partition(router, DATABASE, TABLE, &[values]).await;
        let detail = if status == 200 {
            parsed["Partition"]["Values"].to_string()
        } else {
            format!("{} {}", error_code(&parsed), error_message(&parsed))
        };
        answers.push((status, detail));
    }
    answers
}

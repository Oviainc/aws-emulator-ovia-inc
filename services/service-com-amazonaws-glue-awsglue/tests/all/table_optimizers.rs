// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue table-optimizer slice: `CreateTableOptimizer`,
//! `GetTableOptimizer` and `DeleteTableOptimizer`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: an optimizer is a resource OF a table, addressed by that table and
//! by its `TableOptimizerType`. So it can only be registered on a table that exists — a
//! database or a table the catalog does not hold refuses the create with
//! `EntityNotFoundException` and persists nothing — and only on a table whose type can be
//! optimized, which is an open-table-format table (the ineligible table is refused with
//! `InvalidInputException`, which [`crate::error_classification`] covers). A table carries
//! at most one optimizer per type, a read reports the configuration the create wrote
//! together with the resource it belongs to, and a delete returns the table to the state
//! where that type is a miss.
//!
//! The rows below differ along what the rule generalises over: which optimizer type is in
//! play (every value of the modelled enum), which part of the address is missing, and which
//! members the submitted `TableOptimizerConfiguration` carries.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::TABLE_OPTIMIZER_ALREADY_EXISTS;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator, error_code, ok, ACCOUNT_ID};

/// Every value of the modelled `TableOptimizerType` enum, which `Type` is one of. Source:
/// `smithy_query shape=TableOptimizerType`.
const OPTIMIZER_TYPES: [&str; 3] = ["compaction", "retention", "orphan_file_deletion"];

/// The role an optimizer configuration passes to the service.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/glue-table-optimizer";

/// Create `database` holding `table`, as an open-table-format table: only such a table can
/// carry an optimizer, because an optimizer works on the data files of one.
async fn seed(router: &MultiServiceRouter, database: &str, table: &str) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": {
                "Name": table,
                "StorageDescriptor": { "Location": "s3://lake/events/" }
            },
            "OpenTableFormatInput": { "IcebergInput": { "MetadataOperation": "CREATE" } }
        }),
    )
    .await;
}

/// One optimizer read, answering the HTTP status and body.
async fn get_optimizer(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    optimizer_type: &str,
) -> (u16, Value) {
    call(
        router,
        "GetTableOptimizer",
        &json!({
            "CatalogId": ACCOUNT_ID,
            "DatabaseName": database,
            "TableName": table,
            "Type": optimizer_type
        }),
    )
    .await
}

// ---------------------------------------------------------------------------
// The optimizer's table has to exist
// ---------------------------------------------------------------------------

/// A create whose address does not resolve to a stored table is refused with
/// `EntityNotFoundException` and persists NOTHING, so the read that follows is the same
/// miss.
///
/// The rows differ along which part of the address is missing (the database, the table) and
/// along the optimizer type asked for — every value of the modelled enum.
#[tokio::test]
async fn an_optimizer_on_a_table_that_does_not_exist_is_not_found() {
    let router = emulator();
    seed(&router, "optimizer_db", "events").await;

    // (case, the database, the table)
    let mut missing: Vec<(String, &str, &str)> = Vec::new();
    for optimizer_type in OPTIMIZER_TYPES {
        missing.push((
            format!("a table the database does not hold, type {optimizer_type}"),
            "optimizer_db",
            "no_such_table",
        ));
        missing.push((
            format!("a database the catalog does not hold, type {optimizer_type}"),
            "no_such_db",
            "events",
        ));
    }

    for (index, (case, database, table)) in missing.iter().enumerate() {
        let optimizer_type = OPTIMIZER_TYPES[index / 2];
        let (status, body) = call(
            &router,
            "CreateTableOptimizer",
            &json!({
                "CatalogId": ACCOUNT_ID,
                "DatabaseName": database,
                "TableName": table,
                "Type": optimizer_type,
                "TableOptimizerConfiguration": { "roleArn": ROLE_ARN, "enabled": true }
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: an optimizer needs a stored table, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {} in {body}",
            error_code(&body)
        );

        let (status, read) = get_optimizer(&router, database, table, optimizer_type).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused create persisted nothing, so the read is a miss too, \
             got {status} with {read}"
        );
        assert!(
            error_code(&read).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException from the read, got {}",
            error_code(&read)
        );
    }
}

// ---------------------------------------------------------------------------
// create -> read -> delete -> miss
// ---------------------------------------------------------------------------

/// An optimizer the create registered is read back with the configuration it was created
/// with, attributed to the catalog, database and table it belongs to — and a delete returns
/// that type to being a miss.
///
/// The rows differ along the optimizer type (every value of the modelled enum) and along
/// which members the configuration carries: the role and the enabled flag alone, a
/// compaction configuration, a retention configuration, an orphan-file-deletion
/// configuration, and a VPC configuration.
#[tokio::test]
async fn the_optimizer_lifecycle_is_create_read_delete_then_a_miss() {
    // (case, the type, the configuration)
    let cases: [(&str, &str, Value); 5] = [
        (
            "compaction with the role and the enabled flag",
            "compaction",
            json!({ "roleArn": ROLE_ARN, "enabled": true }),
        ),
        (
            "compaction with an Iceberg compaction configuration",
            "compaction",
            json!({
                "roleArn": ROLE_ARN,
                "enabled": true,
                "compactionConfiguration": {
                    "icebergConfiguration": {
                        "strategy": "sort",
                        "minInputFiles": 5,
                        "deleteFileThreshold": 10
                    }
                }
            }),
        ),
        (
            "retention with an Iceberg retention configuration",
            "retention",
            json!({
                "roleArn": ROLE_ARN,
                "enabled": false,
                "retentionConfiguration": {
                    "icebergConfiguration": {
                        "snapshotRetentionPeriodInDays": 7,
                        "numberOfSnapshotsToRetain": 3,
                        "cleanExpiredFiles": true,
                        "runRateInHours": 24
                    }
                }
            }),
        ),
        (
            "orphan file deletion with its Iceberg configuration",
            "orphan_file_deletion",
            json!({
                "roleArn": ROLE_ARN,
                "enabled": true,
                "orphanFileDeletionConfiguration": {
                    "icebergConfiguration": {
                        "orphanFileRetentionPeriodInDays": 2,
                        "location": "s3://lake/events/data/",
                        "runRateInHours": 12
                    }
                }
            }),
        ),
        (
            "compaction in a customer VPC",
            "compaction",
            json!({
                "roleArn": ROLE_ARN,
                "enabled": true,
                "vpcConfiguration": { "glueConnectionName": "lake-vpc" }
            }),
        ),
    ];

    for (index, (case, optimizer_type, configuration)) in cases.iter().enumerate() {
        let router = emulator();
        let database = format!("lifecycle_db_{index}");
        let table = format!("lifecycle_table_{index}");
        seed(&router, &database, &table).await;

        ok(
            &router,
            "CreateTableOptimizer",
            &json!({
                "CatalogId": ACCOUNT_ID,
                "DatabaseName": database,
                "TableName": table,
                "Type": optimizer_type,
                "TableOptimizerConfiguration": configuration
            }),
        )
        .await;

        let (status, read) = get_optimizer(&router, &database, &table, optimizer_type).await;
        assert_eq!(
            status, 200,
            "case {case}: the registered optimizer must be readable, got {status} with {read}"
        );
        assert_eq!(
            read["CatalogId"].as_str(),
            Some(ACCOUNT_ID),
            "case {case}: the read must attribute the optimizer to its catalog, got {read}"
        );
        assert_eq!(
            read["DatabaseName"].as_str(),
            Some(database.as_str()),
            "case {case}: the read must attribute the optimizer to its database, got {read}"
        );
        assert_eq!(
            read["TableName"].as_str(),
            Some(table.as_str()),
            "case {case}: the read must attribute the optimizer to its table, got {read}"
        );
        assert_eq!(
            read["TableOptimizer"]["type"].as_str(),
            Some(*optimizer_type),
            "case {case}: the read must report the type it was created under, got {read}"
        );
        assert_echoed(
            case,
            configuration,
            &read["TableOptimizer"]["configuration"],
        );
        assert!(
            read["TableOptimizer"]["lastRun"].is_null(),
            "case {case}: nothing ran the optimizer, so no run may be reported, got {read}"
        );
        // Another type of the same table is a miss: an optimizer is addressed by its type.
        for other in OPTIMIZER_TYPES
            .iter()
            .filter(|other| *other != optimizer_type)
        {
            let (status, read) = get_optimizer(&router, &database, &table, other).await;
            assert_eq!(
                status, 400,
                "case {case}: no optimizer of type {other} was created, got {status} with {read}"
            );
        }

        ok(
            &router,
            "DeleteTableOptimizer",
            &json!({
                "CatalogId": ACCOUNT_ID,
                "DatabaseName": database,
                "TableName": table,
                "Type": optimizer_type
            }),
        )
        .await;
        let (status, read) = get_optimizer(&router, &database, &table, optimizer_type).await;
        assert_eq!(
            status, 400,
            "case {case}: the deleted optimizer must be a miss, got {status} with {read}"
        );
        assert!(
            error_code(&read).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException after the delete, got {}",
            error_code(&read)
        );
        // Deleting what is no longer there is that same miss.
        let (status, body) = call(
            &router,
            "DeleteTableOptimizer",
            &json!({
                "CatalogId": ACCOUNT_ID,
                "DatabaseName": database,
                "TableName": table,
                "Type": optimizer_type
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a second delete must be refused, got {status} with {body}"
        );
        // The table it belonged to is untouched by any of this.
        let table_read = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": database, "Name": table }),
        )
        .await;
        assert_eq!(
            table_read["Table"]["Name"].as_str(),
            Some(table.as_str()),
            "case {case}: the optimizer's table must still be readable, got {table_read}"
        );
    }
}

// ---------------------------------------------------------------------------
// One optimizer per type per table
// ---------------------------------------------------------------------------

/// A second create of the SAME type on one table is refused with `AlreadyExistsException`
/// and leaves the stored configuration exactly as the first create wrote it.
///
/// The rows differ along the optimizer type, so the one-per-type rule is asserted for each
/// value of the modelled enum.
#[tokio::test]
async fn a_second_optimizer_of_one_type_is_refused_and_overwrites_nothing() {
    for optimizer_type in OPTIMIZER_TYPES {
        let router = emulator();
        seed(&router, "duplicate_db", "events").await;
        let first = json!({ "roleArn": ROLE_ARN, "enabled": true });
        ok(
            &router,
            "CreateTableOptimizer",
            &json!({
                "CatalogId": ACCOUNT_ID,
                "DatabaseName": "duplicate_db",
                "TableName": "events",
                "Type": optimizer_type,
                "TableOptimizerConfiguration": first
            }),
        )
        .await;
        let stored = get_optimizer(&router, "duplicate_db", "events", optimizer_type)
            .await
            .1;

        let (status, body) = call(
            &router,
            "CreateTableOptimizer",
            &json!({
                "CatalogId": ACCOUNT_ID,
                "DatabaseName": "duplicate_db",
                "TableName": "events",
                "Type": optimizer_type,
                "TableOptimizerConfiguration": {
                    "roleArn": "arn:aws:iam::000000000000:role/another",
                    "enabled": false
                }
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {optimizer_type}: a second create must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("AlreadyExistsException"),
            "case {optimizer_type}: expected AlreadyExistsException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            crate::wire::error_message(&body),
            TABLE_OPTIMIZER_ALREADY_EXISTS,
            "case {optimizer_type}: the duplicate refusal must say the optimizer exists, got \
             {body}"
        );

        let after = get_optimizer(&router, "duplicate_db", "events", optimizer_type)
            .await
            .1;
        assert_eq!(
            after, stored,
            "case {optimizer_type}: a refused create must not overwrite the stored \
             configuration; expected {stored}, got {after}"
        );
    }
}

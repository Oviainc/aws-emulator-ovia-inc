// SPDX-License-Identifier: CC0-1.0

//! The identity and attribution members every Data Catalog read reports, and the
//! `__type` discriminator the catalog writes onto the nested structures it tags.
//!
//! Three rules are under test, each over a TABLE of inputs rather than one recorded
//! case:
//!
//! 1. Every database and table the catalog returns is attributed to the catalog that
//!    owns it (`CatalogIdentifier`) and to the catalog-assigned id of the database
//!    (`DatabaseId`), reported definitely on every read path that returns the entity —
//!    `GetDatabase`, `GetTable` and every element of a `GetTables` page — and a table's
//!    `DatabaseId` is the id its own database's read reports. A table also reports
//!    `IsRowFilteringEnabled` as an explicit boolean.
//! 2. A nested structure the catalog tags carries a `__type` equal to its Smithy shape's
//!    local name: `PrincipalPermissions` inside `Database$CreateTableDefaultPermissions`
//!    and `ErrorDetail` inside `BatchDeleteTable`'s `Errors`.
//! 3. `BatchDeleteTable`'s per-item error message is built from the one name the request
//!    asked for: `Table <Name> not found`, naming no database.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness.

use std::collections::HashSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{emulator, ok, ACCOUNT_ID, ERROR_DETAIL_TAG, PRINCIPAL_PERMISSIONS_TAG};

/// How many characters a catalog-assigned `DatabaseId` has, and the alphabet it draws
/// from: 32 lowercase hexadecimal characters.
///
/// Source: the `DatabaseId` member of `com.amazonaws.glue#Database` in this crate's
/// model, documented as "a 32-character lowercase hexadecimal string".
const DATABASE_ID_LEN: usize = 32;

/// Whether `id` is a well-formed catalog-assigned id: [`DATABASE_ID_LEN`] characters,
/// every one a lowercase hexadecimal digit.
fn is_catalog_assigned_id(id: &str) -> bool {
    id.len() == DATABASE_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// The `Database` body `GetDatabase` reports.
async fn get_database(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "GetDatabase", &json!({ "Name": name })).await["Database"].clone()
}

/// The `Table` body `GetTable` reports.
async fn get_table(router: &MultiServiceRouter, database: &str, name: &str) -> Value {
    ok(
        router,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": name }),
    )
    .await["Table"]
        .clone()
}

/// Every element of one `GetTables` page, at the modelled maximum page size.
async fn list_tables(router: &MultiServiceRouter, database: &str) -> Vec<Value> {
    let page = ok(
        router,
        "GetTables",
        &json!({ "DatabaseName": database, "MaxResults": 100 }),
    )
    .await;
    page["TableList"]
        .as_array()
        .expect("TableList is a list")
        .clone()
}

/// Assert the identity members of a table body against the database that holds it.
fn assert_table_identity(case: &str, table: &Value, database_id: &str) {
    assert_eq!(
        table["CatalogIdentifier"],
        json!(ACCOUNT_ID),
        "case {case}: Table.CatalogIdentifier is the owning catalog's id, got {:?}",
        table["CatalogIdentifier"]
    );
    assert_eq!(
        table["DatabaseId"],
        json!(database_id),
        "case {case}: Table.DatabaseId is the containing database's catalog-assigned id \
         {database_id}, got {:?}",
        table["DatabaseId"]
    );
    assert_eq!(
        table["IsRowFilteringEnabled"],
        json!(false),
        "case {case}: a table with no row-level filter reports IsRowFilteringEnabled \
         false, not an absent member, got {:?}",
        table["IsRowFilteringEnabled"]
    );
}

// ---------------------------------------------------------------------------
// Rule 1: the identity members of a database and of its tables
// ---------------------------------------------------------------------------

/// `GetDatabase` attributes every database to its catalog and to the id the catalog
/// assigned it, and the id is well formed.
///
/// The rows differ along the member set the create sent, the casing of the name, and
/// whether the request named the `CatalogId` explicitly or let it default — none of
/// which the rule depends on.
#[tokio::test]
async fn every_database_read_reports_its_catalog_and_its_catalog_assigned_id() {
    let router = emulator();

    // (case, DatabaseInput, the GetDatabase request's extra members)
    let cases: Vec<(&str, Value, Value)> = vec![
        ("the bare required name", json!({ "Name": "ident_bare_db" }), json!({})),
        (
            "a fully described database",
            json!({
                "Name": "ident_rich_db",
                "Description": "every member set",
                "LocationUri": "s3://bucket/ident/",
                "Parameters": { "tier": "gold", "owner": "data-eng" }
            }),
            json!({}),
        ),
        (
            "a mixed-case name, folded on store",
            json!({ "Name": "Ident_Mixed_DB" }),
            json!({}),
        ),
        (
            "the catalog id named explicitly",
            json!({ "Name": "ident_explicit_catalog_db" }),
            json!({ "CatalogId": ACCOUNT_ID }),
        ),
        (
            "a database carrying its own grant list",
            json!({
                "Name": "ident_granted_db",
                "CreateTableDefaultPermissions": [
                    {
                        "Principal": { "DataLakePrincipalIdentifier": "arn:aws:iam::000000000000:role/analyst" },
                        "Permissions": [ "SELECT" ]
                    }
                ]
            }),
            json!({}),
        ),
    ];

    let mut seen_ids: HashSet<String> = HashSet::new();

    for (case, database_input, extra) in &cases {
        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": database_input }),
        )
        .await;

        let name = database_input["Name"]
            .as_str()
            .expect("the input names the database")
            .to_ascii_lowercase();
        let mut request = json!({ "Name": name });
        for (member, value) in extra.as_object().expect("the extra members are a structure") {
            request[member] = value.clone();
        }
        let database = ok(&router, "GetDatabase", &request).await["Database"].clone();

        assert_eq!(
            database["CatalogIdentifier"],
            json!(ACCOUNT_ID),
            "case {case}: Database.CatalogIdentifier is the owning catalog's id, got {:?}",
            database["CatalogIdentifier"]
        );
        let id = database["DatabaseId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: Database.DatabaseId must be a string, got {database}")
            })
            .to_string();
        assert!(
            is_catalog_assigned_id(&id),
            "case {case}: expected a {DATABASE_ID_LEN}-character lowercase hex DatabaseId, \
             got {id:?}"
        );
        assert!(
            seen_ids.insert(id.clone()),
            "case {case}: the catalog assigns each database its own id, but {id:?} was already \
             handed out"
        );

        // Assigned at create and stored, so a second read answers the same id rather
        // than minting one per call.
        let again = get_database(&router, &name).await;
        assert_eq!(
            again["DatabaseId"],
            json!(id),
            "case {case}: two reads of one database must report the same DatabaseId, got {:?} \
             then {:?}",
            id,
            again["DatabaseId"]
        );
    }
}

/// `GetTable` and every element of a `GetTables` page carry the catalog identity, the
/// containing database's id and the row-filtering flag — and the `DatabaseId` is the one
/// `GetDatabase` reported for that database a moment earlier.
///
/// The rows differ along how many tables the database holds (so the per-element rule is
/// exercised on a one-, two- and three-element page) and along the member set each table
/// was created with.
#[tokio::test]
async fn every_table_read_agrees_with_its_database_on_the_catalog_identity() {
    let router = emulator();

    for table_count in [1_usize, 2, 3] {
        let case = format!("{table_count} table(s)");
        let database = format!("ident_tables_db_{table_count}");
        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": { "Name": database } }),
        )
        .await;
        let database_id = get_database(&router, &database).await["DatabaseId"]
            .as_str()
            .expect("Database.DatabaseId is a string")
            .to_string();

        let mut names: Vec<String> = Vec::new();
        for index in 0..table_count {
            // The member set differs per table so the rule is not read off one shape.
            let table_input = match index % 3 {
                0 => json!({ "Name": format!("ident_tbl_{index}") }),
                1 => json!({
                    "Name": format!("ident_tbl_{index}"),
                    "TableType": "EXTERNAL_TABLE",
                    "Owner": "data-eng",
                    "StorageDescriptor": {
                        "Columns": [ { "Name": "id", "Type": "string" } ],
                        "Location": format!("s3://bucket/{database}/{index}/")
                    }
                }),
                _ => json!({
                    "Name": format!("ident_tbl_{index}"),
                    "TableType": "GOVERNED",
                    "PartitionKeys": [ { "Name": "dt", "Type": "string" } ],
                    "Parameters": { "classification": "parquet" }
                }),
            };
            ok(
                &router,
                "CreateTable",
                &json!({ "DatabaseName": database, "TableInput": table_input }),
            )
            .await;
            names.push(
                table_input["Name"]
                    .as_str()
                    .expect("the input names the table")
                    .to_string(),
            );
        }

        for name in &names {
            let table = get_table(&router, &database, name).await;
            assert_table_identity(&format!("{case} / GetTable {name}"), &table, &database_id);
        }

        let listed = list_tables(&router, &database).await;
        assert_eq!(
            listed.len(),
            table_count,
            "case {case}: every stored table must be listed, got {listed:?}"
        );
        for element in &listed {
            let name = element["Name"].as_str().unwrap_or("<unnamed>");
            assert_table_identity(
                &format!("{case} / TableList element {name}"),
                element,
                &database_id,
            );
            // The list projection IS the single-table projection, so the element must
            // equal what `GetTable` answers for the same table.
            let single = get_table(&router, &database, name).await;
            assert_eq!(
                element, &single,
                "case {case}: the TableList element for {name} must be the GetTable \
                 projection, got {element:?} against {single:?}"
            );
        }
    }
}

/// The catalog-assigned id identifies a database rather than naming it: renaming the
/// database through `UpdateDatabase` leaves the id alone, and its tables keep reporting
/// it.
#[tokio::test]
async fn renaming_a_database_does_not_change_its_catalog_assigned_id() {
    // (case, the name created, the name the update submits)
    let cases: Vec<(&str, &str, &str)> = vec![
        ("renamed to another name", "ident_rename_before", "ident_rename_after"),
        ("rewritten under the same name", "ident_same_name", "ident_same_name"),
        ("renamed with mixed casing", "ident_case_before", "Ident_Case_AFTER"),
    ];

    for (case, created, submitted) in &cases {
        let router = emulator();
        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": { "Name": created } }),
        )
        .await;
        ok(
            &router,
            "CreateTable",
            &json!({ "DatabaseName": created, "TableInput": { "Name": "ident_tbl" } }),
        )
        .await;
        let before = get_database(&router, created).await["DatabaseId"]
            .as_str()
            .expect("Database.DatabaseId is a string")
            .to_string();

        ok(
            &router,
            "UpdateDatabase",
            &json!({ "Name": created, "DatabaseInput": { "Name": submitted } }),
        )
        .await;

        let folded = submitted.to_ascii_lowercase();
        let after = get_database(&router, &folded).await;
        assert_eq!(
            after["DatabaseId"],
            json!(before),
            "case {case}: the catalog-assigned id identifies the database, so a rename must \
             not change it; expected {before}, got {:?}",
            after["DatabaseId"]
        );
        let table = get_table(&router, &folded, "ident_tbl").await;
        assert_table_identity(case, &table, &before);
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the `__type` discriminator on the nested structures the catalog tags
// ---------------------------------------------------------------------------

/// Every element of a database's `CreateTableDefaultPermissions` carries the
/// `PrincipalPermissions` discriminator, whether the grants came from the catalog's
/// default or from the client — and whether or not the client sent a tag of its own.
///
/// The rows differ along the grant list sent: absent (so the catalog's own default is
/// reported), explicitly empty, one grant, two grants, and a grant the client already
/// tagged.
#[tokio::test]
async fn every_nested_grant_carries_the_principal_permissions_discriminator() {
    let router = emulator();

    let analyst = "arn:aws:iam::000000000000:role/analyst";
    let engineer = "arn:aws:iam::000000000000:role/engineer";

    // (case, DatabaseInput, how many grants the read must report)
    let cases: Vec<(&str, Value, usize)> = vec![
        ("no grants sent", json!({ "Name": "tag_default_db" }), 1),
        (
            "explicitly empty grant list",
            json!({ "Name": "tag_empty_db", "CreateTableDefaultPermissions": [] }),
            0,
        ),
        (
            "one grant",
            json!({
                "Name": "tag_one_db",
                "CreateTableDefaultPermissions": [
                    { "Principal": { "DataLakePrincipalIdentifier": analyst }, "Permissions": [ "SELECT" ] }
                ]
            }),
            1,
        ),
        (
            "two grants",
            json!({
                "Name": "tag_two_db",
                "CreateTableDefaultPermissions": [
                    { "Principal": { "DataLakePrincipalIdentifier": analyst }, "Permissions": [ "SELECT" ] },
                    { "Principal": { "DataLakePrincipalIdentifier": engineer }, "Permissions": [ "ALL", "ALTER" ] }
                ]
            }),
            2,
        ),
        (
            // The tag belongs to the catalog's projection, not to the record: a client
            // that sends one of its own gets the catalog's back, not its own echoed.
            "a grant the client already tagged",
            json!({
                "Name": "tag_echoed_db",
                "CreateTableDefaultPermissions": [
                    {
                        "__type": "SomethingElse",
                        "Principal": { "DataLakePrincipalIdentifier": engineer },
                        "Permissions": [ "DROP" ]
                    }
                ]
            }),
            1,
        ),
    ];

    for (case, database_input, expected_grants) in &cases {
        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": database_input }),
        )
        .await;
        let name = database_input["Name"]
            .as_str()
            .expect("the input names the database");
        let database = get_database(&router, name).await;

        let grants = database["CreateTableDefaultPermissions"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: CreateTableDefaultPermissions is a list, got {database}")
            })
            .clone();
        assert_eq!(
            grants.len(),
            *expected_grants,
            "case {case}: expected {expected_grants} grant(s), got {grants:?}"
        );
        for grant in &grants {
            assert_eq!(
                grant["__type"],
                json!(PRINCIPAL_PERMISSIONS_TAG),
                "case {case}: a nested grant must carry __type {PRINCIPAL_PERMISSIONS_TAG}, \
                 got {grant:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rules 2 and 3: the per-item error of a partial batch delete
// ---------------------------------------------------------------------------

/// One row of the partial-batch-delete table: the case name, the database, the tables
/// created in it, the names the batch deletes, and the names whose delete must fail.
type BatchDeleteRow = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
);

/// A `BatchDeleteTable` entry the database does not hold is reported with an
/// `ErrorDetail` that carries the `ErrorDetail` discriminator and the message the
/// catalog builds from the requested name alone: `Table <Name> not found`, with no
/// database clause.
///
/// The rows differ along how many names are missing and where they sit in the list, the
/// casing of a missing name, and the database the batch addresses — the message is built
/// from the one name, so none of those may appear in it.
#[tokio::test]
async fn a_missing_batch_delete_entry_is_tagged_and_named_by_the_requested_table() {
    let cases: Vec<BatchDeleteRow> = vec![
        ("one name missing", "err_db_one", &["events"], &["events", "absent"], &["absent"]),
        (
            "every name missing",
            "err_db_all",
            &["events"],
            &["ghost_one", "ghost_two"],
            &["ghost_one", "ghost_two"],
        ),
        (
            "a missing name between two present ones",
            "err_db_middle",
            &["clicks", "orders"],
            &["clicks", "absent", "orders"],
            &["absent"],
        ),
        (
            // The reported name is the folded one, as `TableError$TableName` documents,
            // and the message names exactly that.
            "a missing name in mixed casing",
            "err_db_casing",
            &["events"],
            &["Missing_Table"],
            &["missing_table"],
        ),
        (
            "an empty database, every name missing",
            "err_db_empty",
            &[],
            &["only_ghost"],
            &["only_ghost"],
        ),
    ];

    for (case, database, created, to_delete, expected_failures) in &cases {
        let router = emulator();
        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": { "Name": database } }),
        )
        .await;
        for table in *created {
            ok(
                &router,
                "CreateTable",
                &json!({ "DatabaseName": database, "TableInput": { "Name": table } }),
            )
            .await;
        }

        let answer = ok(
            &router,
            "BatchDeleteTable",
            &json!({ "DatabaseName": database, "TablesToDelete": to_delete }),
        )
        .await;
        let errors = answer["Errors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: Errors is a list, got {answer}"))
            .clone();
        assert_eq!(
            errors.len(),
            expected_failures.len(),
            "case {case}: expected {} per-item error(s), got {errors:?}",
            expected_failures.len()
        );

        for (entry, expected_name) in errors.iter().zip(expected_failures.iter()) {
            let detail = &entry["ErrorDetail"];
            assert_eq!(
                detail["__type"],
                json!(ERROR_DETAIL_TAG),
                "case {case}: a per-item ErrorDetail must carry __type {ERROR_DETAIL_TAG}, \
                 got {entry:?}"
            );
            assert_eq!(
                detail["ErrorCode"],
                json!("EntityNotFoundException"),
                "case {case}: a missing table's ErrorCode is EntityNotFoundException, got \
                 {entry:?}"
            );
            assert_eq!(
                detail["ErrorMessage"],
                json!(format!("Table {expected_name} not found")),
                "case {case}: expected the message \"Table {expected_name} not found\", got \
                 {:?}",
                detail["ErrorMessage"]
            );
            let message = detail["ErrorMessage"].as_str().unwrap_or_default();
            assert!(
                !message.contains(database),
                "case {case}: the per-item message is built from the table name alone, so it \
                 must not name the database {database}, got {message:?}"
            );
        }
    }
}

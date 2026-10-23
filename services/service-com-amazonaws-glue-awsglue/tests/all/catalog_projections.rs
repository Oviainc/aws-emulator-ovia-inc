// SPDX-License-Identifier: CC0-1.0

//! What the Data Catalog's READ projections report beyond the members a client sent.
//!
//! `GetDatabase`, `GetTable` and `GetTables` serialize members the catalog itself
//! determines — the Lake Formation default grant of a database created without one, the
//! three-state view flags, the sort order of a storage descriptor, the id of the version
//! a create made, and the principal that made it. Each is reported DEFINITELY, so a
//! client never has to read an absent member as "unknown".
//!
//! Every suite here is a table: the rows differ along what the rule generalises over
//! (the grant list sent or not sent, the shape of the storage descriptor's sort order,
//! the table type, how many tables the listed database holds), and the recorded
//! single-table case is one row among them. Calls go over the wire through the shared
//! [`crate::wire`] harness.

use serde_json::{json, Value};

use crate::wire::{emulator, ok, ACCOUNT_ID, CALLER_PRINCIPAL, PRINCIPAL_PERMISSIONS_TAG};

/// `grants` as the catalog REPORTS them: every element carries the
/// [`PRINCIPAL_PERMISSIONS_TAG`] discriminator, whatever the client sent.
fn as_reported(grants: Value) -> Value {
    let tagged: Vec<Value> = grants
        .as_array()
        .expect("a grant list is a list")
        .iter()
        .map(|grant| {
            let mut grant = grant.clone();
            grant
                .as_object_mut()
                .expect("a grant is a structure")
                .insert("__type".to_string(), json!(PRINCIPAL_PERMISSIONS_TAG));
            grant
        })
        .collect();
    Value::Array(tagged)
}

/// The `CreateTableDefaultPermissions` a database carries when its create named none:
/// the single `ALL` grant to the Lake Formation `IAM_ALLOWED_PRINCIPALS` principal.
///
/// Source: `smithy_query shape=Database,PrincipalPermissions,Permission` on this crate's
/// model — `Database$CreateTableDefaultPermissions` targets a `PrincipalPermissionsList`
/// of `PrincipalPermissions{Principal: DataLakePrincipal, Permissions: PermissionList}`,
/// and `ALL` is one of the `Permission` enum's values.
fn iam_allowed_principals_grant() -> Value {
    as_reported(json!([
        {
            "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
            "Permissions": [ "ALL" ]
        }
    ]))
}

/// Create `database` and read it back.
async fn create_then_get_database(
    router: &smithy_server_runtime::MultiServiceRouter,
    database_input: &Value,
) -> Value {
    let name = database_input["Name"].as_str().expect("a database name");
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": database_input }),
    )
    .await;
    let fetched = ok(router, "GetDatabase", &json!({ "Name": name })).await;
    fetched["Database"].clone()
}

/// Create `table_input` in `database` and read it back through `GetTable`.
async fn create_then_get_table(
    router: &smithy_server_runtime::MultiServiceRouter,
    database: &str,
    table_input: &Value,
) -> Value {
    let name = table_input["Name"].as_str().expect("a table name");
    ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": table_input }),
    )
    .await;
    let fetched = ok(
        router,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": name }),
    )
    .await;
    fetched["Table"].clone()
}

// ---------------------------------------------------------------------------
// Database: the Lake Formation default grant
// ---------------------------------------------------------------------------

/// A database created without `CreateTableDefaultPermissions` reports the catalog's
/// default grant; one created WITH a list — the empty list included — reports exactly
/// that list. The two are different governance decisions, so the projection may not
/// collapse them, and the recorded value is set at create, not minted per read.
///
/// The rows differ along the grant list sent: absent (with and without other members
/// beside it), explicitly empty, one grant of one permission, one grant of several
/// permissions the issue never cited, and two grants.
#[tokio::test]
async fn a_databases_create_table_default_permissions_are_reported_on_every_read() {
    let router = emulator();

    let analyst = "arn:aws:iam::000000000000:role/analyst";
    let engineer = "arn:aws:iam::000000000000:role/engineer";

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "no grants sent",
            json!({ "Name": "grants_default_db" }),
            iam_allowed_principals_grant(),
        ),
        (
            "no grants sent, other members set",
            json!({
                "Name": "grants_default_rich_db",
                "Description": "a database nobody governed",
                "LocationUri": "s3://bucket/default/",
                "Parameters": { "owner": "data-eng" }
            }),
            iam_allowed_principals_grant(),
        ),
        (
            "explicitly empty grant list",
            json!({ "Name": "grants_empty_db", "CreateTableDefaultPermissions": [] }),
            as_reported(json!([])),
        ),
        (
            "one grant of one permission",
            json!({
                "Name": "grants_one_db",
                "CreateTableDefaultPermissions": [
                    {
                        "Principal": { "DataLakePrincipalIdentifier": analyst },
                        "Permissions": [ "SELECT" ]
                    }
                ]
            }),
            as_reported(json!([
                {
                    "Principal": { "DataLakePrincipalIdentifier": analyst },
                    "Permissions": [ "SELECT" ]
                }
            ])),
        ),
        (
            // The permissions here are `Permission` enum values the issue did not cite.
            "one grant of several permissions",
            json!({
                "Name": "grants_many_permissions_db",
                "CreateTableDefaultPermissions": [
                    {
                        "Principal": { "DataLakePrincipalIdentifier": engineer },
                        "Permissions": [ "CREATE_TABLE", "ALTER", "DATA_LOCATION_ACCESS" ]
                    }
                ]
            }),
            as_reported(json!([
                {
                    "Principal": { "DataLakePrincipalIdentifier": engineer },
                    "Permissions": [ "CREATE_TABLE", "ALTER", "DATA_LOCATION_ACCESS" ]
                }
            ])),
        ),
        (
            "two grants",
            json!({
                "Name": "grants_two_db",
                "CreateTableDefaultPermissions": [
                    {
                        "Principal": { "DataLakePrincipalIdentifier": analyst },
                        "Permissions": [ "SELECT" ]
                    },
                    {
                        "Principal": { "DataLakePrincipalIdentifier": engineer },
                        "Permissions": [ "ALL" ]
                    }
                ]
            }),
            as_reported(json!([
                {
                    "Principal": { "DataLakePrincipalIdentifier": analyst },
                    "Permissions": [ "SELECT" ]
                },
                {
                    "Principal": { "DataLakePrincipalIdentifier": engineer },
                    "Permissions": [ "ALL" ]
                }
            ])),
        ),
    ];

    for (case, database_input, expected) in &cases {
        let database = create_then_get_database(&router, database_input).await;
        assert_eq!(
            &database["CreateTableDefaultPermissions"], expected,
            "case {case}: expected CreateTableDefaultPermissions {expected}, got {:?}",
            database["CreateTableDefaultPermissions"]
        );
        assert_eq!(
            database["CatalogId"],
            json!(ACCOUNT_ID),
            "case {case}: Database.CatalogId is the catalog's account id, got {:?}",
            database["CatalogId"]
        );

        // Recorded at create, so a second read reports the same grants.
        let name = database_input["Name"].as_str().expect("a database name");
        let again = ok(&router, "GetDatabase", &json!({ "Name": name })).await;
        assert_eq!(
            again["Database"], database,
            "case {case}: two reads of one database must agree, got {:?} then {:?}",
            database, again["Database"]
        );
    }
}

// ---------------------------------------------------------------------------
// Table: the members the catalog determines
// ---------------------------------------------------------------------------

/// `GetTable` reports the catalog-determined members of a table definition: the first
/// version of its history, the principal that created it, the three-state view flags as
/// explicit booleans, and its storage descriptor's sort order as a list — empty when the
/// table defines no sort order, rather than omitted.
///
/// The rows differ along the storage descriptor sent: none at all, one without a sort
/// order, one with an explicitly empty sort order, one sorted column, two sorted columns
/// with both documented `Order$SortOrder` values (0 descending, 1 ascending), and a
/// bucketed `GOVERNED` table with no sort order.
#[tokio::test]
async fn the_table_projection_reports_the_members_the_catalog_determines() {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "projection_db" } }),
    )
    .await;

    // (case, TableInput, expected StorageDescriptor.SortColumns — None when the whole
    // storage descriptor is expected to stay absent)
    let cases: Vec<(&str, Value, Option<Value>)> = vec![
        (
            "no storage descriptor",
            json!({ "Name": "no_sd", "TableType": "EXTERNAL_TABLE" }),
            None,
        ),
        (
            "storage descriptor without a sort order",
            json!({
                "Name": "sd_without_sort",
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Columns": [ { "Name": "id", "Type": "string" } ],
                    "Location": "s3://bucket/sd_without_sort/"
                }
            }),
            Some(json!([])),
        ),
        (
            "explicitly empty sort order",
            json!({
                "Name": "sd_empty_sort",
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Columns": [ { "Name": "id", "Type": "string" } ],
                    "SortColumns": []
                }
            }),
            Some(json!([])),
        ),
        (
            "one sorted column",
            json!({
                "Name": "sd_one_sort",
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Columns": [ { "Name": "day", "Type": "date" } ],
                    "BucketColumns": [ "day" ],
                    "NumberOfBuckets": 4,
                    "SortColumns": [ { "Column": "day", "SortOrder": 1 } ]
                }
            }),
            Some(json!([ { "Column": "day", "SortOrder": 1 } ])),
        ),
        (
            "two sorted columns, descending then ascending",
            json!({
                "Name": "sd_two_sorts",
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Columns": [
                        { "Name": "day", "Type": "date" },
                        { "Name": "amount", "Type": "double" }
                    ],
                    "BucketColumns": [ "day", "amount" ],
                    "NumberOfBuckets": 8,
                    "SortColumns": [
                        { "Column": "day", "SortOrder": 0 },
                        { "Column": "amount", "SortOrder": 1 }
                    ]
                }
            }),
            Some(json!([
                { "Column": "day", "SortOrder": 0 },
                { "Column": "amount", "SortOrder": 1 }
            ])),
        ),
        (
            "bucketed governed table with no sort order",
            json!({
                "Name": "sd_governed",
                "TableType": "GOVERNED",
                "StorageDescriptor": {
                    "Columns": [ { "Name": "id", "Type": "string" } ],
                    "BucketColumns": [ "id" ],
                    "NumberOfBuckets": 2,
                    "Compressed": true
                }
            }),
            Some(json!([])),
        ),
    ];

    for (case, table_input, expected_sort_columns) in &cases {
        let table = create_then_get_table(&router, "projection_db", table_input).await;

        assert_eq!(
            table["VersionId"],
            json!("0"),
            "case {case}: the version a create makes is the first of the table's history, \
             \"0\", got {:?}",
            table["VersionId"]
        );
        assert_eq!(
            table["CreatedBy"],
            json!(CALLER_PRINCIPAL),
            "case {case}: Table.CreatedBy is the principal ARN the request speaks for, got {:?}",
            table["CreatedBy"]
        );
        assert_eq!(
            table["CatalogId"],
            json!(ACCOUNT_ID),
            "case {case}: Table.CatalogId is the catalog's account id, got {:?}",
            table["CatalogId"]
        );
        assert_eq!(
            table["DatabaseName"],
            json!("projection_db"),
            "case {case}: Table.DatabaseName names the owning database, got {:?}",
            table["DatabaseName"]
        );
        for flag in ["IsMultiDialectView", "IsMaterializedView"] {
            assert_eq!(
                table[flag],
                json!(false),
                "case {case}: {flag} must be an explicit false for a table that is no view, \
                 got {:?}",
                table[flag]
            );
        }
        assert_eq!(
            table["IsRegisteredWithLakeFormation"],
            json!(false),
            "case {case}: IsRegisteredWithLakeFormation must be an explicit false, got {:?}",
            table["IsRegisteredWithLakeFormation"]
        );
        assert!(
            table["ViewDefinition"].is_null(),
            "case {case}: a table that is no view carries no ViewDefinition, got {:?}",
            table["ViewDefinition"]
        );

        match expected_sort_columns {
            None => assert!(
                table["StorageDescriptor"].is_null(),
                "case {case}: a storage descriptor the client never sent stays absent, got {:?}",
                table["StorageDescriptor"]
            ),
            Some(expected) => assert_eq!(
                &table["StorageDescriptor"]["SortColumns"], expected,
                "case {case}: expected StorageDescriptor.SortColumns {expected}, got {:?}",
                table["StorageDescriptor"]["SortColumns"]
            ),
        }

        // The list read serves the SAME projection, element by element, not a reduced
        // one: the element for this table is identical to what `GetTable` answered.
        let name = table_input["Name"].as_str().expect("a table name");
        let page = ok(
            &router,
            "GetTables",
            &json!({ "DatabaseName": "projection_db" }),
        )
        .await;
        let listed = page["TableList"]
            .as_array()
            .expect("TableList is a list")
            .iter()
            .find(|entry| entry["Name"] == json!(name))
            .unwrap_or_else(|| panic!("case {case}: {name} must appear in TableList"));
        assert_eq!(
            listed, &table,
            "case {case}: the TableList element must be the GetTable projection, got {listed:?} \
             against {table:?}"
        );
    }
}

/// Every element of a `GetTables` page carries the full projection, whatever the page
/// holds: the defect is per element, so a two-table database must not hide it.
///
/// The rows differ along how many tables the listed database holds.
#[tokio::test]
async fn every_element_of_a_table_list_carries_the_full_projection() {
    let router = emulator();

    for table_count in [1_usize, 2, 3] {
        let case = format!("{table_count} table(s)");
        let database = format!("list_db_{table_count}");
        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": { "Name": database } }),
        )
        .await;

        let mut expected: Vec<Value> = Vec::new();
        for index in 0..table_count {
            let table_input = json!({
                "Name": format!("listed_table_{index}"),
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Columns": [ { "Name": "id", "Type": "string" } ],
                    "Location": format!("s3://bucket/{database}/{index}/")
                }
            });
            expected.push(create_then_get_table(&router, &database, &table_input).await);
        }

        let page = ok(&router, "GetTables", &json!({ "DatabaseName": database })).await;
        let listed = page["TableList"]
            .as_array()
            .expect("TableList is a list")
            .clone();
        assert_eq!(
            listed.len(),
            table_count,
            "case {case}: every stored table must be listed, got {listed:?}"
        );
        for (element, want) in listed.iter().zip(expected.iter()) {
            assert_eq!(
                element, want,
                "case {case}: a TableList element must be the GetTable projection, got \
                 {element:?} against {want:?}"
            );
            assert_eq!(
                element["VersionId"],
                json!("0"),
                "case {case}: a listed table that was never updated is version \"0\", got {:?}",
                element["VersionId"]
            );
            assert_eq!(
                element["CreatedBy"],
                json!(CALLER_PRINCIPAL),
                "case {case}: a listed table's CreatedBy is the caller principal, got {:?}",
                element["CreatedBy"]
            );
            assert_eq!(
                element["StorageDescriptor"]["SortColumns"],
                json!([]),
                "case {case}: a listed table with no sort order reports an empty list, got {:?}",
                element["StorageDescriptor"]["SortColumns"]
            );
            for flag in ["IsMultiDialectView", "IsMaterializedView"] {
                assert_eq!(
                    element[flag],
                    json!(false),
                    "case {case}: a listed table's {flag} is an explicit false, got {:?}",
                    element[flag]
                );
            }
        }
    }
}

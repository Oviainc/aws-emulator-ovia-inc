// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for what the CREATOR of a Data Catalog table holds on it.
//!
//! One rule, over a table of inputs rather than one request: creating a table in the Data
//! Catalog is itself what grants its creator Super on it, so Lake Formation reports the
//! creating principal as holding `ALL` and every individual table permission WITH the grant
//! option — at the two scopes a table's permissions are held at ("call this operation on
//! the table, and the operation returns the table and the table w columns" —
//! `ListPermissions$Resource`), so the table's own entry carries the table-scoped
//! permissions and its column-wildcard entry the column-scoped one. What the parent
//! database hands the tables created in it
//! (`Database$CreateTableDefaultPermissions`) neither adds to nor takes away from it; that
//! half of what a table is created carrying is the `table_default_grants.rs` suite beside
//! this one.
//!
//! The rows span what the rule generalises over: the `DatabaseInput` the create sent —
//! including both values `CreateTableDefaultPermissions` admits — the form of the table
//! name, how many tables the database holds, which filter the listing names, and which of
//! the creator's permissions a revoke takes away.
//!
//! Who the creator IS is asked of the Data Catalog (`Table$CreatedBy`) rather than spelled
//! here, so the assertions follow the emulator's own identity contract.

use serde_json::{json, Value};

use crate::glue_catalog::{
    account_catalog, create_database_with_input, create_table, emulator, error_code,
    lake_formation as call, list_permissions as list, permission_entries as entries,
    table_created_by, Entry,
};

/// The group a database's default table permissions name, spelled as Lake Formation spells
/// it on the wire.
const IAM_ALLOWED_PRINCIPALS: &str = "IAM_ALLOWED_PRINCIPALS";

/// What a table's creator holds on it: Lake Formation's `ALL` — "Super" — together with
/// every individual permission a table takes. The members are the values of the modelled
/// `Permission` enum (`smithy_query(query="shape",shape="Permission")`) that the published
/// Lake Formation permissions reference lists for a TABLE resource; every other value of
/// the enum names another scope.
pub const CREATOR_PERMISSIONS: [&str; 7] = [
    "ALL", "ALTER", "DELETE", "DESCRIBE", "DROP", "INSERT", "SELECT",
];

/// The one of those Lake Formation holds at COLUMN scope — the permission
/// `TableWithColumnsResource` exists to narrow, since that shape "is only used when
/// granting a SELECT permission" (`smithy_query(query="shape",shape="TableWithColumnsResource")`).
pub const COLUMN_SCOPED_PERMISSIONS: [&str; 1] = ["SELECT"];

/// The permissions of [`CREATOR_PERMISSIONS`] held on the table ITSELF: the ones that are
/// not column-scoped, sorted.
pub fn table_scoped() -> Vec<String> {
    let mut permissions: Vec<String> = CREATOR_PERMISSIONS
        .iter()
        .filter(|permission| !COLUMN_SCOPED_PERMISSIONS.contains(permission))
        .map(|permission| (*permission).to_string())
        .collect();
    permissions.sort();
    permissions
}

/// The column-scoped permissions of [`CREATOR_PERMISSIONS`], sorted.
pub fn column_scoped() -> Vec<String> {
    let mut permissions: Vec<String> = COLUMN_SCOPED_PERMISSIONS
        .iter()
        .map(|permission| (*permission).to_string())
        .collect();
    permissions.sort();
    permissions
}

/// A `DatabaseInput` that hands the tables created in it NO default permissions — one of
/// the two values the member admits ("The only permitted values are an empty array or an
/// array that contains a single JSON object that grants ALL to IAM_ALLOWED_PRINCIPALS").
pub fn governed_database(name: &str) -> Value {
    json!({ "Name": name, "CreateTableDefaultPermissions": [] })
}

/// The other value: the IAM-access-control default, which hands every IAM-allowed
/// principal `ALL` on the tables created in the database.
pub fn iam_access_control_database(name: &str) -> Value {
    json!({
        "Name": name,
        "CreateTableDefaultPermissions": [{
            "Principal": { "DataLakePrincipalIdentifier": IAM_ALLOWED_PRINCIPALS },
            "Permissions": ["ALL"],
        }],
    })
}

/// The `Table` resource a listing filters on.
pub fn table_filter(database: &str, table: &str) -> Value {
    json!({ "Table": { "DatabaseName": database, "Name": table } })
}

/// The `Table` resource a listing REPORTS: the catalog id the model documents a default for
/// ("By default, it is the account ID of the caller" — `TableResource$CatalogId`) and the
/// names folded the way the Data Catalog stores them.
pub fn reported_table(database: &str, table: &str) -> Value {
    json!({
        "Table": {
            "CatalogId": account_catalog(),
            "DatabaseName": database.to_lowercase(),
            "Name": table.to_lowercase(),
        }
    })
}

/// Its column-level record: every column of the table, named by the wildcard.
pub fn reported_columns(database: &str, table: &str) -> Value {
    json!({
        "TableWithColumns": {
            "CatalogId": account_catalog(),
            "DatabaseName": database.to_lowercase(),
            "Name": table.to_lowercase(),
            "ColumnWildcard": {},
        }
    })
}

/// The two entries the creator of `database`.`table` holds, in the same sorted form the
/// shared harness's `permission_entries` answers: the table's own permissions and its
/// column-wildcard one, each with the grant option.
pub fn creator_entries(creator: &str, database: &str, table: &str) -> Vec<Entry> {
    expected_entries(creator, database, table, &table_scoped(), &column_scoped())
}

/// The entries a principal holding `on_table` at table scope and `on_columns` at column
/// scope, in both lists, is reported with — a scope that holds nothing is no entry at all.
pub fn expected_entries(
    principal: &str,
    database: &str,
    table: &str,
    on_table: &[String],
    on_columns: &[String],
) -> Vec<Entry> {
    let mut expected: Vec<Entry> = Vec::new();
    if !on_table.is_empty() {
        expected.push((
            principal.to_string(),
            reported_table(database, table).to_string(),
            on_table.to_vec(),
            on_table.to_vec(),
        ));
    }
    if !on_columns.is_empty() {
        expected.push((
            principal.to_string(),
            reported_columns(database, table).to_string(),
            on_columns.to_vec(),
            on_columns.to_vec(),
        ));
    }
    expected.sort();
    expected
}

/// The entry the IAM-access-control table-level default is reported as: `ALL` on the table
/// itself, held by the group and passable to nobody.
pub fn iam_allowed_entry(database: &str, table: &str) -> Entry {
    (
        IAM_ALLOWED_PRINCIPALS.to_string(),
        reported_table(database, table).to_string(),
        vec!["ALL".to_string()],
        Vec::new(),
    )
}

/// The entries a listing of `database`.`table` must carry: the creator's two, plus the
/// IAM-access-control default when the database hands its tables one.
fn expected_table_entries(
    creator: &str,
    database: &str,
    table: &str,
    hands_iam_default: bool,
) -> Vec<Entry> {
    let mut expected = creator_entries(creator, database, table);
    if hands_iam_default {
        expected.push(iam_allowed_entry(database, table));
    }
    expected.sort();
    expected
}

#[tokio::test]
async fn a_created_table_is_held_by_its_creator_whatever_the_database_hands_its_tables() {
    // Rows differ along the `DatabaseInput` the create sent — both values
    // `CreateTableDefaultPermissions` admits and a create that names neither, which records
    // the catalog's own default — the form of the table name, and how many tables the
    // database holds. The fourth field is whether the database hands its tables the
    // IAM-access-control default, which is an entry of its own beside the creator's.
    let cases: Vec<(&str, Value, Vec<&str>, bool)> = vec![
        (
            "a database that hands its tables no defaults",
            governed_database("sales"),
            vec!["orders"],
            false,
        ),
        (
            "a database that hands its tables the IAM-access-control default",
            iam_access_control_database("sales"),
            vec!["orders"],
            true,
        ),
        (
            "a database whose create named no table-level defaults",
            json!({ "Name": "sales" }),
            vec!["orders"],
            true,
        ),
        (
            "a mixed-case table name",
            governed_database("sales"),
            vec!["Mixed_Case_Table"],
            false,
        ),
        (
            "several tables",
            governed_database("sales"),
            vec!["orders", "returns", "shipments"],
            false,
        ),
        (
            "a mixed-case database name",
            governed_database("Mixed_Case_DB"),
            vec!["orders"],
            false,
        ),
        (
            "a database that also carries a description and a location",
            json!({
                "Name": "warehouse",
                "Description": "the warehouse",
                "LocationUri": "s3://lake-bucket/warehouse/",
                "CreateTableDefaultPermissions": [],
            }),
            vec!["orders"],
            false,
        ),
    ];

    for (case, input, tables, hands_iam_default) in cases {
        // A fresh emulator per row, so a listing can only be about this row's catalog.
        let router = emulator();
        let database = input["Name"].as_str().expect("the row names a database");
        create_database_with_input(&router, &input).await;
        for table in &tables {
            create_table(&router, database, table).await;
        }

        for table in &tables {
            let creator = table_created_by(&router, database, table).await;
            let listed = list(
                &router,
                case,
                &json!({ "Resource": table_filter(database, table) }),
            )
            .await;
            let expected = expected_table_entries(&creator, database, table, hands_iam_default);
            assert_eq!(
                entries(&listed),
                expected,
                "case {case:?}: expected the creator of {database}.{table} to hold \
                 {expected:?}, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_table_nothing_created_holds_nothing_for_anyone() {
    // The complement of the rule: the grant belongs to a table that was CREATED, so a
    // listing about a table or a database the Data Catalog does not hold reports nothing.
    // The rows differ along which part of the resource names something absent.
    let router = emulator();
    create_database_with_input(&router, &governed_database("sales")).await;
    create_table(&router, "sales", "orders").await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "a table the database does not hold",
            table_filter("sales", "absent"),
        ),
        (
            "a table in a database the catalog does not hold",
            table_filter("absent", "orders"),
        ),
        (
            "a table in a Data Catalog this emulator holds nothing for",
            json!({
                "Table": { "CatalogId": "210987654321", "DatabaseName": "sales", "Name": "orders" }
            }),
        ),
    ];

    for (case, resource) in cases {
        let listed = list(&router, case, &json!({ "Resource": resource })).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: expected no entry at all, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_creators_grants_are_selected_by_exactly_the_filters_that_name_them() {
    // One row per filter member the operation carries, on either side of what selects a
    // table-level grant held by the creator. The catalog holds a `sales` database that
    // hands its tables no defaults, with its `orders` table; the database also carries the
    // account's DATABASE-level default grant — so a row that selects the database alone
    // reports one entry.
    let router = emulator();
    create_database_with_input(&router, &governed_database("sales")).await;
    create_table(&router, "sales", "orders").await;
    let creator = table_created_by(&router, "sales", "orders").await;
    let catalog = account_catalog();

    let cases: Vec<(&str, Value, usize)> = vec![
        ("no filter at all", json!({}), 3),
        (
            "the table itself",
            json!({ "Resource": table_filter("sales", "orders") }),
            2,
        ),
        (
            "the table named with its catalog id",
            json!({
                "Resource": {
                    "Table": { "CatalogId": catalog, "DatabaseName": "sales", "Name": "orders" }
                }
            }),
            2,
        ),
        (
            "every column of the table",
            json!({
                "Resource": {
                    "TableWithColumns": {
                        "DatabaseName": "sales",
                        "Name": "orders",
                        "ColumnWildcard": {},
                    }
                }
            }),
            1,
        ),
        (
            "every table in the database",
            json!({ "Resource": { "Table": { "DatabaseName": "sales", "TableWildcard": {} } } }),
            0,
        ),
        (
            "a table the catalog does not hold",
            json!({ "Resource": table_filter("sales", "absent") }),
            0,
        ),
        (
            "a table in a database the catalog does not hold",
            json!({ "Resource": table_filter("absent", "orders") }),
            0,
        ),
        (
            "the database, which carries its own default grant",
            json!({ "Resource": { "Database": { "Name": "sales" } } }),
            1,
        ),
        ("ResourceType TABLE", json!({ "ResourceType": "TABLE" }), 2),
        (
            "ResourceType DATABASE",
            json!({ "ResourceType": "DATABASE" }),
            1,
        ),
        (
            "ResourceType CATALOG",
            json!({ "ResourceType": "CATALOG" }),
            0,
        ),
        (
            "the principal that created the table",
            json!({ "Principal": { "DataLakePrincipalIdentifier": creator } }),
            2,
        ),
        (
            "that principal and the table, which is an effective listing",
            json!({
                "Principal": { "DataLakePrincipalIdentifier": creator },
                "Resource": table_filter("sales", "orders"),
            }),
            2,
        ),
        (
            "another principal",
            json!({
                "Principal": {
                    "DataLakePrincipalIdentifier": format!("arn:aws:iam::{catalog}:role/analyst")
                }
            }),
            0,
        ),
        (
            "another Data Catalog",
            json!({ "CatalogId": "111122223333" }),
            0,
        ),
    ];

    for (case, filter, expected) in cases {
        let listed = list(&router, case, &filter).await;
        assert_eq!(
            listed.len(),
            expected,
            "case {case:?}: expected {expected} entries for {filter:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn revoking_the_creators_permissions_leaves_exactly_the_rest() {
    // Rows differ along WHICH of the creator's permissions the revoke names: one held at
    // table scope, two, the one held at column scope, every table-scoped one, and every
    // permission the creator holds. Each row asserts what is left over both scopes.
    let named = |permissions: &[&str]| -> Vec<String> {
        permissions
            .iter()
            .map(|permission| (*permission).to_string())
            .collect()
    };
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("one table-scoped permission", named(&["DROP"])),
        ("two table-scoped permissions", named(&["DROP", "INSERT"])),
        ("the column-scoped permission", named(&["SELECT"])),
        ("every table-scoped permission", table_scoped()),
        (
            "every permission the creator holds",
            named(&CREATOR_PERMISSIONS),
        ),
    ];

    for (case, revoked) in cases {
        let router = emulator();
        create_database_with_input(&router, &governed_database("sales")).await;
        create_table(&router, "sales", "orders").await;
        let creator = table_created_by(&router, "sales", "orders").await;

        let body = json!({
            "Principal": { "DataLakePrincipalIdentifier": creator },
            "Resource": table_filter("sales", "orders"),
            "Permissions": revoked,
            "PermissionsWithGrantOption": revoked,
        });
        let (status, payload) = call(&router, "RevokePermissions", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the revoke to be accepted, got {status} {payload:?}"
        );

        let left = |held: Vec<String>| -> Vec<String> {
            held.into_iter()
                .filter(|permission| !revoked.contains(permission))
                .collect()
        };
        let expected = expected_entries(
            &creator,
            "sales",
            "orders",
            &left(table_scoped()),
            &left(column_scoped()),
        );
        let listed = list(
            &router,
            case,
            &json!({ "Resource": table_filter("sales", "orders") }),
        )
        .await;
        assert_eq!(
            entries(&listed),
            expected,
            "case {case:?}: expected {expected:?} left after revoking {revoked:?}, got \
             {listed:?}"
        );

        // Nothing is revocable twice: what the revoke took away is gone, so naming it
        // again is a permission the grantee does not hold.
        let (status, payload) = call(&router, "RevokePermissions", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the second revoke to be refused, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {payload:?}"
        );
    }
}

#[tokio::test]
async fn the_creators_grant_is_per_table() {
    // The grant belongs to one table: revoking it on one says nothing about the tables
    // beside it, in the same database or in another one.
    let router = emulator();
    for database in ["sales", "finance"] {
        create_database_with_input(&router, &governed_database(database)).await;
        for table in ["orders", "returns"] {
            create_table(&router, database, table).await;
        }
    }
    let creator = table_created_by(&router, "sales", "orders").await;

    let (status, payload) = call(
        &router,
        "RevokePermissions",
        &json!({
            "Principal": { "DataLakePrincipalIdentifier": creator },
            "Resource": table_filter("sales", "orders"),
            "Permissions": CREATOR_PERMISSIONS,
            "PermissionsWithGrantOption": CREATOR_PERMISSIONS,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"revoke one table's creator grant\": expected 200, got {status} {payload:?}"
    );

    let cases: Vec<(&str, &str, &str, bool)> = vec![
        ("the revoked table", "sales", "orders", false),
        ("another table in the same database", "sales", "returns", true),
        ("a table in another database", "finance", "orders", true),
        ("another table in another database", "finance", "returns", true),
    ];
    for (case, database, table, still_held) in cases {
        let listed = list(
            &router,
            case,
            &json!({ "Resource": table_filter(database, table) }),
        )
        .await;
        let expected = if still_held {
            creator_entries(&creator, database, table)
        } else {
            Vec::new()
        };
        assert_eq!(
            entries(&listed),
            expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }
}

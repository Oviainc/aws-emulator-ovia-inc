// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the default permissions a Data Catalog table is CREATED carrying
//! from its parent database.
//!
//! One rule, over a table of inputs rather than one request: a database's
//! `Database$CreateTableDefaultPermissions` — "creates a set of default permissions on the
//! table for principals. Used by Lake Formation" — is held on every table created in it by
//! the principals that setting names, at the scope each permission applies at and without
//! the grant option, so a listing of such a table reports that grant beside the creator's
//! own (which is the `creator_table_grants.rs` suite beside this one). A database that
//! hands its tables nothing leaves the table to its creator alone.
//!
//! The rows span what the rule generalises over: both values the member admits ("The only
//! permitted values are an empty array or an array that contains a single JSON object that
//! grants ALL to IAM_ALLOWED_PRINCIPALS"), a create that names neither, which database and
//! table the listing is about, and whether the grant has been revoked.

use serde_json::{json, Value};

use crate::creator_table_grants::{
    column_scoped, creator_entries, governed_database, iam_access_control_database,
    iam_allowed_entry, table_filter,
};
use crate::glue_catalog::{
    create_database_with_input, create_table, emulator, error_code, lake_formation as call,
    list_permissions as list, permission_entries as entries, table_created_by, Entry,
};

/// The group the IAM-access-control default names, spelled as Lake Formation spells it on
/// the wire.
const IAM_ALLOWED_PRINCIPALS: &str = "IAM_ALLOWED_PRINCIPALS";

#[tokio::test]
async fn a_created_table_carries_what_its_database_hands_the_tables_created_in_it() {
    // Rows differ along what the create named for `CreateTableDefaultPermissions`: the
    // IAM-access-control default, nothing at all (which records the catalog's own default,
    // the same value), and the empty list, which hands the tables nothing.
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "the IAM-access-control default",
            iam_access_control_database("sales"),
            true,
        ),
        (
            "a create that named no table-level defaults",
            json!({ "Name": "sales" }),
            true,
        ),
        (
            "no table-level defaults at all",
            governed_database("sales"),
            false,
        ),
    ];

    for (case, input, hands_iam_default) in cases {
        // A fresh emulator per row, so a listing can only be about this row's catalog.
        let router = emulator();
        create_database_with_input(&router, &input).await;
        create_table(&router, "sales", "orders").await;
        let creator = table_created_by(&router, "sales", "orders").await;

        let listed = list(
            &router,
            case,
            &json!({ "Resource": table_filter("sales", "orders") }),
        )
        .await;
        let held = entries(&listed);
        let group_entries: Vec<Entry> = held
            .iter()
            .filter(|(principal, ..)| principal == IAM_ALLOWED_PRINCIPALS)
            .cloned()
            .collect();
        let expected: Vec<Entry> = if hands_iam_default {
            vec![iam_allowed_entry("sales", "orders")]
        } else {
            Vec::new()
        };
        assert_eq!(
            group_entries, expected,
            "case {case:?}: expected {expected:?} for the group, got {listed:?}"
        );

        // Whatever the database hands its tables, the creator's own grant is unchanged.
        let creator_held: Vec<Entry> = held
            .iter()
            .filter(|(principal, ..)| principal == &creator)
            .cloned()
            .collect();
        let expected = creator_entries(&creator, "sales", "orders");
        assert_eq!(
            creator_held, expected,
            "case {case:?}: expected the creator to hold {expected:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_default_a_database_hands_its_tables_is_revocable_exactly_once_per_table() {
    // The grant belongs to each table the database holds, so revoking it on one leaves the
    // others carrying it; a second revoke on the same table has nothing left to take. The
    // rows differ along which table the listing is about after the revoke.
    let router = emulator();
    create_database_with_input(&router, &iam_access_control_database("sales")).await;
    for table in ["orders", "returns"] {
        create_table(&router, "sales", table).await;
    }
    let creator = table_created_by(&router, "sales", "orders").await;

    let revoke = json!({
        "Principal": { "DataLakePrincipalIdentifier": IAM_ALLOWED_PRINCIPALS },
        "Resource": table_filter("sales", "orders"),
        "Permissions": ["ALL"],
    });
    let (status, payload) = call(&router, "RevokePermissions", &revoke).await;
    assert_eq!(
        status, 200,
        "case \"revoke the default on one table\": expected 200, got {status} {payload:?}"
    );

    let cases: Vec<(&str, &str, bool)> = vec![
        ("the revoked table", "orders", false),
        ("another table in the same database", "returns", true),
    ];
    for (case, table, still_held) in cases {
        let listed = list(
            &router,
            case,
            &json!({ "Resource": table_filter("sales", table) }),
        )
        .await;
        let mut expected = creator_entries(&creator, "sales", table);
        if still_held {
            expected.push(iam_allowed_entry("sales", table));
        }
        expected.sort();
        assert_eq!(
            entries(&listed),
            expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // Nothing is revocable twice: the revoke is remembered, so naming it again is a
    // permission the grantee does not hold.
    let (status, payload) = call(&router, "RevokePermissions", &revoke).await;
    assert_eq!(
        status, 400,
        "case \"revoke the default twice\": expected 400, got {status} {payload:?}"
    );
    assert_eq!(
        error_code(&payload),
        "InvalidInputException",
        "case \"revoke the default twice\": expected InvalidInputException, got {payload:?}"
    );
}

#[tokio::test]
async fn the_default_is_held_at_the_scope_each_permission_it_names_applies_at() {
    // A database that hands its tables a column-scoped permission hands it at COLUMN scope,
    // exactly as a grant of it would be recorded ("call this operation on the table, and
    // the operation returns the table and the table w columns" —
    // `ListPermissions$Resource`). The rows differ along which scopes the named permissions
    // belong to: the table alone, the columns alone, and both. Every permission named is
    // one the Data Catalog's own `CreateTableDefaultPermissions` enum admits.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("a table-scoped permission", vec!["ALTER"]),
        ("the column-scoped permission", vec!["SELECT"]),
        ("one of each", vec!["ALTER", "SELECT"]),
        ("Super, which is the table's own", vec!["ALL"]),
    ];

    for (case, permissions) in cases {
        let router = emulator();
        let analyst = format!(
            "arn:aws:iam::{}:role/analyst",
            crate::glue_catalog::account_catalog()
        );
        create_database_with_input(
            &router,
            &json!({
                "Name": "sales",
                "CreateTableDefaultPermissions": [{
                    "Principal": { "DataLakePrincipalIdentifier": analyst },
                    "Permissions": permissions,
                }],
            }),
        )
        .await;
        create_table(&router, "sales", "orders").await;

        let on_table: Vec<String> = permissions
            .iter()
            .filter(|permission| !column_scoped().contains(&(*permission).to_string()))
            .map(|permission| (*permission).to_string())
            .collect();
        let on_columns: Vec<String> = permissions
            .iter()
            .filter(|permission| column_scoped().contains(&(*permission).to_string()))
            .map(|permission| (*permission).to_string())
            .collect();
        // A default carries no grant option, so the second list of every entry is empty.
        let mut expected: Vec<Entry> = Vec::new();
        if !on_table.is_empty() {
            let mut sorted = on_table.clone();
            sorted.sort();
            expected.push((
                analyst.clone(),
                crate::creator_table_grants::reported_table("sales", "orders").to_string(),
                sorted,
                Vec::new(),
            ));
        }
        if !on_columns.is_empty() {
            let mut sorted = on_columns.clone();
            sorted.sort();
            expected.push((
                analyst.clone(),
                crate::creator_table_grants::reported_columns("sales", "orders").to_string(),
                sorted,
                Vec::new(),
            ));
        }
        expected.sort();

        let listed = list(
            &router,
            case,
            &json!({
                "Resource": table_filter("sales", "orders"),
                "Principal": { "DataLakePrincipalIdentifier": analyst },
            }),
        )
        .await;
        assert_eq!(
            entries(&listed),
            expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }
}

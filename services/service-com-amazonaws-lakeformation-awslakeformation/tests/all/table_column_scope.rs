// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the scope a GRANT OPTION is held at.
//!
//! `permission_scope.rs` beside this file covers which record each granted permission lands
//! on. This suite covers the member that travels with it: `PermissionsWithGrantOption`
//! ("Indicates a list of the granted permissions that the principal may pass to other
//! users. These permissions may only be a subset of the permissions granted in the
//! Privileges." — `GrantPermissions$PermissionsWithGrantOption`), which is held at the same
//! scope as the permission it accompanies — so a table grant whose grant option names the
//! column-scoped permission records that grant option on the table's "table w columns"
//! record and not on the table's own.
//!
//! The rows span what the rule generalises over: which list carries the column-scoped
//! permission (both, the grant option only, neither), whether the grant splits at all, and
//! the resource form (a table by name, which has a column scope, and a database, which has
//! none). Every row keeps `PermissionsWithGrantOption` a subset of `Permissions`, which is
//! what the member documents.
//!
//! The Data Catalog is seeded through the shared harness, because a grant naming an entity
//! the catalog does not hold is refused; every call goes over the wire the way an SDK
//! client does, so a response can only be what the handler computed from stored state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::glue_catalog::{account_catalog, emulator_with_catalog, lake_formation};

/// The grantee every row grants to: one role, so a principal-filtered listing reads back
/// exactly the records this row's grant wrote.
const ANALYST: &str = "arn:aws:iam::123456789012:role/analyst";

/// One record a listing must report: its `Resource`, its `Permissions` and its
/// `PermissionsWithGrantOption`.
type ExpectedRecord = (Value, Vec<&'static str>, Vec<&'static str>);

/// One row: the resource granted on, the two permission lists, and the records that must
/// come back.
struct GrantOptionCase {
    case: &'static str,
    resource: Value,
    permissions: Vec<&'static str>,
    with_grant_option: Vec<&'static str>,
    expected: Vec<ExpectedRecord>,
}

/// An emulator whose catalog holds the `sales` database and its `orders` table.
async fn emulator() -> MultiServiceRouter {
    emulator_with_catalog(&[("sales", &["orders"])]).await
}

/// The string list `member` of one entry, empty when the member carries none.
fn strings(entry: &Value, member: &str) -> Vec<String> {
    entry
        .get(member)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// What `ANALYST` holds, as (resource, permissions, grant-option permissions) triples
/// sorted so the comparison does not depend on the order records are enumerated in.
async fn held(
    router: &MultiServiceRouter,
    case: &str,
) -> Vec<(String, Vec<String>, Vec<String>)> {
    let (status, payload) = lake_formation(
        router,
        "ListPermissions",
        &json!({ "Principal": { "DataLakePrincipalIdentifier": ANALYST } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListPermissions to answer 200, got {status} {payload:?}"
    );
    let entries = payload
        .get("PrincipalResourcePermissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: PrincipalResourcePermissions must be a list, got {payload:?}")
        });
    let mut held: Vec<(String, Vec<String>, Vec<String>)> = entries
        .iter()
        .map(|entry| {
            (
                entry.get("Resource").cloned().unwrap_or(Value::Null).to_string(),
                strings(entry, "Permissions"),
                strings(entry, "PermissionsWithGrantOption"),
            )
        })
        .collect();
    held.sort();
    held
}

/// The expected records of one row, in the same sorted, stringified form [`held`] answers.
fn wanted(expected: &[ExpectedRecord]) -> Vec<(String, Vec<String>, Vec<String>)> {
    let mut wanted: Vec<(String, Vec<String>, Vec<String>)> = expected
        .iter()
        .map(|(resource, permissions, with_grant_option)| {
            (
                resource.to_string(),
                permissions.iter().map(|value| (*value).to_string()).collect(),
                with_grant_option
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect(),
            )
        })
        .collect();
    wanted.sort();
    wanted
}

/// The table record of the seeded table, as a listing echoes it (with the catalog id the
/// model documents a default for filled in).
fn table_record() -> Value {
    json!({ "Table": { "CatalogId": account_catalog(), "DatabaseName": "sales", "Name": "orders" } })
}

/// Its column-level record: every column, named by the wildcard.
fn columns_record() -> Value {
    json!({
        "TableWithColumns": {
            "CatalogId": account_catalog(),
            "DatabaseName": "sales",
            "Name": "orders",
            "ColumnWildcard": {},
        }
    })
}

fn grant_option_cases() -> Vec<GrantOptionCase> {
    let table = json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } });
    vec![
        GrantOptionCase {
            case: "the grant option names the column-scoped permission",
            resource: table.clone(),
            permissions: vec!["SELECT", "DESCRIBE"],
            with_grant_option: vec!["SELECT"],
            expected: vec![
                (table_record(), vec!["DESCRIBE"], vec![]),
                (columns_record(), vec!["SELECT"], vec!["SELECT"]),
            ],
        },
        GrantOptionCase {
            case: "the grant option names the table-scoped permission",
            resource: table.clone(),
            permissions: vec!["SELECT", "DESCRIBE"],
            with_grant_option: vec!["DESCRIBE"],
            expected: vec![
                (table_record(), vec!["DESCRIBE"], vec!["DESCRIBE"]),
                (columns_record(), vec!["SELECT"], vec![]),
            ],
        },
        GrantOptionCase {
            case: "the grant option names both",
            resource: table.clone(),
            permissions: vec!["SELECT", "DESCRIBE"],
            with_grant_option: vec!["SELECT", "DESCRIBE"],
            expected: vec![
                (table_record(), vec!["DESCRIBE"], vec!["DESCRIBE"]),
                (columns_record(), vec!["SELECT"], vec!["SELECT"]),
            ],
        },
        GrantOptionCase {
            case: "a grant that does not split at all",
            resource: table.clone(),
            permissions: vec!["DESCRIBE", "ALTER"],
            with_grant_option: vec!["ALTER"],
            expected: vec![(table_record(), vec!["DESCRIBE", "ALTER"], vec!["ALTER"])],
        },
        GrantOptionCase {
            case: "a grant that is wholly column-scoped",
            resource: table,
            permissions: vec!["SELECT"],
            with_grant_option: vec!["SELECT"],
            expected: vec![(columns_record(), vec!["SELECT"], vec!["SELECT"])],
        },
        GrantOptionCase {
            // A database has no column scope, so both lists stay on its one record.
            case: "a database, which has no column scope",
            resource: json!({ "Database": { "Name": "sales" } }),
            permissions: vec!["SELECT", "DESCRIBE"],
            with_grant_option: vec!["SELECT"],
            expected: vec![(
                json!({ "Database": { "CatalogId": account_catalog(), "Name": "sales" } }),
                vec!["SELECT", "DESCRIBE"],
                vec!["SELECT"],
            )],
        },
    ]
}

#[tokio::test]
async fn a_grant_option_is_held_at_the_scope_of_the_permission_it_accompanies() {
    for case in grant_option_cases() {
        // A fresh emulator per row, so the listing reports only this row's grant.
        let router = emulator().await;
        let (status, payload) = lake_formation(
            &router,
            "GrantPermissions",
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": ANALYST },
                "Resource": case.resource,
                "Permissions": case.permissions,
                "PermissionsWithGrantOption": case.with_grant_option,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GrantPermissions to answer 200, got {status} {payload:?}",
            case.case
        );

        let held = held(&router, case.case).await;
        let wanted = wanted(&case.expected);
        assert_eq!(
            held, wanted,
            "case {:?}: expected the principal to hold {wanted:?}, got {held:?}",
            case.case
        );
    }
}

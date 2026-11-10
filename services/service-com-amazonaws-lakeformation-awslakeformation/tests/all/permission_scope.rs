// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the two rules `GrantPermissions` applies to a Data Catalog
//! resource: the resource must EXIST, and each permission is recorded at the SCOPE it
//! applies at.
//!
//! Both need the Glue Data Catalog, so this suite mounts Lake Formation AND Glue on ONE
//! shared engine — the pair the production router builds — and creates the databases and
//! tables through Glue's own wire API (`awsJson1_1` `POST /` with `X-Amz-Target`) before
//! granting on them through Lake Formation's (`restJson1` `POST /<Operation>`). A
//! response can therefore only be what the handler computed from stored state.
//!
//! The tables below span the classes each rule generalises over (which entity is missing,
//! which resource kind names it, the permission set); `permissions.rs` beside this file
//! covers the rest of the grant store.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one copy of the harness every permission suite shares: the Glue+LakeFormation
// emulator, the Glue catalog writers, the `restJson1` wire path and the error-code
// extractor.
use crate::glue_catalog::{
    account_catalog as account, create_database, create_table, emulator, error_code,
    lake_formation, revoke_default_database_grant, revoke_table_default_grants,
};

/// One row of the scope table: the case name, the resource granted on, the permissions
/// asked for, and the `(Resource, Permissions)` records that must come back.
type ScopeCase = (
    &'static str,
    Value,
    Vec<&'static str>,
    Vec<(Value, Vec<&'static str>)>,
);

/// `GrantPermissions` for `principal` on `resource`, answering `(status, body)`.
async fn grant(
    router: &MultiServiceRouter,
    principal: &str,
    resource: &Value,
    permissions: &[&str],
) -> (u16, Value) {
    lake_formation(
        router,
        "GrantPermissions",
        &json!({
            "Principal": {"DataLakePrincipalIdentifier": principal},
            "Resource": resource,
            "Permissions": permissions,
        }),
    )
    .await
}

/// `ListPermissions` with whatever filters `filters` names, asserting 200.
async fn list(router: &MultiServiceRouter, case: &str, filters: &Value) -> Value {
    let (status, body) = lake_formation(router, "ListPermissions", filters).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListPermissions to answer 200, got {status} {body}"
    );
    body
}

/// The `(Resource, Permissions)` pairs a `ListPermissions` body carries, sorted so a
/// comparison does not depend on the order the service enumerated records in.
fn records(body: &Value) -> Vec<(Value, Vec<String>)> {
    let mut records: Vec<(Value, Vec<String>)> = body
        .get("PrincipalResourcePermissions")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    let permissions = entry
                        .get("Permissions")
                        .and_then(Value::as_array)
                        .map(|permissions| {
                            permissions
                                .iter()
                                .map(|permission| {
                                    permission.as_str().unwrap_or_default().to_string()
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (
                        entry.get("Resource").cloned().unwrap_or(Value::Null),
                        permissions,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    records.sort_by_key(|(resource, _)| resource.to_string());
    records
}

/// The expected records, sorted the way [`records`] sorts what came back.
fn expected(pairs: Vec<(Value, Vec<&'static str>)>) -> Vec<(Value, Vec<String>)> {
    let mut expected: Vec<(Value, Vec<String>)> = pairs
        .into_iter()
        .map(|(resource, permissions)| {
            (
                resource,
                permissions.into_iter().map(str::to_string).collect(),
            )
        })
        .collect();
    expected.sort_by_key(|(resource, _)| resource.to_string());
    expected
}

// ---------------------------------------------------------------------------
// A grant names a Data Catalog entity the catalog holds
// ---------------------------------------------------------------------------

/// A grant naming a database or table the Data Catalog does not hold is refused with HTTP
/// 400 and the modelled `InvalidInputException` and persists nothing; a grant naming one
/// it does hold is recorded.
///
/// The rows differ along which entity is missing and which resource member names it: the
/// database itself, a table in a database that exists, a table under a database that does
/// not, every table of a database that does not, and the same table through
/// `TableWithColumns`. Each row also fixes WHICH kind of entity the refusal names, which
/// is the database whenever the database is what could not be resolved.
#[tokio::test]
async fn a_grant_on_an_entity_the_catalog_does_not_hold_is_refused() {
    let principal = "arn:aws:iam::000000000000:user/analyst";

    // An emulator whose catalog has never held anything: a catalog that holds no database
    // refuses a grant on one, the way Glue's own `GetDatabase` does.
    let empty = emulator();
    let (status, body) = grant(
        &empty,
        principal,
        &json!({"Database": {"Name": "sales"}}),
        &["DESCRIBE"],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"no database has ever been created\": expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case \"no database has ever been created\": expected InvalidInputException, got {body}"
    );
    let body = list(&empty, "the empty catalog", &json!({})).await;
    assert!(
        records(&body).is_empty(),
        "case \"no database has ever been created\": nothing may be persisted, got {body}"
    );

    let router = emulator();
    create_database(&router, "sales").await;
    create_table(&router, "sales", "orders").await;
    // The assertions below count the records the grants wrote, so the database's
    // creation-time default grant and the grants creating the table made (see
    // `creator_table_grants.rs`) are revoked first.
    revoke_default_database_grant(&router, "sales").await;
    revoke_table_default_grants(&router, "sales", "orders").await;

    // The third field is the entity kind the refusal must name: whichever of the two the
    // catalog could not resolve, the database first.
    let refused: Vec<(&str, Value, &str)> = vec![
        (
            "a missing database",
            json!({"Database": {"Name": "absent"}}),
            "Database",
        ),
        (
            "a missing table in a database that exists",
            json!({"Table": {"DatabaseName": "sales", "Name": "absent"}}),
            "Table",
        ),
        (
            "a table under a missing database",
            json!({"Table": {"DatabaseName": "absent", "Name": "orders"}}),
            "Database",
        ),
        (
            "every table of a missing database",
            json!({"Table": {"DatabaseName": "absent", "TableWildcard": {}}}),
            "Database",
        ),
        (
            "a missing table through TableWithColumns",
            json!({
                "TableWithColumns": {
                    "DatabaseName": "sales",
                    "Name": "absent",
                    "ColumnWildcard": {}
                }
            }),
            "Table",
        ),
        (
            "a missing table with the catalog id named explicitly",
            json!({"Table": {"CatalogId": account(), "DatabaseName": "sales", "Name": "absent"}}),
            "Table",
        ),
    ];
    for (case, resource, kind) in refused {
        let (status, body) = grant(&router, principal, &resource, &["SELECT"]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        let message = body
            .get("Message")
            .or_else(|| body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            message.starts_with(kind),
            "case {case:?}: expected the refusal to name the missing {kind}, got {message:?}"
        );
    }

    // No refusal above wrote anything.
    let body = list(&router, "after the refusals", &json!({})).await;
    assert!(
        records(&body).is_empty(),
        "a refused grant must persist nothing, got {body}"
    );

    // A permission per row, because a resource that addresses COLUMNS takes only a
    // column-scoped one (`TableWithColumnsResource` "is only used when granting a SELECT
    // permission").
    let accepted: Vec<(&str, Value, &str)> = vec![
        (
            "the database",
            json!({"Database": {"Name": "sales"}}),
            "DESCRIBE",
        ),
        (
            "the table",
            json!({"Table": {"DatabaseName": "sales", "Name": "orders"}}),
            "DESCRIBE",
        ),
        (
            "every table of the database",
            json!({"Table": {"DatabaseName": "sales", "TableWildcard": {}}}),
            "DESCRIBE",
        ),
        (
            "the table, through TableWithColumns",
            json!({
                "TableWithColumns": {
                    "DatabaseName": "sales",
                    "Name": "orders",
                    "ColumnWildcard": {}
                }
            }),
            "SELECT",
        ),
        ("the catalog itself", json!({"Catalog": {}}), "DESCRIBE"),
        // A catalog this emulator holds nothing for answers nothing about its contents,
        // so a grant on it is not refused — the refusal has to be grounded.
        (
            "a database in a catalog this emulator holds no Data Catalog for",
            json!({"Database": {"CatalogId": "210987654321", "Name": "absent"}}),
            "DESCRIBE",
        ),
    ];
    let accepted_count = accepted.len();
    for (case, resource, permission) in accepted {
        let (status, body) = grant(&router, principal, &resource, &[permission]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
    }
    let body = list(&router, "after the accepted grants", &json!({})).await;
    assert_eq!(
        records(&body).len(),
        accepted_count,
        "each accepted grant must leave one record, got {body}"
    );
}

/// A table whose name differs only in case is the SAME table, because the Data Catalog
/// folds a name to lowercase when it stores it ("For Hive compatibility, this is folded to
/// lowercase when it is stored" — `TableInput$Name`), so each spelling is accepted AND
/// lands on the one record, keyed by the stored name.
#[tokio::test]
async fn a_name_is_matched_the_way_the_catalog_stores_it() {
    let router = emulator();
    create_database(&router, "Mixed_Case_DB").await;
    create_table(&router, "Mixed_Case_DB", "Mixed_Case_Table").await;
    let principal = "arn:aws:iam::000000000000:user/analyst";

    // Each row names the same two entities in a different casing.
    for (case, database, table) in [
        ("as created", "Mixed_Case_DB", "Mixed_Case_Table"),
        ("folded", "mixed_case_db", "mixed_case_table"),
        ("upper", "MIXED_CASE_DB", "MIXED_CASE_TABLE"),
    ] {
        let (status, body) = grant(
            &router,
            principal,
            &json!({"Table": {"DatabaseName": database, "Name": table}}),
            &["DESCRIBE"],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
    }

    // Three spellings of one table are one record, keyed by the name the catalog stores.
    let body = list(
        &router,
        "the folded table",
        &json!({"Principal": {"DataLakePrincipalIdentifier": principal}}),
    )
    .await;
    assert_eq!(
        records(&body),
        expected(vec![(
            json!({
                "Table": {
                    "CatalogId": account(),
                    "DatabaseName": "mixed_case_db",
                    "Name": "mixed_case_table"
                }
            }),
            vec!["DESCRIBE"],
        )]),
        "three spellings of one table must leave one record on the stored name, got {body}"
    );
}

// ---------------------------------------------------------------------------
// A permission is recorded at the scope it applies at
// ---------------------------------------------------------------------------

/// A table grant is recorded at the scope each permission applies at: the column-scoped
/// ones (`SELECT`) as a `TableWithColumns` record over every column, the rest as the
/// table's own record — the "table and the table w columns" pair
/// `ListPermissions$Resource` documents.
///
/// The rows differ along the permission set and the resource kind: a set that splits, one
/// that is wholly column-scoped, one that is wholly table-scoped, `ALL`, a database (no
/// narrower scope), a table wildcard (no single table to scope columns on), and a grant
/// made on `TableWithColumns` directly.
#[tokio::test]
async fn a_table_grant_is_recorded_at_the_scope_each_permission_applies_at() {
    let router = emulator();
    create_database(&router, "sales").await;
    create_table(&router, "sales", "orders").await;
    let catalog = account();

    let table =
        json!({"Table": {"CatalogId": catalog, "DatabaseName": "sales", "Name": "orders"}});
    let all_columns = json!({
        "TableWithColumns": {
            "CatalogId": catalog,
            "DatabaseName": "sales",
            "Name": "orders",
            "ColumnWildcard": {}
        }
    });
    let on_table = json!({"Table": {"DatabaseName": "sales", "Name": "orders"}});

    let cases: Vec<ScopeCase> = vec![
        (
            "SELECT and DESCRIBE split into a table and a column-wildcard record",
            on_table.clone(),
            vec!["SELECT", "DESCRIBE"],
            vec![
                (table.clone(), vec!["DESCRIBE"]),
                (all_columns.clone(), vec!["SELECT"]),
            ],
        ),
        (
            "SELECT alone is recorded only at column scope",
            on_table.clone(),
            vec!["SELECT"],
            vec![(all_columns.clone(), vec!["SELECT"])],
        ),
        (
            "DESCRIBE alone stays on the table",
            on_table.clone(),
            vec!["DESCRIBE"],
            vec![(table.clone(), vec!["DESCRIBE"])],
        ),
        (
            "ALL is a table permission, not a column one",
            on_table.clone(),
            vec!["ALL"],
            vec![(table.clone(), vec!["ALL"])],
        ),
        (
            "the table-scoped permissions keep the order they were asked in",
            on_table.clone(),
            vec!["ALTER", "SELECT", "DROP"],
            vec![
                (table.clone(), vec!["ALTER", "DROP"]),
                (all_columns.clone(), vec!["SELECT"]),
            ],
        ),
        (
            "a database grant does not split",
            json!({"Database": {"Name": "sales"}}),
            vec!["CREATE_TABLE", "DESCRIBE"],
            vec![(
                json!({"Database": {"CatalogId": catalog, "Name": "sales"}}),
                vec!["CREATE_TABLE", "DESCRIBE"],
            )],
        ),
        (
            "a table wildcard names no single table to scope columns on",
            json!({"Table": {"DatabaseName": "sales", "TableWildcard": {}}}),
            vec!["SELECT", "DESCRIBE"],
            vec![(
                json!({
                    "Table": {
                        "CatalogId": catalog,
                        "DatabaseName": "sales",
                        "TableWildcard": {}
                    }
                }),
                vec!["SELECT", "DESCRIBE"],
            )],
        ),
        (
            "a grant made on the columns keeps the columns it named",
            json!({
                "TableWithColumns": {
                    "DatabaseName": "sales",
                    "Name": "orders",
                    "ColumnNames": ["total", "customer"]
                }
            }),
            vec!["SELECT"],
            vec![(
                json!({
                    "TableWithColumns": {
                        "CatalogId": catalog,
                        "DatabaseName": "sales",
                        "Name": "orders",
                        "ColumnNames": ["total", "customer"]
                    }
                }),
                vec!["SELECT"],
            )],
        ),
    ];

    for (index, (case, resource, permissions, want)) in cases.into_iter().enumerate() {
        // One principal per row, so each row reads back only its own records.
        let principal = format!("arn:aws:iam::000000000000:user/scope-{index}");
        let (status, body) = grant(&router, &principal, &resource, &permissions).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );

        let body = list(
            &router,
            case,
            &json!({"Principal": {"DataLakePrincipalIdentifier": principal}}),
        )
        .await;
        assert_eq!(
            records(&body),
            expected(want),
            "case {case:?}: unexpected records, got {body}"
        );
    }
}

/// A grant made DIRECTLY on `TableWithColumns` may only ask for a column-scoped
/// permission: that resource "is only used when granting a SELECT permission"
/// (`smithy_query(query="doc",shape="TableWithColumnsResource")`), so a request that
/// addresses columns and names any other permission is refused with the modelled
/// `InvalidInputException` and persists nothing.
///
/// The rows differ along WHERE the offending permission is named and which one it is: in
/// `Permissions` alone, beside a `SELECT`, in `PermissionsWithGrantOption`, and the
/// table-wide `ALL`. The accepted rows at the end show the refusal is about the SCOPE, not
/// the resource: `SELECT` on the same columns is recorded, and the same non-column
/// permissions on the TABLE are recorded at table scope.
#[tokio::test]
async fn a_column_scoped_grant_may_only_name_a_column_scoped_permission() {
    let router = emulator();
    create_database(&router, "sales").await;
    create_table(&router, "sales", "orders").await;
    // One assertion below is that a refused grant persisted nothing, so the database's
    // creation-time default grant and the grants creating the table made (see
    // `creator_table_grants.rs`) are revoked first.
    revoke_default_database_grant(&router, "sales").await;
    revoke_table_default_grants(&router, "sales", "orders").await;

    let columns = json!({
        "TableWithColumns": {
            "DatabaseName": "sales",
            "Name": "orders",
            "ColumnNames": ["total"]
        }
    });

    // (case, Permissions, PermissionsWithGrantOption)
    let refused: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("DESCRIBE on columns", vec!["DESCRIBE"], vec![]),
        ("SELECT beside DROP on columns", vec!["SELECT", "DROP"], vec![]),
        ("ALTER on columns", vec!["ALTER"], vec![]),
        ("the table-wide ALL on columns", vec!["ALL"], vec![]),
        (
            "INSERT named only with the grant option",
            vec!["SELECT"],
            vec!["INSERT"],
        ),
    ];

    for (case, permissions, with_grant_option) in refused {
        // Both write paths apply the rule, so a column-scoped request naming a
        // non-column permission is refused on the way in and on the way out.
        for operation in ["GrantPermissions", "RevokePermissions"] {
            let (status, body) = lake_formation(
                &router,
                operation,
                &json!({
                    "Principal": {"DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS"},
                    "Resource": columns,
                    "Permissions": permissions,
                    "PermissionsWithGrantOption": with_grant_option,
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {case:?} on {operation}: expected InvalidInputException, got {body}"
            );
        }
    }

    // No refusal above wrote anything.
    let body = list(&router, "after the column-scope refusals", &json!({})).await;
    assert!(
        records(&body).is_empty(),
        "case \"after the column-scope refusals\": a refused grant persists nothing, got {body}"
    );

    // The same permissions are fine where they DO apply: SELECT on those columns, and the
    // non-column ones on the table itself.
    let accepted: Vec<(&str, Value, Vec<&str>)> = vec![
        ("SELECT on the columns", columns.clone(), vec!["SELECT"]),
        (
            "DESCRIBE and ALTER on the table",
            json!({"Table": {"DatabaseName": "sales", "Name": "orders"}}),
            vec!["DESCRIBE", "ALTER"],
        ),
    ];
    for (index, (case, resource, permissions)) in accepted.into_iter().enumerate() {
        let principal = format!("arn:aws:iam::000000000000:user/column-scope-{index}");
        let (status, body) = grant(&router, &principal, &resource, &permissions).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        let body = list(
            &router,
            case,
            &json!({"Principal": {"DataLakePrincipalIdentifier": principal}}),
        )
        .await;
        let held: BTreeSet<String> = records(&body)
            .into_iter()
            .flat_map(|(_, permissions)| permissions)
            .collect();
        let want: BTreeSet<String> = permissions.iter().map(|p| p.to_string()).collect();
        assert_eq!(
            held, want,
            "case {case:?}: expected the stored permissions to be {want:?}, got {held:?}"
        );
    }
}

/// Whatever the permission set, the records a table grant writes carry exactly the
/// permissions asked for, once each, across at most the table and its columns.
/// The invariant over a SET of inputs rather than one request: EVERY non-empty subset of
/// the table-applicable permissions is granted and read back. The names are the
/// `Permission` enum's own (`smithy_query(query="shape",shape="Permission")` — ALL,
/// SELECT, ALTER, DROP, DELETE, INSERT, DESCRIBE, ...), narrowed to the ones a table
/// takes.
#[tokio::test]
async fn every_requested_permission_is_recorded_exactly_once() {
    let router = emulator();
    create_database(&router, "sales").await;
    create_table(&router, "sales", "orders").await;

    const TABLE_PERMISSIONS: [&str; 6] = ["SELECT", "ALTER", "DROP", "DELETE", "INSERT", "DESCRIBE"];
    for mask in 1u32..(1 << TABLE_PERMISSIONS.len()) {
        let permissions: Vec<&str> = TABLE_PERMISSIONS
            .iter()
            .enumerate()
            .filter(|(bit, _)| mask & (1 << bit) != 0)
            .map(|(_, permission)| *permission)
            .collect();
        let principal = format!("arn:aws:iam::000000000000:user/subset-{mask}");
        let (status, body) = grant(
            &router,
            &principal,
            &json!({"Table": {"DatabaseName": "sales", "Name": "orders"}}),
            &permissions,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {permissions:?}: expected HTTP 200, got {status} {body}"
        );

        let body = list(
            &router,
            "a subset of the table permissions",
            &json!({"Principal": {"DataLakePrincipalIdentifier": principal}}),
        )
        .await;
        let written = records(&body);
        let mut held: Vec<String> = written
            .iter()
            .flat_map(|(_, permissions)| permissions.clone())
            .collect();
        let distinct: BTreeSet<String> = held.iter().cloned().collect();
        assert_eq!(
            held.len(),
            distinct.len(),
            "case {permissions:?}: no permission may be recorded twice, got {body}"
        );
        held.sort();
        let mut want: Vec<String> = permissions.iter().map(|p| p.to_string()).collect();
        want.sort();
        assert_eq!(
            held, want,
            "case {permissions:?}: the records must carry exactly what was granted, got {body}"
        );
        assert!(
            written.len() <= 2,
            "case {permissions:?}: a table grant writes at most the table and its columns, \
             got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// A revoke reaches every scope the matching grant wrote at
// ---------------------------------------------------------------------------

/// `RevokePermissions` naming a TABLE takes back what `GrantPermissions` on that table
/// wrote, at BOTH scopes: the table's own record and the column-scoped one.
///
/// The rows differ along which part of the grant is revoked: all of it, only the
/// column-scoped permission, only the table-scoped one, and one that was never granted
/// (refused, changing nothing).
#[tokio::test]
async fn a_revoke_naming_the_table_reaches_both_scopes() {
    let catalog = account();
    let principal = "IAM_ALLOWED_PRINCIPALS";
    let on_table = json!({"Table": {"DatabaseName": "sales", "Name": "orders"}});
    let table = json!({"Table": {"CatalogId": catalog, "DatabaseName": "sales", "Name": "orders"}});
    let all_columns = json!({
        "TableWithColumns": {
            "CatalogId": catalog,
            "DatabaseName": "sales",
            "Name": "orders",
            "ColumnWildcard": {}
        }
    });

    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Vec<&str>, u16, Vec<(Value, Vec<&'static str>)>)> = vec![
        (
            "both scopes at once",
            vec!["SELECT", "DESCRIBE"],
            200,
            vec![],
        ),
        (
            "only the column-scoped permission",
            vec!["SELECT"],
            200,
            vec![(table.clone(), vec!["DESCRIBE"])],
        ),
        (
            "only the table-scoped permission",
            vec!["DESCRIBE"],
            200,
            vec![(all_columns.clone(), vec!["SELECT"])],
        ),
        (
            "a permission that was never granted",
            vec!["DROP"],
            400,
            vec![
                (table.clone(), vec!["DESCRIBE"]),
                (all_columns.clone(), vec!["SELECT"]),
            ],
        ),
    ];

    for (case, revoked, want_status, remaining) in cases {
        // One emulator per row, so each row revokes from the same starting state: the
        // grant of SELECT + DESCRIBE on the table, which is two records.
        let router = emulator();
        create_database(&router, "sales").await;
        create_table(&router, "sales", "orders").await;
        // The row counts the records its own grant wrote, so what creating the table
        // granted is revoked first (see the `creator_table_grants.rs` suite).
        revoke_table_default_grants(&router, "sales", "orders").await;
        let (status, body) = grant(&router, principal, &on_table, &["SELECT", "DESCRIBE"]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the seeding grant expected HTTP 200, got {status} {body}"
        );
        let body = list(&router, case, &json!({"Resource": on_table})).await;
        assert_eq!(
            records(&body).len(),
            2,
            "case {case:?}: the grant must leave the table and column records, got {body}"
        );

        let (status, body) = lake_formation(
            &router,
            "RevokePermissions",
            &json!({
                "Principal": {"DataLakePrincipalIdentifier": principal},
                "Resource": on_table,
                "Permissions": revoked,
            }),
        )
        .await;
        assert_eq!(
            status, want_status,
            "case {case:?}: expected HTTP {want_status}, got {status} {body}"
        );
        if want_status != 200 {
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {case:?}: expected InvalidInputException, got {body}"
            );
        }

        let body = list(&router, case, &json!({"Resource": on_table})).await;
        assert_eq!(
            records(&body),
            expected(remaining),
            "case {case:?}: unexpected records after the revoke, got {body}"
        );
    }
}

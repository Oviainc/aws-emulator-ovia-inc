// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the Lake Formation permission grant store:
//! `GrantPermissions`, `ListPermissions`, `RevokePermissions`.
//!
//! Every call goes over the wire the way an SDK client does — a `restJson1` `POST` of a
//! JSON body through the router that claims `lakeformation` — so a response can only be
//! what the handler computed from the single shared engine store. Each suite under
//! `tests/all/` carries its own dispatch harness, which is the convention this crate's
//! other suite (`lf_tags.rs`) already follows.
//!
//! The tables below span the classes the rules generalise over (resource kind, permission
//! set, repeated grant, filter member, page bound); the issue's own request — a grant to
//! `IAM_ALLOWED_PRINCIPALS` on a table, granted twice and revoked — is one row among
//! them.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one copy of the harness every permission suite shares: the Glue+LakeFormation
// emulator, the seeded catalog, the `restJson1` wire path and the error-code extractor.
use crate::glue_catalog::{
    account_catalog, create_database, emulator, error_code, lake_formation as call,
    revoke_default_database_grant, revoke_table_default_grants,
};

/// One row of the repeated-grant table: the case name, the permissions each successive
/// `GrantPermissions` call names, the permissions the records hold afterwards, and how
/// many records that is — one per SCOPE the permissions apply at (a table's own record,
/// its "table w columns" record, or both).
type RepeatedGrantCase = (
    &'static str,
    Vec<Vec<&'static str>>,
    Vec<&'static str>,
    usize,
);

/// One row of the absent-revoke table: the case name, what was granted first (if
/// anything), and the principal, resource and permissions the revoke names.
type AbsentRevokeCase = (
    &'static str,
    Option<Vec<&'static str>>,
    &'static str,
    Value,
    Vec<&'static str>,
);

/// One row of the partial-revoke table: the case name, the permissions granted, those
/// granted WITH the grant option, those revoked, and what remains afterwards (`None`
/// when the record itself is gone).
type PartialRevokeCase = (
    &'static str,
    Vec<&'static str>,
    Vec<&'static str>,
    Vec<&'static str>,
    Option<Vec<&'static str>>,
);

/// The Data Catalog this suite's rows grant on: the `sales` database with its `orders`
/// and `returns` tables, seeded through the shared harness.
///
/// This suite counts the records its own grants wrote, so the grants creating the two
/// tables made — their creator's and the database's table-level default — are revoked
/// first, the same reason its rows revoke the database's creation-time default. Those
/// grants are covered by the `creator_table_grants.rs` and `table_default_grants.rs`
/// suites.
async fn emulator_with_catalog() -> MultiServiceRouter {
    let router =
        crate::glue_catalog::emulator_with_catalog(&[("sales", &["orders", "returns"])]).await;
    for table in ["orders", "returns"] {
        revoke_table_default_grants(&router, "sales", table).await;
    }
    router
}

/// `GrantPermissions`, asserting it was accepted.
async fn grant(
    router: &MultiServiceRouter,
    case: &str,
    principal: &str,
    resource: &Value,
    permissions: &[&str],
) {
    let body = json!({
        "Principal": { "DataLakePrincipalIdentifier": principal },
        "Resource": resource,
        "Permissions": permissions,
    });
    let (status, payload) = call(router, "GrantPermissions", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GrantPermissions to answer 200, got {status} {payload:?}"
    );
}

/// The `PrincipalResourcePermissions` of a `ListPermissions` call, asserting 200 and that
/// the member is present (an empty listing is an EMPTY LIST, never an absent member).
async fn list(router: &MultiServiceRouter, case: &str, body: &Value) -> Vec<Value> {
    let (status, payload) = call(router, "ListPermissions", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListPermissions to answer 200, got {status} {payload:?}"
    );
    let entries = payload.get("PrincipalResourcePermissions").cloned();
    let entries = entries.unwrap_or_else(|| {
        panic!("case {case:?}: ListPermissions must carry PrincipalResourcePermissions, got {payload:?}")
    });
    entries
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: PrincipalResourcePermissions must be a list, got {entries:?}")
        })
        .clone()
}

/// The `Permissions` of one entry, as owned strings.
fn permissions_of(entry: &Value) -> Vec<String> {
    entry
        .get("Permissions")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `DataLakePrincipalIdentifier` of one entry.
fn principal_of(entry: &Value) -> String {
    entry
        .get("Principal")
        .and_then(|principal| principal.get("DataLakePrincipalIdentifier"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_grant_is_read_back_on_every_resource_kind_the_model_carries() {
    // One row per member of `Resource`
    // (`smithy_query(query="closure",operation="ListPermissions")`), each with the
    // permissions `Permission` admits for it (`PERMISSION_VALUES`), and the resource the
    // listing must echo: the catalog id the model documents a default for is filled in
    // ("By default, it is the account ID of the caller." — `TableResource$CatalogId`),
    // while `CatalogResource$Id` and `DataCellsFilterResource$TableCatalogId`, which
    // document no default, are echoed exactly as granted.
    let catalog = account_catalog();
    let cases: Vec<(&str, Value, Vec<&str>, Value)> = vec![
        (
            "catalog",
            json!({ "Catalog": {} }),
            vec!["CREATE_DATABASE"],
            json!({ "Catalog": {} }),
        ),
        (
            "database",
            json!({ "Database": { "Name": "sales" } }),
            vec!["CREATE_TABLE", "DESCRIBE"],
            json!({ "Database": { "CatalogId": catalog, "Name": "sales" } }),
        ),
        (
            "table",
            json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } }),
            vec!["ALL"],
            json!({ "Table": { "CatalogId": catalog, "DatabaseName": "sales", "Name": "orders" } }),
        ),
        (
            "table wildcard",
            json!({ "Table": { "DatabaseName": "sales", "TableWildcard": {} } }),
            vec!["SELECT"],
            json!({
                "Table": { "CatalogId": catalog, "DatabaseName": "sales", "TableWildcard": {} }
            }),
        ),
        (
            "table with named columns",
            json!({
                "TableWithColumns": {
                    "DatabaseName": "sales",
                    "Name": "orders",
                    "ColumnNames": ["amount", "currency"],
                }
            }),
            vec!["SELECT"],
            json!({
                "TableWithColumns": {
                    "CatalogId": catalog,
                    "DatabaseName": "sales",
                    "Name": "orders",
                    "ColumnNames": ["amount", "currency"],
                }
            }),
        ),
        (
            "table with a column wildcard",
            json!({
                "TableWithColumns": {
                    "DatabaseName": "sales",
                    "Name": "returns",
                    "ColumnWildcard": { "ExcludedColumnNames": ["ssn"] },
                }
            }),
            vec!["SELECT"],
            json!({
                "TableWithColumns": {
                    "CatalogId": catalog,
                    "DatabaseName": "sales",
                    "Name": "returns",
                    "ColumnWildcard": { "ExcludedColumnNames": ["ssn"] },
                }
            }),
        ),
        (
            "data location",
            json!({ "DataLocation": { "ResourceArn": "arn:aws:s3:::lake/raw" } }),
            vec!["DATA_LOCATION_ACCESS"],
            json!({
                "DataLocation": { "CatalogId": catalog, "ResourceArn": "arn:aws:s3:::lake/raw" }
            }),
        ),
        (
            "data cells filter",
            json!({
                "DataCellsFilter": {
                    "DatabaseName": "sales",
                    "TableName": "orders",
                    "Name": "eu-rows",
                }
            }),
            vec!["SELECT"],
            json!({
                "DataCellsFilter": {
                    "TableCatalogId": catalog,
                    "DatabaseName": "sales",
                    "TableName": "orders",
                    "Name": "eu-rows",
                }
            }),
        ),
        (
            "lf-tag",
            json!({ "LFTag": { "TagKey": "env", "TagValues": ["dev", "prod"] } }),
            vec!["ASSOCIATE"],
            json!({
                "LFTag": { "CatalogId": catalog, "TagKey": "env", "TagValues": ["dev", "prod"] }
            }),
        ),
        (
            "lf-tag policy",
            json!({
                "LFTagPolicy": {
                    "ResourceType": "TABLE",
                    "Expression": [{ "TagKey": "env", "TagValues": ["prod"] }],
                }
            }),
            vec!["SELECT", "DESCRIBE"],
            json!({
                "LFTagPolicy": {
                    "CatalogId": catalog,
                    "ResourceType": "TABLE",
                    "Expression": [{ "TagKey": "env", "TagValues": ["prod"] }],
                }
            }),
        ),
        (
            "lf-tag expression",
            json!({ "LFTagExpression": { "Name": "prod-only" } }),
            vec!["GRANT_WITH_LF_TAG_EXPRESSION"],
            json!({ "LFTagExpression": { "CatalogId": catalog, "Name": "prod-only" } }),
        ),
        (
            "rls policy",
            json!({ "RLSPolicy": { "Name": "eu-only" } }),
            vec!["SELECT"],
            json!({ "RLSPolicy": { "CatalogId": catalog, "Name": "eu-only" } }),
        ),
    ];

    for (case, resource, permissions, expected_resource) in cases {
        // A fresh emulator per row, so a listing's single entry can only be this grant —
        // with the database's creation-time default grant revoked, since this row is about
        // the record the grant wrote rather than the one the database was created with.
        let router = emulator_with_catalog().await;
        revoke_default_database_grant(&router, "sales").await;
        let principal = format!("arn:aws:iam::{}:role/analyst", account_catalog());
        grant(&router, case, &principal, &resource, &permissions).await;

        let entries = list(&router, case, &json!({ "Resource": resource })).await;
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: expected exactly one entry on the granted resource, got {entries:?}"
        );
        let entry = &entries[0];
        assert_eq!(
            principal_of(entry),
            principal,
            "case {case:?}: expected the grantee {principal:?}, got {entry:?}"
        );
        assert_eq!(
            permissions_of(entry),
            permissions,
            "case {case:?}: expected the granted permissions {permissions:?}, got {entry:?}"
        );
        assert_eq!(
            entry.get("Resource"),
            Some(&expected_resource),
            "case {case:?}: expected the resource {expected_resource:?}, got {entry:?}"
        );
        assert_eq!(
            entry.get("PermissionsWithGrantOption"),
            Some(&json!([])),
            "case {case:?}: a grant with no grant option carries an empty list, got {entry:?}"
        );
    }
}

#[tokio::test]
async fn granting_again_unions_into_the_records_it_already_holds() {
    // Rows differ along what the rule generalises over: the same permissions repeated, a
    // disjoint second set, an overlapping one, more than two calls, and a repeat inside
    // one call.
    let resource = json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } });
    let cases: Vec<RepeatedGrantCase> = vec![
        // The issue's own case: the same grant twice is exactly one entry.
        ("repeated", vec![vec!["ALL"], vec!["ALL"]], vec!["ALL"], 1),
        (
            "disjoint",
            vec![vec!["SELECT"], vec!["ALTER"]],
            vec!["ALTER", "SELECT"],
            2,
        ),
        (
            "overlapping",
            vec![vec!["SELECT", "DESCRIBE"], vec!["DESCRIBE", "DROP"]],
            vec!["DESCRIBE", "DROP", "SELECT"],
            2,
        ),
        (
            "three calls",
            vec![vec!["SELECT"], vec!["SELECT"], vec!["INSERT"]],
            vec!["INSERT", "SELECT"],
            2,
        ),
        (
            "repeated within one call",
            vec![vec!["SELECT", "SELECT"]],
            vec!["SELECT"],
            1,
        ),
    ];

    for (case, grants, expected, scopes_expected) in cases {
        let router = emulator_with_catalog().await;
        for permissions in &grants {
            grant(
                &router,
                case,
                "IAM_ALLOWED_PRINCIPALS",
                &resource,
                permissions,
            )
            .await;
        }

        let entries = list(&router, case, &json!({ "Resource": resource })).await;
        // A table grant is recorded at the SCOPE each permission applies at, so a
        // column-scoped `SELECT` is its own "table w columns" entry beside the table's
        // own (see `crate::state::scoped_grant_records`). What repeating a grant must not
        // do is open another entry at a scope: there is EXACTLY one entry per scope the
        // permissions apply at, and together they hold exactly the union granted.
        assert_eq!(
            entries.len(),
            scopes_expected,
            "case {case:?}: {} grants leave {scopes_expected} record(s), got {entries:?}",
            grants.len()
        );
        let scopes: BTreeSet<String> = entries
            .iter()
            .map(|entry| entry["Resource"].to_string())
            .collect();
        assert_eq!(
            scopes.len(),
            entries.len(),
            "case {case:?}: no scope may carry two records, got {entries:?}"
        );
        let mut held: Vec<String> = entries.iter().flat_map(permissions_of).collect();
        held.sort();
        let want: Vec<String> = expected
            .iter()
            .map(|permission| permission.to_string())
            .collect();
        assert_eq!(
            held, want,
            "case {case:?}: expected the union {want:?}, got {entries:?}"
        );
    }
}

#[tokio::test]
async fn revoking_what_was_never_granted_is_refused_and_changes_nothing() {
    // Rows differ along WHY nothing is there to revoke: no grant at all, a permission the
    // grant does not carry, another principal's grant, another resource, another catalog.
    let resource = json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } });
    let other_resource = json!({ "Table": { "DatabaseName": "sales", "Name": "returns" } });
    let cases: Vec<AbsentRevokeCase> = vec![
        // The issue's own case: a revoke of an absent IAM_ALLOWED_PRINCIPALS grant.
        (
            "no grant at all",
            None,
            "IAM_ALLOWED_PRINCIPALS",
            resource.clone(),
            vec!["ALL"],
        ),
        (
            "permission not held",
            Some(vec!["SELECT"]),
            "IAM_ALLOWED_PRINCIPALS",
            resource.clone(),
            vec!["DROP"],
        ),
        (
            "one held and one not",
            Some(vec!["SELECT"]),
            "IAM_ALLOWED_PRINCIPALS",
            resource.clone(),
            vec!["SELECT", "DROP"],
        ),
        (
            "another principal",
            Some(vec!["ALL"]),
            "arn:aws:iam::111122223333:role/other",
            resource.clone(),
            vec!["ALL"],
        ),
        (
            "another resource",
            Some(vec!["ALL"]),
            "IAM_ALLOWED_PRINCIPALS",
            other_resource,
            vec!["ALL"],
        ),
    ];

    for (case, granted, principal, revoked_resource, revoked) in cases {
        let router = emulator_with_catalog().await;
        if let Some(permissions) = &granted {
            grant(
                &router,
                case,
                "IAM_ALLOWED_PRINCIPALS",
                &resource,
                permissions,
            )
            .await;
        }

        let (status, payload) = call(
            &router,
            "RevokePermissions",
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": principal },
                "Resource": revoked_resource,
                "Permissions": revoked,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a revoke of {revoked:?} to be refused with 400, got \
             {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {payload:?}"
        );
        // The message names the permissions THIS request asked to revoke and the grantee
        // does not hold — the part of it built from the request.
        let message = payload
            .get("message")
            .or_else(|| payload.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let unheld: Vec<&str> = revoked
            .iter()
            .copied()
            .filter(|permission| {
                granted
                    .as_ref()
                    .is_none_or(|held| !held.contains(permission))
            })
            .collect();
        for permission in &unheld {
            assert!(
                message.contains(permission),
                "case {case:?}: expected the refusal to name {permission:?}, got {message:?}"
            );
        }

        // The refusal took nothing away: whatever was granted is still there.
        let entries = list(&router, case, &json!({ "Resource": resource })).await;
        match &granted {
            Some(permissions) => {
                assert_eq!(
                    entries.len(),
                    1,
                    "case {case:?}: the refused revoke must leave the grant, got {entries:?}"
                );
                assert_eq!(
                    permissions_of(&entries[0]),
                    *permissions,
                    "case {case:?}: expected {permissions:?} to still be held, got {:?}",
                    entries[0]
                );
            }
            None => assert!(
                entries.is_empty(),
                "case {case:?}: nothing was granted, so nothing is listed, got {entries:?}"
            ),
        }
    }
}

#[tokio::test]
async fn a_revoke_takes_away_only_what_it_names() {
    // Rows differ along how much of the grant the revoke names: a part, every part (which
    // drops the record), and the grant-option set alone.
    let resource = json!({ "Database": { "Name": "sales" } });
    let cases: Vec<PartialRevokeCase> = vec![
        (
            "one of two",
            vec!["SELECT", "ALTER"],
            vec![],
            vec!["SELECT"],
            Some(vec!["ALTER"]),
        ),
        (
            "both of two",
            vec!["SELECT", "ALTER"],
            vec![],
            vec!["SELECT", "ALTER"],
            None,
        ),
        (
            "the only one",
            vec!["ALL"],
            vec![],
            vec!["ALL"],
            None,
        ),
        (
            "leaving the grant option",
            vec!["SELECT"],
            vec!["SELECT"],
            vec!["SELECT"],
            Some(vec![]),
        ),
    ];

    for (case, granted, with_grant_option, revoked, remaining) in cases {
        // The database's creation-time default grant goes first: this table is about what
        // a revoke takes out of the record its own grant wrote.
        let router = emulator_with_catalog().await;
        revoke_default_database_grant(&router, "sales").await;
        let (status, payload) = call(
            &router,
            "GrantPermissions",
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
                "Resource": resource,
                "Permissions": granted,
                "PermissionsWithGrantOption": with_grant_option,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the grant to be accepted, got {status} {payload:?}"
        );

        let (status, payload) = call(
            &router,
            "RevokePermissions",
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
                "Resource": resource,
                "Permissions": revoked,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the revoke of {revoked:?} to be accepted, got {status} \
             {payload:?}"
        );

        let entries = list(&router, case, &json!({ "Resource": resource })).await;
        match &remaining {
            Some(permissions) => {
                assert_eq!(
                    entries.len(),
                    1,
                    "case {case:?}: a partly revoked grant is still one entry, got {entries:?}"
                );
                assert_eq!(
                    permissions_of(&entries[0]),
                    *permissions,
                    "case {case:?}: expected {permissions:?} to remain, got {:?}",
                    entries[0]
                );
            }
            None => assert!(
                entries.is_empty(),
                "case {case:?}: a fully revoked grant is dropped, got {entries:?}"
            ),
        }
    }
}

#[tokio::test]
async fn the_filters_select_by_principal_resource_and_resource_type() {
    let router = emulator_with_catalog().await;
    // This table is about the filters over the explicit grant store, so the database's
    // creation-time default grant is revoked before the grants below are made.
    revoke_default_database_grant(&router, "sales").await;
    let catalog = account_catalog();
    let analyst = format!("arn:aws:iam::{catalog}:role/analyst");
    let auditor = format!("arn:aws:iam::{catalog}:role/auditor");
    let database = json!({ "Database": { "Name": "sales" } });
    let table = json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } });
    let location = json!({ "DataLocation": { "ResourceArn": "arn:aws:s3:::lake/raw" } });

    grant(&router, "seed", &analyst, &database, &["DESCRIBE"]).await;
    grant(&router, "seed", &analyst, &table, &["SELECT"]).await;
    grant(&router, "seed", &auditor, &table, &["DESCRIBE"]).await;
    grant(
        &router,
        "seed",
        &analyst,
        &location,
        &["DATA_LOCATION_ACCESS"],
    )
    .await;

    // Rows differ along which filter member narrows the listing, including the catalog
    // that holds no grant at all and a `ResourceType` value with no grant of its kind
    // (`DATA_LAKE_RESOURCE_TYPE_VALUES` carries nine values; CATALOG is tested here, the
    // kinds themselves above).
    let cases: Vec<(&str, Value, usize)> = vec![
        ("no filter", json!({}), 4),
        (
            "principal",
            json!({ "Principal": { "DataLakePrincipalIdentifier": analyst } }),
            3,
        ),
        (
            "other principal",
            json!({ "Principal": { "DataLakePrincipalIdentifier": auditor } }),
            1,
        ),
        ("resource: the table", json!({ "Resource": table }), 2),
        (
            "resource: the database",
            json!({ "Resource": database }),
            1,
        ),
        (
            "resource and principal",
            json!({
                "Resource": table,
                "Principal": { "DataLakePrincipalIdentifier": auditor },
            }),
            1,
        ),
        ("resource type: TABLE", json!({ "ResourceType": "TABLE" }), 2),
        (
            "resource type: DATABASE",
            json!({ "ResourceType": "DATABASE" }),
            1,
        ),
        (
            "resource type: DATA_LOCATION",
            json!({ "ResourceType": "DATA_LOCATION" }),
            1,
        ),
        (
            "resource type with no grant: CATALOG",
            json!({ "ResourceType": "CATALOG" }),
            0,
        ),
        (
            "another catalog",
            json!({ "CatalogId": "111122223333" }),
            0,
        ),
        (
            "principal with no grant",
            json!({ "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" } }),
            0,
        ),
    ];

    for (case, filter, expected) in cases {
        let entries = list(&router, case, &filter).await;
        assert_eq!(
            entries.len(),
            expected,
            "case {case:?}: expected {expected} entries for {filter:?}, got {entries:?}"
        );
    }
}

#[tokio::test]
async fn a_table_listing_also_reports_the_column_level_grant_on_that_table() {
    // `ListPermissions$Resource`: "This operation does not support getting privileges on a
    // table with columns. Instead, call this operation on the table, and the operation
    // returns the table and the table w columns."
    let router = emulator_with_catalog().await;
    let principal = "IAM_ALLOWED_PRINCIPALS";
    let table = json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } });
    let with_columns = json!({
        "TableWithColumns": {
            "DatabaseName": "sales",
            "Name": "orders",
            "ColumnWildcard": {},
        }
    });
    let other_table_columns = json!({
        "TableWithColumns": {
            "DatabaseName": "sales",
            "Name": "returns",
            "ColumnWildcard": {},
        }
    });
    // The same cell filter, named WITHOUT the catalog id the model defaults to the
    // caller's account: one resource, so one record.
    let cells = json!({
        "DataCellsFilter": {
            "DatabaseName": "sales",
            "TableName": "orders",
            "Name": "eu-rows",
        }
    });
    let cells_with_catalog = json!({
        "DataCellsFilter": {
            "TableCatalogId": account_catalog(),
            "DatabaseName": "sales",
            "TableName": "orders",
            "Name": "eu-rows",
        }
    });

    grant(&router, "table", principal, &table, &["DESCRIBE"]).await;
    grant(&router, "columns", principal, &with_columns, &["SELECT"]).await;
    grant(
        &router,
        "other columns",
        principal,
        &other_table_columns,
        &["SELECT"],
    )
    .await;
    grant(&router, "cells", principal, &cells, &["SELECT"]).await;
    grant(
        &router,
        "cells named with the catalog id",
        principal,
        &cells_with_catalog,
        &["SELECT"],
    )
    .await;

    // Rows: the table listing carries the table and its column-level grant but not
    // another table's; the cell filter joins it only with IncludeRelated=TRUE
    // ("Set the field to TRUE to show the cell filters on a table resource").
    let cases: Vec<(&str, Value, usize)> = vec![
        ("the table", json!({ "Resource": table }), 2),
        (
            "the table, related included",
            json!({ "Resource": table, "IncludeRelated": "TRUE" }),
            3,
        ),
        (
            "the table, related excluded explicitly",
            json!({ "Resource": table, "IncludeRelated": "FALSE" }),
            2,
        ),
        (
            "the column-level resource itself",
            json!({ "Resource": with_columns }),
            1,
        ),
        ("the cell filter itself", json!({ "Resource": cells }), 1),
        (
            "the cell filter named with the catalog id",
            json!({ "Resource": cells_with_catalog }),
            1,
        ),
    ];

    for (case, filter, expected) in cases {
        let entries = list(&router, case, &filter).await;
        assert_eq!(
            entries.len(),
            expected,
            "case {case:?}: expected {expected} entries for {filter:?}, got {entries:?}"
        );
    }

    // "The Principal parameter must not be specified when requesting cell filter
    // information." — `ListPermissions$IncludeRelated`.
    let (status, payload) = call(
        &router,
        "ListPermissions",
        &json!({
            "Resource": table,
            "IncludeRelated": "TRUE",
            "Principal": { "DataLakePrincipalIdentifier": principal },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"principal with IncludeRelated\": expected 400, got {status} {payload:?}"
    );
    assert_eq!(
        error_code(&payload),
        "InvalidInputException",
        "case \"principal with IncludeRelated\": expected InvalidInputException, got {payload:?}"
    );
}

#[tokio::test]
async fn a_listing_pages_through_every_grant_exactly_once() {
    let router = emulator();
    let principal = "IAM_ALLOWED_PRINCIPALS";
    let databases: Vec<String> = (0..7).map(|index| format!("database-{index}")).collect();
    // Each grant is on a database the catalog holds, so each is accepted.
    for database in &databases {
        create_database(&router, database).await;
    }
    for database in &databases {
        grant(
            &router,
            "seed",
            principal,
            &json!({ "Database": { "Name": database } }),
            &["DESCRIBE"],
        )
        .await;
    }

    // `MaxResults` is modelled `range 1..1000`: every page size from the published lower
    // bound up, plus the upper bound, must walk the seven grants exactly once.
    for page_size in [1_i64, 2, 3, 6, 7, 8, 1000] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let (status, payload) = call(&router, "ListPermissions", &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 on page {pages}, got {status} {payload:?}"
            );
            let entries = payload["PrincipalResourcePermissions"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                entries.len() as i64 <= page_size,
                "case {case:?}: a page carries at most MaxResults entries, got {entries:?}"
            );
            for entry in &entries {
                seen.push(
                    entry["Resource"]["Database"]["Name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= 10,
                "case {case:?}: paging did not terminate after {pages} pages, saw {seen:?}"
            );
            match payload.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted, databases,
            "case {case:?}: expected every grant exactly once, saw {seen:?}"
        );
    }

    // One past each published bound is refused by the modelled `range 1..1000`.
    for page_size in [0_i64, 1001] {
        let case = format!("MaxResults={page_size}");
        let (status, payload) = call(
            &router,
            "ListPermissions",
            &json!({ "MaxResults": page_size }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults outside range 1..1000 is refused, got {status} {payload:?}"
        );
    }

    // A token this service did not mint is a bad request, not page one.
    let (status, payload) = call(
        &router,
        "ListPermissions",
        &json!({ "NextToken": "not-a-token" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"foreign NextToken\": expected 400, got {status} {payload:?}"
    );
    assert_eq!(
        error_code(&payload),
        "InvalidInputException",
        "case \"foreign NextToken\": expected InvalidInputException, got {payload:?}"
    );
}

#[tokio::test]
async fn a_resource_that_names_no_single_target_is_refused() {
    let router = emulator_with_catalog().await;
    // The closing assertion is that nothing was recorded, so the database's creation-time
    // default grant is revoked first and the listing can only hold what a call wrote.
    revoke_default_database_grant(&router, "sales").await;
    // Rows differ along HOW the `Resource` fails to name exactly one target: none, two,
    // a table named both ways, a table named neither way, columns named both ways and an
    // empty column list.
    let cases: Vec<(&str, Value)> = vec![
        ("no member", json!({})),
        (
            "two members",
            json!({
                "Database": { "Name": "sales" },
                "Table": { "DatabaseName": "sales", "Name": "orders" },
            }),
        ),
        (
            "name and wildcard",
            json!({
                "Table": { "DatabaseName": "sales", "Name": "orders", "TableWildcard": {} }
            }),
        ),
        (
            "neither name nor wildcard",
            json!({ "Table": { "DatabaseName": "sales" } }),
        ),
        (
            "columns and column wildcard",
            json!({
                "TableWithColumns": {
                    "DatabaseName": "sales",
                    "Name": "orders",
                    "ColumnNames": ["amount"],
                    "ColumnWildcard": {},
                }
            }),
        ),
        (
            "no column selection",
            json!({ "TableWithColumns": { "DatabaseName": "sales", "Name": "orders" } }),
        ),
        (
            "an empty column list",
            json!({
                "TableWithColumns": {
                    "DatabaseName": "sales",
                    "Name": "orders",
                    "ColumnNames": [],
                }
            }),
        ),
    ];

    for (case, resource) in cases {
        for operation in ["GrantPermissions", "RevokePermissions", "ListPermissions"] {
            let (status, payload) = call(
                &router,
                operation,
                &json!({
                    "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
                    "Resource": resource,
                    "Permissions": ["SELECT"],
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected 400, got {status} {payload:?}"
            );
            assert_eq!(
                error_code(&payload),
                "InvalidInputException",
                "case {case:?} on {operation}: expected InvalidInputException, got {payload:?}"
            );
        }
    }

    // Nothing above was recorded: the store is still empty.
    let entries = list(&router, "after the refusals", &json!({})).await;
    assert!(
        entries.is_empty(),
        "case \"after the refusals\": a refused grant records nothing, got {entries:?}"
    );
}

/// A permission value the `Permission` enum does not carry is refused on BOTH write
/// paths, and nothing is recorded for it.
///
/// `Permission` is modelled as an enum, and a value it does not carry is refused at the
/// boundary with `ValidationException` (HTTP 400) naming the member path and the admitted
/// value set, which is how the real service answers one: it never reaches the grant store,
/// so no later `ListPermissions` can report it. This pins that for both write paths, so a
/// handler can never start recording an unmodelled value.
///
/// The rows differ along WHERE the unmodelled value sits (alone, beside a modelled one, in
/// `PermissionsWithGrantOption`) and along its form (an invented name, a lower-cased
/// modelled value, a modelled value with trailing whitespace, the empty string); the last
/// row is the control, naming only values the enum carries, which must still be accepted
/// and recorded.
#[tokio::test]
async fn a_permission_the_model_does_not_carry_is_refused_on_both_write_paths() {
    // One row per class of unmodelled value, plus the accepted control. `Permissions`,
    // `PermissionsWithGrantOption`, and whether the request is expected to be accepted.
    let cases: Vec<(&str, Vec<&str>, Vec<&str>, bool)> = vec![
        ("an invented value alone", vec!["NOT_A_PERMISSION"], vec![], false),
        ("an invented value beside a modelled one", vec!["SELECT", "SUPER"], vec![], false),
        // `Permission`'s values are upper-case (`PERMISSION_VALUES`), so a lower-cased
        // spelling is a different string and not a member of the enum.
        ("a lower-cased modelled value", vec!["select"], vec![], false),
        ("a modelled value with trailing whitespace", vec!["SELECT "], vec![], false),
        ("the empty string", vec![""], vec![], false),
        (
            "an invented value in PermissionsWithGrantOption",
            vec!["SELECT"],
            vec!["SELECT", "NOT_A_PERMISSION"],
            false,
        ),
        // The control: every value here IS a member of the enum, so this one is recorded.
        ("only modelled values", vec!["SELECT"], vec!["SELECT"], true),
    ];

    let router = emulator_with_catalog().await;
    // Each row reads back the record its own grant wrote, so the database's creation-time
    // default grant — which a Principal-and-Resource listing would resolve onto the row's
    // principal — is revoked first.
    revoke_default_database_grant(&router, "sales").await;
    for (index, (case, permissions, with_grant_option, accepted)) in
        cases.into_iter().enumerate()
    {
        // One principal per row, so no row can read another's record.
        let principal = format!("arn:aws:iam::000000000000:role/enum-{index}");
        let resource = json!({ "Database": { "Name": "sales" } });
        let mut body = json!({
            "Principal": { "DataLakePrincipalIdentifier": principal },
            "Resource": resource,
            "Permissions": permissions,
        });
        if !with_grant_option.is_empty() {
            body["PermissionsWithGrantOption"] = json!(with_grant_option);
        }

        for operation in ["GrantPermissions", "RevokePermissions"] {
            // The revoke row for the accepted control runs after its grant below, so it
            // is the only one that may answer 200 here; every other row must be refused
            // by both paths for the same reason.
            if accepted && operation == "RevokePermissions" {
                continue;
            }
            let (status, payload) = call(&router, operation, &body).await;
            let expected = if accepted { 200 } else { 400 };
            assert_eq!(
                status, expected,
                "case {case:?}: expected {operation} to answer {expected}, got {status} {payload:?}"
            );
            if !accepted {
                let code = error_code(&payload);
                assert_eq!(
                    code, "ValidationException",
                    "case {case:?}: expected {operation} to refuse with ValidationException, \
                     got {code:?} {payload:?}"
                );
            }
        }

        let entries = list(
            &router,
            case,
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": principal },
                "Resource": resource,
            }),
        )
        .await;
        let stored: Vec<String> = entries.iter().flat_map(permissions_of).collect();
        let expected: Vec<String> = if accepted {
            vec!["SELECT".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the stored permissions to be {expected:?}, got {stored:?}"
        );
    }
}

/// One grant may name every `Permission` the model admits, and what it records is the SET
/// of the values it named — so the ceiling on how many DISTINCT permissions one grant can
/// push into a record is the `Permission` enum's own size, which a request naming only
/// modelled values can reach but never pass.
///
/// `PermissionList` carries no length constraint of its own, so the bound is inherent
/// rather than enforced: the boundary validator admits only values the `Permission` enum
/// carries, and the handler de-duplicates, so a stored record can hold at most
/// `state::MAX_PERMISSIONS_PER_GRANT` of them. The rows differ along how many times each
/// value is named: once, many times over, and every value twice — all three collapse to
/// the same stored set.
#[tokio::test]
async fn a_grant_records_the_set_of_the_permissions_it_names_up_to_the_modelled_ceiling() {
    use service_com_amazonaws_lakeformation_awslakeformation::state::MAX_PERMISSIONS_PER_GRANT;

    /// Every value of `Permission`
    /// (`smithy_query(query="shape",shape="Permission")`, rendered in this crate as
    /// `ops::types::PERMISSION_VALUES`).
    const MODELLED: [&str; 16] = [
        "ALL",
        "ALTER",
        "ASSOCIATE",
        "CREATE_CATALOG",
        "CREATE_DATABASE",
        "CREATE_LF_TAG",
        "CREATE_LF_TAG_EXPRESSION",
        "CREATE_TABLE",
        "DATA_LOCATION_ACCESS",
        "DELETE",
        "DESCRIBE",
        "DROP",
        "GRANT_WITH_LF_TAG_EXPRESSION",
        "INSERT",
        "SELECT",
        "SUPER_USER",
    ];

    // The ceiling IS the enum's size, so naming every modelled permission at once is
    // exactly at the bound and no request of modelled values can exceed it.
    assert_eq!(
        MODELLED.len(),
        MAX_PERMISSIONS_PER_GRANT,
        "case \"the ceiling\": expected the distinct-permission ceiling to be the size of \
         Permission ({}), got {MAX_PERMISSIONS_PER_GRANT}",
        MODELLED.len()
    );

    let every_twice: Vec<&str> = MODELLED.iter().chain(MODELLED.iter()).copied().collect();
    let repeated: Vec<&str> = std::iter::repeat_n("SELECT", 64).collect();
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "every modelled permission once",
            MODELLED.to_vec(),
            MODELLED.to_vec(),
        ),
        (
            "one permission named 64 times",
            repeated,
            vec!["SELECT"],
        ),
        (
            "every modelled permission twice",
            every_twice,
            MODELLED.to_vec(),
        ),
    ];

    let router = emulator_with_catalog().await;
    // Each row reads back the set its own grant recorded, so the database's creation-time
    // default grant is revoked first.
    revoke_default_database_grant(&router, "sales").await;
    for (index, (case, granted, held)) in cases.into_iter().enumerate() {
        // One principal per row, so the rows cannot merge into one another's record.
        let principal = format!("arn:aws:iam::000000000000:role/ceiling-{index}");
        let resource = json!({ "Database": { "Name": "sales" } });
        grant(&router, case, &principal, &resource, &granted).await;

        let entries = list(
            &router,
            case,
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": principal },
                "Resource": resource,
            }),
        )
        .await;
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: expected exactly one record, got {entries:?}"
        );
        let actual: BTreeSet<String> = permissions_of(&entries[0]).into_iter().collect();
        let expected: BTreeSet<String> = held.iter().map(|value| value.to_string()).collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored permissions to be {expected:?}, got {actual:?}"
        );
    }
}


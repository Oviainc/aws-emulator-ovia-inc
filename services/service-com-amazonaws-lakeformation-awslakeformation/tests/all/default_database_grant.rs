// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for what `ListPermissions` REPORTS of the grant a Data Catalog
//! database is created carrying: the account's `CreateDatabaseDefaultPermissions`, `ALL`
//! to `IAM_ALLOWED_PRINCIPALS`.
//!
//! Three rules are pinned here, each over a TABLE of inputs rather than one request:
//!
//! * a database holds that database-level grant from the moment it is created, whatever
//!   `DatabaseInput` the create sent — the table-level defaults a database hands to the
//!   TABLES in it (`Database$CreateTableDefaultPermissions`) are a different setting and
//!   change nothing about it;
//! * a listing that names BOTH a `Principal` and a `Resource` reports what that principal
//!   EFFECTIVELY holds ("If both Principal and Resource parameters are provided, the
//!   response returns effective permissions rather than the explicitly granted
//!   permissions" — `ListPermissions`), which is its own grants together with those of
//!   every group it belongs to, reported under the principal the request asked about;
//! * a revoke of that grant takes it out of BOTH views.
//!
//! `revoke_default_grant.rs` beside this file covers the revoke path's own refusals, and
//! `permissions.rs` the explicit grant store.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one copy of the harness every permission suite shares: the Glue+LakeFormation
// emulator, the Glue catalog writers and the `restJson1` wire path.
use crate::glue_catalog::{
    account_catalog, create_database, create_database_with_input, create_table, emulator,
    lake_formation as call, revoke_table_default_grants,
};

/// The group the default grant names, spelled as Lake Formation spells it on the wire.
const IAM_ALLOWED_PRINCIPALS: &str = "IAM_ALLOWED_PRINCIPALS";

/// The permission that grant carries.
const ALL: &str = "ALL";

/// The `PrincipalResourcePermissions` entries a `ListPermissions` body answers, asserting
/// 200 and that the member is present (an empty listing is an EMPTY list).
async fn list(router: &MultiServiceRouter, case: &str, body: &Value) -> Vec<Value> {
    let (status, payload) = call(router, "ListPermissions", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListPermissions to answer 200, got {status} {payload:?}"
    );
    payload
        .get("PrincipalResourcePermissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListPermissions must carry a PrincipalResourcePermissions list, got {payload:?}")
        })
}

/// One entry as the triple this suite compares: its principal, its resource and its
/// permissions.
fn triple(entry: &Value) -> (String, Value, Vec<String>) {
    let principal = entry
        .get("Principal")
        .and_then(|principal| principal.get("DataLakePrincipalIdentifier"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut permissions: Vec<String> = entry
        .get("Permissions")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    permissions.sort();
    (
        principal,
        entry.get("Resource").cloned().unwrap_or(Value::Null),
        permissions,
    )
}

/// The triples a listing carries, sorted so a comparison does not depend on the order the
/// service enumerated them in.
fn triples(entries: &[Value]) -> Vec<(String, Value, Vec<String>)> {
    let mut triples: Vec<(String, Value, Vec<String>)> = entries.iter().map(triple).collect();
    triples.sort_by_key(|(principal, resource, _)| format!("{principal}{resource}"));
    triples
}

/// The `Database` resource a listing filters on.
fn database_resource(name: &str) -> Value {
    json!({ "Database": { "Name": name } })
}

/// The `Database` resource a listing REPORTS, with the catalog id the model documents a
/// default for filled in ("The identifier for the Data Catalog. By default, the account
/// ID.") and the name folded the way the Data Catalog stores it.
fn reported_database(name: &str) -> Value {
    json!({ "Database": { "CatalogId": account_catalog(), "Name": name.to_lowercase() } })
}

/// `RevokePermissions` of the default grant on `database`, answering `(status, body)`.
async fn revoke_default(router: &MultiServiceRouter, database: &str) -> (u16, Value) {
    call(
        router,
        "RevokePermissions",
        &json!({
            "Principal": { "DataLakePrincipalIdentifier": IAM_ALLOWED_PRINCIPALS },
            "Resource": database_resource(database),
            "Permissions": [ALL],
        }),
    )
    .await
}

#[tokio::test]
async fn a_created_database_carries_the_default_grant_whatever_the_create_sent() {
    // Rows differ along the `DatabaseInput` the create sent: the name alone, the two
    // values `CreateTableDefaultPermissions` admits ("The only permitted values are an
    // empty array or an array that contains a single JSON object that grants ALL to
    // IAM_ALLOWED_PRINCIPALS" — `DataLakeSettings$CreateDatabaseDefaultPermissions`, the
    // same pair `Database$CreateTableDefaultPermissions` carries), the optional
    // description and location, and a mixed-case name the catalog folds when it stores it.
    let cases: Vec<(&str, &str, Value)> = vec![
        ("the name alone", "plain", json!({ "Name": "plain" })),
        (
            "no table-level defaults at all",
            "locked_tables",
            json!({ "Name": "locked_tables", "CreateTableDefaultPermissions": [] }),
        ),
        (
            "the IAM-access-control table-level default",
            "open_tables",
            json!({
                "Name": "open_tables",
                "CreateTableDefaultPermissions": [{
                    "Principal": { "DataLakePrincipalIdentifier": IAM_ALLOWED_PRINCIPALS },
                    "Permissions": [ALL],
                }],
            }),
        ),
        (
            "a description and a location",
            "warehouse",
            json!({
                "Name": "warehouse",
                "Description": "the warehouse",
                "LocationUri": "s3://lake-bucket/warehouse/",
            }),
        ),
        (
            "a mixed-case name",
            "Mixed_Case_DB",
            json!({ "Name": "Mixed_Case_DB" }),
        ),
    ];

    for (case, name, input) in cases {
        // A fresh emulator per row, so the listing can only be about this database.
        let router = emulator();
        create_database_with_input(&router, &input).await;

        let entries = list(&router, case, &json!({ "Resource": database_resource(name) })).await;
        assert_eq!(
            triples(&entries),
            vec![(
                IAM_ALLOWED_PRINCIPALS.to_string(),
                reported_database(name),
                vec![ALL.to_string()],
            )],
            "case {case:?}: expected the database to carry the default grant, got {entries:?}"
        );

        // The same grant is what the unfiltered listing reports, and it carries no grant
        // option: nothing may be granted onward through it.
        let entries = list(&router, case, &json!({})).await;
        assert_eq!(
            triples(&entries).len(),
            1,
            "case {case:?}: expected the one default grant in the unfiltered listing, got \
             {entries:?}"
        );
        assert_eq!(
            entries[0].get("PermissionsWithGrantOption"),
            Some(&json!([])),
            "case {case:?}: expected the default grant to carry no grant option, got {entries:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_naming_a_principal_and_a_resource_resolves_the_group_grant_onto_it() {
    // "If both Principal and Resource parameters are provided, the response returns
    // effective permissions rather than the explicitly granted permissions."
    // `IAM_ALLOWED_PRINCIPALS` is the group of every principal IAM itself allows, so an
    // IAM user or role holds what it holds. The rows differ along the FORM of the queried
    // identifier: the IAM principals IAM allows (a role, a user, a role under a path), the
    // group itself, and three identifiers that are not IAM principals — a SAML-decorated
    // principal, a QuickSight user and a bare account id.
    let catalog = account_catalog();
    let cases: Vec<(&str, String, bool)> = vec![
        ("an IAM role", format!("arn:aws:iam::{catalog}:role/analyst"), true),
        ("an IAM user", format!("arn:aws:iam::{catalog}:user/auditor"), true),
        (
            "an IAM role under a path",
            format!("arn:aws:iam::{catalog}:role/team/analyst"),
            true,
        ),
        ("the group itself", IAM_ALLOWED_PRINCIPALS.to_string(), true),
        (
            "a SAML-decorated principal",
            format!("arn:aws:iam::{catalog}:saml-provider/idp:user/jdoe"),
            false,
        ),
        (
            "a QuickSight user",
            format!("arn:aws:quicksight:us-east-1:{catalog}:user/default/analyst"),
            false,
        ),
        ("a bare account id", catalog.clone(), false),
    ];

    let router = emulator();
    create_database(&router, "sales").await;

    for (case, principal, resolved) in cases {
        let entries = list(
            &router,
            case,
            &json!({
                "Resource": database_resource("sales"),
                "Principal": { "DataLakePrincipalIdentifier": principal },
            }),
        )
        .await;
        let expected = if resolved {
            // Exactly one entry, carrying the group's `ALL` under the queried principal.
            vec![(
                principal.clone(),
                reported_database("sales"),
                vec![ALL.to_string()],
            )]
        } else {
            // Not a principal the group covers, and it holds nothing of its own.
            Vec::new()
        };
        assert_eq!(
            triples(&entries),
            expected,
            "case {case:?}: expected {expected:?}, got {entries:?}"
        );

        // The group grant is resolved only where the request asks about a principal AND a
        // resource: the unfiltered listing reports it under the group that holds it, and
        // names no other principal at all.
        let everyone = list(&router, case, &json!({})).await;
        assert_eq!(
            triples(&everyone),
            vec![(
                IAM_ALLOWED_PRINCIPALS.to_string(),
                reported_database("sales"),
                vec![ALL.to_string()],
            )],
            "case {case:?}: the unfiltered listing must report the grant under the group, \
             got {everyone:?}"
        );
    }
}

#[tokio::test]
async fn an_effective_listing_merges_the_principals_own_grant_with_the_group_grant() {
    // A principal's effective permissions are the union of what it holds in its own name
    // and what the group holds, as ONE entry: a listing carries one entry per (principal,
    // resource, condition) triple. The rows differ along what the principal was granted
    // itself: nothing, a permission the group grant does not carry, and one it does.
    let catalog = account_catalog();
    let principal = format!("arn:aws:iam::{catalog}:role/analyst");
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("nothing of its own", vec![], vec![ALL]),
        ("a narrower permission", vec!["DESCRIBE"], vec![ALL, "DESCRIBE"]),
        ("the same permission", vec![ALL], vec![ALL]),
    ];

    for (case, granted, effective) in cases {
        let router = emulator();
        create_database(&router, "sales").await;
        if !granted.is_empty() {
            let (status, payload) = call(
                &router,
                "GrantPermissions",
                &json!({
                    "Principal": { "DataLakePrincipalIdentifier": principal },
                    "Resource": database_resource("sales"),
                    "Permissions": granted,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the grant to be accepted, got {status} {payload:?}"
            );
        }

        let entries = list(
            &router,
            case,
            &json!({
                "Resource": database_resource("sales"),
                "Principal": { "DataLakePrincipalIdentifier": principal },
            }),
        )
        .await;
        let mut want: Vec<String> = effective.iter().map(|value| value.to_string()).collect();
        want.sort();
        assert_eq!(
            triples(&entries),
            vec![(principal.clone(), reported_database("sales"), want.clone())],
            "case {case:?}: expected the effective permissions {want:?} as one entry, got \
             {entries:?}"
        );
    }
}

#[tokio::test]
async fn revoking_the_default_grant_empties_both_views_for_that_database_alone() {
    // Rows differ along which database the listing is about after one database's default
    // is revoked: the revoked one, and two that were created beside it.
    let catalog = account_catalog();
    let principal = format!("arn:aws:iam::{catalog}:role/analyst");
    let router = emulator();
    for database in ["sales", "finance", "Mixed_Case_DB"] {
        create_database(&router, database).await;
    }

    // Before the revoke, every database carries its own default and the group holds them
    // all: three entries, one per database.
    let everyone = list(&router, "before the revoke", &json!({})).await;
    assert_eq!(
        triples(&everyone).len(),
        3,
        "case \"before the revoke\": expected one default grant per database, got {everyone:?}"
    );

    let (status, payload) = revoke_default(&router, "sales").await;
    assert_eq!(
        status, 200,
        "case \"revoke the default\": expected 200, got {status} {payload:?}"
    );

    let cases: Vec<(&str, &str, bool)> = vec![
        ("the revoked database", "sales", false),
        ("another database", "finance", true),
        ("a mixed-case database", "Mixed_Case_DB", true),
    ];
    for (case, database, still_held) in cases {
        // The view a reader of the resource takes.
        let entries = list(
            &router,
            case,
            &json!({ "Resource": database_resource(database) }),
        )
        .await;
        let expected = if still_held {
            vec![(
                IAM_ALLOWED_PRINCIPALS.to_string(),
                reported_database(database),
                vec![ALL.to_string()],
            )]
        } else {
            Vec::new()
        };
        assert_eq!(
            triples(&entries),
            expected,
            "case {case:?}: expected {expected:?} in the unfiltered view, got {entries:?}"
        );

        // And the view a reader asking "what does this role hold here?" takes.
        let effective = list(
            &router,
            case,
            &json!({
                "Resource": database_resource(database),
                "Principal": { "DataLakePrincipalIdentifier": principal },
            }),
        )
        .await;
        let expected = if still_held {
            vec![(
                principal.clone(),
                reported_database(database),
                vec![ALL.to_string()],
            )]
        } else {
            Vec::new()
        };
        assert_eq!(
            triples(&effective),
            expected,
            "case {case:?}: expected {expected:?} in the effective view, got {effective:?}"
        );
    }
}

#[tokio::test]
async fn the_default_grant_is_selected_by_exactly_the_filters_that_name_it() {
    // One row per filter member the operation carries (`CatalogId`, `Principal`,
    // `Resource`, `ResourceType`), on either side of what selects a DATABASE-level grant
    // held by the group. The seeded catalog holds `sales` with its `orders` table.
    let catalog = account_catalog();
    let router = emulator();
    create_database(&router, "sales").await;
    create_table(&router, "sales", "orders").await;
    // This suite is about the DATABASE-level default alone, so the grants creating the
    // table made (see the `creator_table_grants.rs` suite) are revoked first and every
    // count below
    // is the default's own.
    revoke_table_default_grants(&router, "sales", "orders").await;

    let cases: Vec<(&str, Value, usize)> = vec![
        ("no filter at all", json!({}), 1),
        (
            "the database itself",
            json!({ "Resource": database_resource("sales") }),
            1,
        ),
        (
            "the database named with its catalog id",
            json!({ "Resource": { "Database": { "CatalogId": catalog, "Name": "sales" } } }),
            1,
        ),
        (
            "a database the catalog does not hold",
            json!({ "Resource": database_resource("absent") }),
            0,
        ),
        (
            "a database in a Data Catalog this emulator holds nothing for",
            json!({ "Resource": { "Database": { "CatalogId": "210987654321", "Name": "sales" } } }),
            0,
        ),
        (
            "a table in the database",
            json!({ "Resource": { "Table": { "DatabaseName": "sales", "Name": "orders" } } }),
            0,
        ),
        (
            "every table in the database",
            json!({ "Resource": { "Table": { "DatabaseName": "sales", "TableWildcard": {} } } }),
            0,
        ),
        ("the catalog", json!({ "Resource": { "Catalog": {} } }), 0),
        (
            "ResourceType DATABASE",
            json!({ "ResourceType": "DATABASE" }),
            1,
        ),
        ("ResourceType TABLE", json!({ "ResourceType": "TABLE" }), 0),
        (
            "ResourceType CATALOG",
            json!({ "ResourceType": "CATALOG" }),
            0,
        ),
        (
            "the group that holds it",
            json!({ "Principal": { "DataLakePrincipalIdentifier": IAM_ALLOWED_PRINCIPALS } }),
            1,
        ),
        (
            "a principal with no resource, which is not an effective listing",
            json!({ "Principal": { "DataLakePrincipalIdentifier": format!("arn:aws:iam::{}:role/analyst", account_catalog()) } }),
            0,
        ),
        (
            "another Data Catalog",
            json!({ "CatalogId": "111122223333" }),
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
async fn every_databases_default_grant_is_paged_through_exactly_once() {
    // The defaults page with the explicit records, not beside them: whatever the page
    // size, a walk reports each database's default grant exactly once. `MaxResults` is
    // modelled `range 1..1000`, so the published lower bound and the sizes around the
    // entry count are all walked.
    let databases = ["alpha", "beta", "gamma", "delta"];
    let router = emulator();
    for database in databases {
        create_database(&router, database).await;
    }

    for page_size in [1_i64, 2, 3, 4, 5, 1000] {
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
            let entries = payload
                .get("PrincipalResourcePermissions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                entries.len() as i64 <= page_size,
                "case {case:?}: a page carries at most MaxResults entries, got {entries:?}"
            );
            for entry in &entries {
                let (principal, resource, permissions) = triple(entry);
                assert_eq!(
                    (principal.as_str(), permissions.clone()),
                    (IAM_ALLOWED_PRINCIPALS, vec![ALL.to_string()]),
                    "case {case:?}: expected the group's ALL, got {entry:?}"
                );
                seen.push(
                    resource["Database"]["Name"]
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
        let mut want: Vec<String> = databases.iter().map(|name| name.to_string()).collect();
        want.sort();
        assert_eq!(
            sorted, want,
            "case {case:?}: expected every database's default exactly once, saw {seen:?}"
        );
    }
}

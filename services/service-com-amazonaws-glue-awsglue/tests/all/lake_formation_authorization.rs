// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the authorization of `GetUnfilteredTableMetadata`, the Lake
//! Formation application-integration read of one table.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: this read is vended by Lake Formation, so its authorization is the
//! Lake Formation data access the REQUEST presents — the caller's own data `Permissions` on
//! the table ("Used to authorize the call when no view context is found"), or the
//! query-session context carrying the authorization identifier Lake Formation generated
//! (`QuerySessionContext$QueryAuthorizationId`). A request presenting neither is refused
//! with `AccessDeniedException` BEFORE the table it names is resolved, so what the caller
//! learns is that it may not read, never whether the table exists; a request presenting one
//! of them reaches the catalog, where an address that does not resolve is the catalog's own
//! `EntityNotFoundException`.
//!
//! The rows differ along what the rule generalises over: which authorization the request
//! presents (none, an empty list, a context carrying no identifier, one permission, several,
//! an identifier), and whether the database and table it names exist at all.

use serde_json::{json, Value};

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// Every value of the modelled `Permission` enum, which `Permissions` is a list of.
/// Source: `smithy_query shape=Permission`.
const PERMISSIONS: [&str; 9] = [
    "ALL",
    "SELECT",
    "ALTER",
    "DROP",
    "DELETE",
    "INSERT",
    "CREATE_DATABASE",
    "CREATE_TABLE",
    "DATA_LOCATION_ACCESS",
];

/// A `GetUnfilteredTableMetadata` body naming one table, carrying whatever `authorization`
/// adds and nothing else.
fn read(database: &str, name: &str, authorization: Value) -> Value {
    let mut body = json!({
        "CatalogId": ACCOUNT_ID,
        "DatabaseName": database,
        "Name": name,
        "SupportedPermissionTypes": ["COLUMN_PERMISSION"],
    });
    if let Some(members) = authorization.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// Store a database holding one table, the catalog state every row is read against.
async fn store_table(router: &smithy_server_runtime::MultiServiceRouter) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "lf_auth_db" } }),
    )
    .await;
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": "lf_auth_db",
            "TableInput": {
                "Name": "events",
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Location": "s3://lake/events/",
                    "Columns": [ { "Name": "id", "Type": "bigint" } ],
                },
            },
        }),
    )
    .await;
}

// ---------------------------------------------------------------------------
// A request presenting no Lake Formation authorization
// ---------------------------------------------------------------------------

/// A read presenting no Lake Formation authorization is refused with
/// `AccessDeniedException`, and the refusal does not depend on the address resolving: the
/// stored table, a table the database does not hold and a database the catalog does not
/// hold all earn the same answer, and none of them answers any metadata.
#[tokio::test]
async fn a_read_presenting_no_lake_formation_authorization_is_denied() {
    let router = emulator();
    store_table(&router).await;

    // (case, what the request presents instead of an authorization)
    let presentations: Vec<(&str, Value)> = vec![
        ("no authorization member at all", json!({})),
        ("an empty permission list", json!({ "Permissions": [] })),
        (
            "a query session context carrying no authorization identifier",
            json!({ "QuerySessionContext": { "QueryId": "11111111111111111111111111111111" } }),
        ),
        (
            "a query session context carrying no authorization identifier at all",
            json!({
                "QuerySessionContext": {
                    "ClusterId": "consumer-1",
                    "AdditionalContext": { "engine": "athena" },
                },
            }),
        ),
        (
            "a view chain, which addresses rather than authorizes",
            json!({
                "ParentResourceArn": "arn:aws:glue:us-east-1:000000000000:table/lf_auth_db/parent",
                "RootResourceArn": "arn:aws:glue:us-east-1:000000000000:table/lf_auth_db/root",
            }),
        ),
        (
            "an audit context, which records rather than authorizes",
            json!({ "AuditContext": { "AdditionalAuditContext": "query-1" } }),
        ),
    ];

    // (case, the database, the table) — the address resolves, or misses in each half.
    let addresses: [(&str, &str, &str); 3] = [
        ("the stored table", "lf_auth_db", "events"),
        ("a table the database does not hold", "lf_auth_db", "no_such_table"),
        ("a database the catalog does not hold", "no_such_db", "events"),
    ];

    for (presentation, authorization) in &presentations {
        for (address, database, name) in &addresses {
            let case = format!("{presentation} for {address}");
            let (status, body) = call(
                &router,
                "GetUnfilteredTableMetadata",
                &read(database, name, authorization.clone()),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case}: an unauthorized read must be refused, got {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with("AccessDeniedException"),
                "case {case}: expected AccessDeniedException, got {} in {body}",
                error_code(&body)
            );
            assert!(
                error_message(&body) == "<no Message>" && body.get("message").is_none(),
                "case {case}: the recorded refusal carries no message, got {body}"
            );
            assert!(
                body.get("Table").is_none(),
                "case {case}: a refused read answers no metadata, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A request presenting one
// ---------------------------------------------------------------------------

/// A read presenting Lake Formation authorization reaches the catalog: the stored table is
/// served, and an address that does not resolve is the catalog's own
/// `EntityNotFoundException` — which is what makes the refusal above about the
/// authorization and not about the table.
#[tokio::test]
async fn a_read_presenting_lake_formation_authorization_reaches_the_catalog() {
    let router = emulator();
    store_table(&router).await;
    let ungated = ok(
        &router,
        "GetTable",
        &json!({ "DatabaseName": "lf_auth_db", "Name": "events" }),
    )
    .await;

    // (case, the authorization the request presents) — one row per modelled permission,
    // plus every permission at once, plus the identifier Lake Formation generates.
    let mut presentations: Vec<(String, Value)> = PERMISSIONS
        .iter()
        .map(|permission| {
            (
                format!("the single permission {permission}"),
                json!({ "Permissions": [permission] }),
            )
        })
        .collect();
    presentations.push((
        "every modelled permission".to_string(),
        json!({ "Permissions": PERMISSIONS }),
    ));
    presentations.push((
        "a query session context carrying an authorization identifier".to_string(),
        json!({
            "QuerySessionContext": {
                "QueryId": "11111111111111111111111111111111",
                "QueryAuthorizationId": "22222222222222222222222222222222",
            },
        }),
    ));
    presentations.push((
        "both a permission list and an authorization identifier".to_string(),
        json!({
            "Permissions": ["SELECT"],
            "QuerySessionContext": { "QueryAuthorizationId": "33333333333333333333333333333333" },
        }),
    ));

    for (case, authorization) in &presentations {
        let (status, body) = call(
            &router,
            "GetUnfilteredTableMetadata",
            &read("lf_auth_db", "events", authorization.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: an authorized read must be served, got {status} with {body}"
        );
        assert_eq!(
            body["Table"], ungated["Table"],
            "case {case}: the authorized read must answer the definition the ungated read \
             answers; expected {}, got {}",
            ungated["Table"], body["Table"]
        );

        // The same authorization against an address that does not resolve is the catalog's
        // miss, so the authorization check is not standing in for the lookup.
        for (address, database, name) in [
            ("a table the database does not hold", "lf_auth_db", "no_such_table"),
            ("a database the catalog does not hold", "no_such_db", "events"),
        ] {
            let (status, body) = call(
                &router,
                "GetUnfilteredTableMetadata",
                &read(database, name, authorization.clone()),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case} for {address}: an address that does not resolve must be refused, \
                 got {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with("EntityNotFoundException"),
                "case {case} for {address}: expected EntityNotFoundException, got {} in {body}",
                error_code(&body)
            );
        }
    }

    // The reads changed nothing: the ungated read answers exactly what it answered before.
    let after = ok(
        &router,
        "GetTable",
        &json!({ "DatabaseName": "lf_auth_db", "Name": "events" }),
    )
    .await;
    assert_eq!(
        after, ungated,
        "the reads must leave the catalog as it was; expected {ungated}, got {after}"
    );
}

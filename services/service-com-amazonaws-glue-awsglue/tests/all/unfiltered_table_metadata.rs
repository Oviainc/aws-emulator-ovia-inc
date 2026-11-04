// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetUnfilteredTableMetadata`, the Lake Formation
//! application-integration read of ONE table.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: an AUTHORIZED read answers the table definition the catalog holds,
//! plus the Lake Formation facts about that table — all negative here, because no stored
//! table's location is registered with Lake Formation and no stored table is a view. Every
//! request below therefore carries the caller's own Lake Formation data permissions on the
//! table, the member the model documents as what authorizes the call when no view context
//! is found; what a request presenting NO such authorization earns is a rule of its own, in
//! `tests/all/lake_formation_authorization.rs`. A table of a FEDERATED database is NOT
//! answered from the catalog: that database "points to an entity outside the Glue Data
//! Catalog", so the metadata comes from the source its registration names, and a source
//! that cannot be reached is the retryable federation failure —
//! `FederationSourceRetryableException`, the code that tells a client to retry rather than
//! give up.
//!
//! The rows below differ along what the rule generalises over: which permission types the
//! call supports, which optional members it carries, how the federated database was
//! registered, and which part of the address is missing.

use serde_json::{json, Value};

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// Every value of the modelled `PermissionType` enum, which `SupportedPermissionTypes` is
/// a list of. Source: `smithy_query shape=PermissionType`.
const PERMISSION_TYPES: [&str; 4] = [
    "COLUMN_PERMISSION",
    "CELL_FILTER_PERMISSION",
    "NESTED_PERMISSION",
    "NESTED_CELL_PERMISSION",
];

/// The table definition every row of the success table is resolved against.
fn table_input(name: &str) -> Value {
    json!({
        "Name": name,
        "Description": "events the lake holds",
        "TableType": "EXTERNAL_TABLE",
        "Parameters": { "classification": "parquet" },
        "PartitionKeys": [ { "Name": "dt", "Type": "string" } ],
        "StorageDescriptor": {
            "Location": "s3://lake/events/",
            "Columns": [
                { "Name": "id", "Type": "bigint" },
                { "Name": "payload", "Type": "string" }
            ]
        }
    })
}

/// A `GetUnfilteredTableMetadata` body for one table, plus whatever `extra` adds.
///
/// It carries `Permissions`, the caller's own Lake Formation data permissions on the table
/// ("Used to authorize the call when no view context is found" —
/// `GetUnfilteredTableMetadataRequest$Permissions`), because this read is vended by Lake
/// Formation: a request presenting no Lake Formation authorization is refused before the
/// table is resolved at all.
fn unfiltered_body(database: &str, name: &str, permission_types: &[&str], extra: Value) -> Value {
    let mut body = json!({
        "CatalogId": ACCOUNT_ID,
        "DatabaseName": database,
        "Name": name,
        "SupportedPermissionTypes": permission_types,
        "Permissions": ["SELECT"],
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

// ---------------------------------------------------------------------------
// The catalog's own table
// ---------------------------------------------------------------------------

/// The read answers the stored table — the same definition the ungated `GetTable` answers —
/// and reports the table as registered with no Lake Formation, with no column withheld and
/// no filter applied.
///
/// The rows differ along which permission types the call supports (each value of the
/// modelled enum, and all of them at once) and which optional members it carries: an audit
/// context, a Region, a query session context, a supported dialect, a parent resource ARN.
#[tokio::test]
async fn the_read_answers_the_stored_table() {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "lf_db" } }),
    )
    .await;
    ok(
        &router,
        "CreateTable",
        &json!({ "DatabaseName": "lf_db", "TableInput": table_input("events") }),
    )
    .await;
    let ungated = ok(
        &router,
        "GetTable",
        &json!({ "DatabaseName": "lf_db", "Name": "events" }),
    )
    .await;

    // (case, the supported permission types, the extra members)
    let mut cases: Vec<(String, Vec<&str>, Value)> = Vec::new();
    for permission_type in PERMISSION_TYPES {
        cases.push((
            format!("supporting {permission_type}"),
            vec![permission_type],
            json!({}),
        ));
    }
    cases.push((
        "supporting every permission type".to_string(),
        PERMISSION_TYPES.to_vec(),
        json!({}),
    ));
    cases.push((
        "carrying an audit context and a Region".to_string(),
        vec![PERMISSION_TYPES[0]],
        json!({
            "AuditContext": { "AdditionalAuditContext": "query-1" },
            "Region": "us-east-1"
        }),
    ));
    cases.push((
        "carrying a query session context and a dialect".to_string(),
        vec![PERMISSION_TYPES[1]],
        json!({
            "QuerySessionContext": { "QueryId": "11111111111111111111111111111111" },
            "SupportedDialect": { "Dialect": "ATHENA", "DialectVersion": "3" },
            "ParentResourceArn": "arn:aws:athena:us-east-1:000000000000:workgroup/primary"
        }),
    ));

    for (case, permission_types, extra) in cases {
        let body = unfiltered_body("lf_db", "events", &permission_types, extra);
        let (status, answer) = call(&router, "GetUnfilteredTableMetadata", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: the stored table must be served, got {status} with {answer}"
        );
        assert_eq!(
            answer["Table"], ungated["Table"],
            "case {case}: the unfiltered read must answer the definition the ungated read \
             answers; expected {}, got {}",
            ungated["Table"], answer["Table"]
        );
        assert_eq!(
            answer["IsRegisteredWithLakeFormation"],
            json!(false),
            "case {case}: no stored table's location is registered with Lake Formation, got \
             {answer}"
        );
        for flag in ["IsMultiDialectView", "IsMaterializedView", "IsProtected"] {
            assert_eq!(
                answer[flag],
                json!(false),
                "case {case}: no stored table is a view or protected, so {flag} is false, got \
                 {answer}"
            );
        }
        for withheld in ["AuthorizedColumns", "CellFilters", "RowFilter", "Permissions"] {
            assert!(
                answer[withheld].is_null(),
                "case {case}: nothing withholds or filters a column here, so {withheld} is \
                 absent, got {answer}"
            );
        }
    }

    // A read changes nothing: the ungated read answers exactly what it answered before.
    let after = ok(
        &router,
        "GetTable",
        &json!({ "DatabaseName": "lf_db", "Name": "events" }),
    )
    .await;
    assert_eq!(
        after, ungated,
        "the unfiltered reads must leave the catalog as it was; expected {ungated}, got {after}"
    );
}

// ---------------------------------------------------------------------------
// A federated database's source
// ---------------------------------------------------------------------------

/// A table of a federated database is answered by the source that database points at, and
/// that source cannot be reached from here: the read fails with
/// `FederationSourceRetryableException`, naming the database and the source its
/// registration points at.
///
/// The rows differ along how the federated database was registered (a connection name, an
/// identifier, both), and along whether the table the request names was ever created in it
/// — the catalog holds no metadata of a federated database either way, so the answer is the
/// federation failure and not a miss.
#[tokio::test]
async fn a_table_of_a_federated_database_fails_retryably() {
    // (case, the FederatedDatabase registration, whether a table is created in it)
    let cases: [(&str, Value, bool); 4] = [
        (
            "registered with an identifier and a connection",
            json!({
                "Identifier": "hive://metastore/sales",
                "ConnectionName": "hive-metastore",
                "ConnectionType": "JDBC"
            }),
            true,
        ),
        (
            "registered with a connection name only",
            json!({ "ConnectionName": "redshift-federation" }),
            true,
        ),
        (
            "registered with an identifier only",
            json!({ "Identifier": "hive://metastore/marketing" }),
            true,
        ),
        (
            "a table the federated database was never given",
            json!({ "ConnectionName": "hive-metastore" }),
            false,
        ),
    ];

    for (index, (case, federated, create_table)) in cases.iter().enumerate() {
        let router = emulator();
        let database = format!("federated_db_{index}");
        ok(
            &router,
            "CreateDatabase",
            &json!({
                "DatabaseInput": { "Name": database, "FederatedDatabase": federated }
            }),
        )
        .await;
        if *create_table {
            ok(
                &router,
                "CreateTable",
                &json!({ "DatabaseName": database, "TableInput": table_input("events") }),
            )
            .await;
        }

        let (status, body) = call(
            &router,
            "GetUnfilteredTableMetadata",
            &unfiltered_body(&database, "events", &[PERMISSION_TYPES[0]], json!({})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: an unreachable federated source must fail the read, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("FederationSourceRetryableException"),
            "case {case}: expected FederationSourceRetryableException, got {} in {body}",
            error_code(&body)
        );
        assert!(
            error_message(&body).contains(&database),
            "case {case}: the failure must name the federated database, got {body}"
        );
        let source = federated["ConnectionName"]
            .as_str()
            .or_else(|| federated["Identifier"].as_str())
            .expect("the row registers a source");
        assert!(
            error_message(&body).contains(source),
            "case {case}: the failure must name the source the registration points at \
             ({source}), got {body}"
        );
        assert!(
            body["Table"].is_null(),
            "case {case}: a failed federation read answers no table, got {body}"
        );

        // The rule is about the READ going through the source, not about this one
        // operation: the single-table read and the listing of the same federated database
        // fail the same way.
        for (operation, request) in [
            (
                "GetTable",
                json!({ "DatabaseName": database, "Name": "events" }),
            ),
            ("GetTables", json!({ "DatabaseName": database })),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case}: {operation} of a federated database must fail too, got {status} \
                 with {body}"
            );
            assert!(
                error_code(&body).ends_with("FederationSourceRetryableException"),
                "case {case}: expected FederationSourceRetryableException from {operation}, \
                 got {} in {body}",
                error_code(&body)
            );
        }

        // The database's own registration is catalog metadata, so reading IT still works:
        // only the tables live in the source.
        let registered = ok(
            &router,
            "GetDatabase",
            &json!({ "Name": database }),
        )
        .await;
        assert_eq!(
            registered["Database"]["FederatedDatabase"], *federated,
            "case {case}: the registration itself is catalog metadata and must still be \
             readable, got {registered}"
        );
    }
}

// ---------------------------------------------------------------------------
// The misses
// ---------------------------------------------------------------------------

/// Each part of the address is resolved in turn, so a database the catalog does not hold
/// and a table its database does not hold are both `EntityNotFoundException`.
#[tokio::test]
async fn an_address_the_catalog_cannot_resolve_is_not_found() {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "miss_db" } }),
    )
    .await;
    ok(
        &router,
        "CreateTable",
        &json!({ "DatabaseName": "miss_db", "TableInput": table_input("events") }),
    )
    .await;

    for (case, database, name) in [
        ("a database the catalog does not hold", "no_such_db", "events"),
        ("a table the database does not hold", "miss_db", "no_such_table"),
    ] {
        let (status, body) = call(
            &router,
            "GetUnfilteredTableMetadata",
            &unfiltered_body(database, name, &[PERMISSION_TYPES[0]], json!({})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: an address that does not resolve must be refused, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {} in {body}",
            error_code(&body)
        );
    }
}

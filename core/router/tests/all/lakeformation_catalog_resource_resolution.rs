// SPDX-License-Identifier: CC0-1.0

//! Lake Formation resolves the Data Catalog resource a permission request names, and
//! records each permission at the scope it is held at — through the WHOLE assembled
//! router.
//!
//! The rules under test, both end-to-end over the wire:
//!
//! * A Lake Formation permission is held on a Data Catalog resource, so `GrantPermissions`
//!   resolves the database or table its `Resource` names against the Data Catalog and
//!   refuses a request naming one the catalog does not hold with HTTP 400
//!   `InvalidInputException`, recording nothing; a `Resource` member that is not a catalog
//!   database or table (a registered location, an LF-tag) names nothing to resolve and is
//!   served.
//! * The permission Lake Formation scopes to a table's COLUMNS is recorded as that table's
//!   "table w columns" entry and the rest as the table's own, which is why a table listing
//!   "returns the table and the table w columns" (`ListPermissions$Resource`).
//!
//! Both need two services on ONE engine — Lake Formation reads the catalog out of Glue's
//! own store — so they are driven here through `build_router`, the production assembly,
//! rather than through a hand-mounted pair: this is the suite that shows the resolution
//! works in the emulator a developer actually boots. The databases and tables are created
//! through Glue's own wire API (`awsJson1_1`) and the grants through Lake Formation's
//! (`restJson1`), so a response can only be what the handlers computed from stored state.
//!
//! The rows span what the rules generalise over: which `Resource` member carries the name
//! (`Database`, `Table` by name, `Table` by wildcard, `TableWithColumns`), which half of
//! the name the catalog does not hold (the database, the table, both), and a resource kind
//! that is not a catalog entity at all.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The database the catalog is seeded with, and a name it is never seeded with.
const PRESENT_DATABASE: &str = "sim_lake";
const ABSENT_DATABASE: &str = "sim_lake_missing";

/// The table the seeded database holds, and a name it never holds.
const PRESENT_TABLE: &str = "usage_events";
const ABSENT_TABLE: &str = "usage_events_missing";

/// The grantee every row below names.
const ANALYST: &str = "arn:aws:iam::123456789012:role/analyst";

/// What the refusal of a grant on a database the Data Catalog does not hold must say, and
/// the same for a table. Spelled out here rather than imported from the crate under test,
/// so the wording is a real expectation: the HTTP 400 and the `InvalidInputException` code
/// are the contract, and the message is the real service's for this error class, naming
/// only the KIND of entity it could not resolve.
const DATABASE_NOT_FOUND: &str = "Database not found.";
const TABLE_NOT_FOUND: &str = "Table not found.";

/// One row: the `Resource` a permission request names and whether the catalog resolves it.
struct ResolutionCase {
    case: &'static str,
    resource: Value,
    permissions: Vec<&'static str>,
    /// The message the refusal must carry when the catalog holds no such resource, and
    /// `None` for a row the catalog resolves. The message is what says WHICH kind of
    /// entity could not be resolved.
    refused_with: Option<&'static str>,
    /// How many records the grant must leave behind: `0` for a refused row, and otherwise
    /// one per SCOPE the permissions apply at — two for a table grant that names both a
    /// column-scoped and a table-scoped permission, one for every other row.
    records: usize,
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// Send one buffered request and answer its status with the parsed body.
fn send(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    path: &str,
    content_type: &str,
    target: Option<&str>,
    body: &Value,
) -> (u16, Value) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://{signing_name}.{REGION}.amazonaws.com{path}"
        ))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type);
    if let Some(target) = target {
        request = request.header("x-amz-target", target);
    }
    let request = request
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Drive one Glue operation (`awsJson1_1`).
fn glue(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    send(
        router,
        runtime,
        "glue",
        "/",
        "application/x-amz-json-1.1",
        Some(&format!("AWSGlue.{operation}")),
        body,
    )
}

/// Drive one Lake Formation operation (`restJson1`: the operation names the path).
fn lakeformation(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    send(
        router,
        runtime,
        "lakeformation",
        &format!("/{operation}"),
        "application/json",
        None,
        body,
    )
}

/// The wire error code, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("")
        .to_string()
}

/// The error message, whichever casing the envelope carries it under.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// A grant body.
fn grant_body(resource: &Value, permissions: &[&str]) -> Value {
    json!({
        "Principal": { "DataLakePrincipalIdentifier": ANALYST },
        "Resource": resource,
        "Permissions": permissions,
    })
}

/// How many entries `ListPermissions` reports for the grantee.
fn held_entries(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime) -> usize {
    let (status, body) = lakeformation(
        router,
        runtime,
        "ListPermissions",
        &json!({ "Principal": { "DataLakePrincipalIdentifier": ANALYST } }),
    );
    assert_eq!(
        status, 200,
        "ListPermissions must answer 200, got {status} {body}"
    );
    body.get("PrincipalResourcePermissions")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_else(|| panic!("PrincipalResourcePermissions must be a list, got {body}"))
}

fn resolution_cases() -> Vec<ResolutionCase> {
    vec![
        ResolutionCase {
            case: "a table the catalog holds",
            resource: json!({
                "Table": { "DatabaseName": PRESENT_DATABASE, "Name": PRESENT_TABLE }
            }),
            permissions: vec!["SELECT", "DESCRIBE"],
            refused_with: None,
            // The column-scoped permission is held on the table's columns and the other
            // on the table itself, so this one grant is two records.
            records: 2,
        },
        ResolutionCase {
            // The issue's own request.
            case: "a table the catalog does not hold",
            resource: json!({
                "Table": { "DatabaseName": PRESENT_DATABASE, "Name": ABSENT_TABLE }
            }),
            permissions: vec!["SELECT", "DESCRIBE"],
            refused_with: Some(TABLE_NOT_FOUND),
            records: 0,
        },
        ResolutionCase {
            case: "a database the catalog holds",
            resource: json!({ "Database": { "Name": PRESENT_DATABASE } }),
            permissions: vec!["DESCRIBE"],
            refused_with: None,
            records: 1,
        },
        ResolutionCase {
            // The issue's second request.
            case: "a database the catalog does not hold",
            resource: json!({ "Database": { "Name": ABSENT_DATABASE } }),
            permissions: vec!["DESCRIBE"],
            refused_with: Some(DATABASE_NOT_FOUND),
            records: 0,
        },
        ResolutionCase {
            case: "a table under a database the catalog does not hold",
            resource: json!({
                "Table": { "DatabaseName": ABSENT_DATABASE, "Name": PRESENT_TABLE }
            }),
            permissions: vec!["DESCRIBE"],
            // The database is resolved first, so it is the level the refusal names.
            refused_with: Some(DATABASE_NOT_FOUND),
            records: 0,
        },
        ResolutionCase {
            case: "every table under a database the catalog holds",
            resource: json!({
                "Table": { "DatabaseName": PRESENT_DATABASE, "TableWildcard": {} }
            }),
            permissions: vec!["DESCRIBE"],
            refused_with: None,
            records: 1,
        },
        ResolutionCase {
            case: "every table under a database the catalog does not hold",
            resource: json!({
                "Table": { "DatabaseName": ABSENT_DATABASE, "TableWildcard": {} }
            }),
            permissions: vec!["DESCRIBE"],
            refused_with: Some(DATABASE_NOT_FOUND),
            records: 0,
        },
        ResolutionCase {
            case: "columns of a table the catalog holds",
            resource: json!({
                "TableWithColumns": {
                    "DatabaseName": PRESENT_DATABASE,
                    "Name": PRESENT_TABLE,
                    "ColumnNames": ["amount"],
                }
            }),
            permissions: vec!["SELECT"],
            refused_with: None,
            records: 1,
        },
        ResolutionCase {
            case: "columns of a table the catalog does not hold",
            resource: json!({
                "TableWithColumns": {
                    "DatabaseName": PRESENT_DATABASE,
                    "Name": ABSENT_TABLE,
                    "ColumnNames": ["amount"],
                }
            }),
            permissions: vec!["SELECT"],
            refused_with: Some(TABLE_NOT_FOUND),
            records: 0,
        },
        ResolutionCase {
            // Not a catalog database or table: an Amazon S3 location is not an entity the
            // catalog can report absent, so nothing is claimed about it.
            case: "a data location",
            resource: json!({ "DataLocation": { "ResourceArn": "arn:aws:s3:::lake/raw" } }),
            permissions: vec!["DATA_LOCATION_ACCESS"],
            refused_with: None,
            records: 1,
        },
        ResolutionCase {
            // Nor is an LF-tag: it is this service's own record.
            case: "an lf-tag",
            resource: json!({ "LFTag": { "TagKey": "env", "TagValues": ["prod"] } }),
            permissions: vec!["ASSOCIATE"],
            refused_with: None,
            records: 1,
        },
    ]
}

#[test]
fn a_permission_is_recorded_only_on_a_resource_the_data_catalog_holds() {
    std::thread::Builder::new()
        .name("lakeformation-catalog-resolution".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // Seed the catalog: one database holding one table.
            let (status, body) = glue(
                &router,
                &runtime,
                "CreateDatabase",
                &json!({ "DatabaseInput": { "Name": PRESENT_DATABASE } }),
            );
            assert_eq!(status, 200, "setup: CreateDatabase got {status} {body}");
            let (status, body) = glue(
                &router,
                &runtime,
                "CreateTable",
                &json!({
                    "DatabaseName": PRESENT_DATABASE,
                    "TableInput": { "Name": PRESENT_TABLE },
                }),
            );
            assert_eq!(status, 200, "setup: CreateTable got {status} {body}");

            for case in resolution_cases() {
                let before = held_entries(&router, &runtime);
                let (status, body) = lakeformation(
                    &router,
                    &runtime,
                    "GrantPermissions",
                    &grant_body(&case.resource, &case.permissions),
                );
                let after = held_entries(&router, &runtime);
                if let Some(message) = case.refused_with {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (400, "InvalidInputException"),
                        "case {:?}: a grant on a resource the catalog does not hold must be \
                         refused 400 InvalidInputException, got {status} {body}",
                        case.case
                    );
                    assert_eq!(
                        error_message(&body),
                        message,
                        "case {:?}: the refusal must name the level the catalog could not \
                         resolve, got {body}",
                        case.case
                    );
                    assert_eq!(
                        after, before,
                        "case {:?}: a refused grant must record nothing, held {before} \
                         entries before and {after} after",
                        case.case
                    );
                } else {
                    assert_eq!(
                        status, 200,
                        "case {:?}: a grant on a resource the catalog holds must be served, \
                         got {status} {body}",
                        case.case
                    );
                    assert_eq!(
                        after,
                        before + case.records,
                        "case {:?}: the grant must leave {} record(s), held {before} entries \
                         before and {after} after",
                        case.case,
                        case.records
                    );
                }
            }

            // The round trip on the resource the catalog does hold: the grant of the
            // column-scoped permission beside a table one is held as two entries — the
            // table's own and the column-level one — and a table-filtered listing reports
            // both, which is what `ListPermissions$Resource` documents.
            //
            // This listing names a principal AND a resource, so it reports what that
            // principal EFFECTIVELY holds: beside what the grant wrote, the table carries
            // the default its database hands the tables created in it (`ALL` to the
            // IAM-allowed group), which an IAM role holds through that group.
            let table =
                json!({ "Table": { "DatabaseName": PRESENT_DATABASE, "Name": PRESENT_TABLE } });
            let (status, body) = lakeformation(
                &router,
                &runtime,
                "ListPermissions",
                &json!({
                    "Principal": { "DataLakePrincipalIdentifier": ANALYST },
                    "Resource": table,
                }),
            );
            assert_eq!(
                status, 200,
                "the filtered listing must answer 200, got {status} {body}"
            );
            let held: Vec<(String, Vec<String>)> = body["PrincipalResourcePermissions"]
                .as_array()
                .expect("PrincipalResourcePermissions must be a list")
                .iter()
                .filter_map(|entry| {
                    let kind = entry["Resource"].as_object()?.keys().next()?.clone();
                    let permissions = entry["Permissions"]
                        .as_array()?
                        .iter()
                        .map(|value| value.as_str().unwrap_or_default().to_string())
                        .collect();
                    Some((kind, permissions))
                })
                .collect();
            // The table's own entry carries what the grant wrote (`DESCRIBE`) beside the
            // default its database hands the tables created in it (`ALL`), which this IAM
            // role holds through the IAM-allowed group the default names.
            let mut on_table: Vec<String> = held
                .iter()
                .filter(|(kind, _)| kind == "Table")
                .flat_map(|(_, permissions)| permissions.clone())
                .collect();
            on_table.sort();
            assert_eq!(
                on_table,
                vec!["ALL".to_string(), "DESCRIBE".to_string()],
                "the table permission must be held on the table itself, got {body}"
            );
            assert!(
                held.iter().any(|(kind, permissions)| kind
                    == "TableWithColumns"
                    && permissions == &["SELECT".to_string()]),
                "the column-scoped permission must be held on the table's columns, got {body}"
            );
        })
        .expect("spawn the catalog-resolution thread")
        .join()
        .expect("every Lake Formation permission resource must be resolved against the catalog");
}

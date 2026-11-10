// SPDX-License-Identifier: CC0-1.0

//! The Data Catalog harness every Lake Formation permission suite shares.
//!
//! Lake Formation grants permissions ON Data Catalog resources, which GLUE owns, so a
//! suite that grants on a database or table has to mount Glue beside Lake Formation (the
//! pair the production router builds) and create those entities through Glue's own wire
//! API (`awsJson1_1` `POST /` with `X-Amz-Target`) first — a grant naming an entity the
//! catalog does not hold is refused with HTTP 400.
//!
//! Declared once in `tests/all.rs`, so the three suites share one copy rather than each
//! carrying its own.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// An emulator with Lake Formation and Glue over ONE shared engine, reached exactly as
/// the production router reaches it: `MultiServiceRouter::dispatch` of a buffered HTTP
/// request.
pub fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_lakeformation_awslakeformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        engine,
    ));
    router
}

/// `POST /` against Glue, with `X-Amz-Target: AWSGlue.<operation>`.
async fn glue(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://glue.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSGlue.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/glue/aws4_request, \
             SignedHeaders=host, Signature=0000",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Create a Glue database, asserting Glue accepted it.
pub async fn create_database(router: &MultiServiceRouter, name: &str) {
    let (status, body) = glue(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": name } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case create database {name:?}: expected 200, got {status} {body:?}"
    );
}

/// An emulator whose Data Catalog already holds `databases`: one entry per database,
/// naming the tables to create in it.
///
/// The seeded-catalog constructor every permission suite shares, so a suite names the
/// entities it grants on instead of carrying its own copy of this loop.
pub async fn emulator_with_catalog(databases: &[(&str, &[&str])]) -> MultiServiceRouter {
    let router = emulator();
    for (database, tables) in databases {
        create_database(&router, database).await;
        for table in *tables {
            create_table(&router, database, table).await;
        }
    }
    router
}

/// `POST /<operation>` against Lake Formation (`restJson1`), answering `(status, body)`.
///
/// The one wire path the permission suites dispatch Lake Formation calls through.
pub async fn lake_formation(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://lakeformation.us-east-1.amazonaws.com/{operation}"
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/lakeformation/aws4_request, \
             SignedHeaders=host, Signature=0000",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// The wire error code (`__type`) of an error envelope, or `""` for a success body.
pub fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The account id every emulated service presents, and therefore the Data Catalog a
/// request without `CatalogId` addresses ("The identifier for the Data Catalog. By
/// default, the account ID." — `CatalogIdString`'s documentation).
pub fn account_catalog() -> String {
    engine::emulator_account_id().to_string()
}

/// Create a Glue table in `database`, asserting Glue accepted it.
pub async fn create_table(router: &MultiServiceRouter, database: &str, name: &str) {
    let (status, body) = glue(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": { "Name": name } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case create table {database}.{name}: expected 200, got {status} {body:?}"
    );
}

/// Create a Glue database from a whole `DatabaseInput`, asserting Glue accepted it.
///
/// For a suite that has to vary what the create sends — the Lake Formation defaults a
/// database hands to its TABLES, say — where [`create_database`] sends only the name.
pub async fn create_database_with_input(router: &MultiServiceRouter, input: &Value) {
    let (status, body) = glue(router, "CreateDatabase", &json!({ "DatabaseInput": input })).await;
    assert_eq!(
        status, 200,
        "case create database {input:?}: expected 200, got {status} {body:?}"
    );
}

/// The principal the Data Catalog recorded as a table's creator (`Table$CreatedBy`, "the
/// person or entity who created the table"), read back through Glue's own `GetTable`.
///
/// A suite about what the CREATOR of a table holds asks the catalog who that is rather
/// than spelling an identity of its own, so the assertion follows the emulator's identity
/// contract instead of pinning a literal ARN.
pub async fn table_created_by(router: &MultiServiceRouter, database: &str, name: &str) -> String {
    let (status, body) = glue(
        router,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case get table {database}.{name}: expected 200, got {status} {body:?}"
    );
    body.get("Table")
        .and_then(|table| table.get("CreatedBy"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case get table {database}.{name}: expected a CreatedBy, got {body:?}")
        })
        .to_string()
}

/// The `PrincipalResourcePermissions` entries a `ListPermissions` body answers, asserting
/// 200 and that the member is present (a listing that matches nothing answers an EMPTY
/// list, not an absent member).
pub async fn list_permissions(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
) -> Vec<Value> {
    let (status, payload) = lake_formation(router, "ListPermissions", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListPermissions to answer 200, got {status} {payload:?}"
    );
    payload
        .get("PrincipalResourcePermissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: ListPermissions must carry a PrincipalResourcePermissions \
                 list, got {payload:?}"
            )
        })
}

/// One listed entry as the permission suites compare it: the principal, the resource, the
/// permissions and the permissions held with the grant option.
pub type Entry = (String, String, Vec<String>, Vec<String>);

/// The string list `member` of one entry, sorted, empty when the member carries none.
fn strings(entry: &Value, member: &str) -> Vec<String> {
    let mut values: Vec<String> = entry
        .get(member)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    values.sort();
    values
}

/// The entries a listing carries, sorted so a comparison does not depend on the order the
/// service enumerated them in.
pub fn permission_entries(listed: &[Value]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = listed
        .iter()
        .map(|entry| {
            (
                entry
                    .get("Principal")
                    .and_then(|principal| principal.get("DataLakePrincipalIdentifier"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                entry
                    .get("Resource")
                    .cloned()
                    .unwrap_or(Value::Null)
                    .to_string(),
                strings(entry, "Permissions"),
                strings(entry, "PermissionsWithGrantOption"),
            )
        })
        .collect();
    entries.sort();
    entries
}

/// Revoke what creating a table in a database that hands its tables the IAM-access-control
/// default granted on it: the creator's Super (`ALL` and every individual table
/// permission, at both of the scopes a table's permissions are held at) and the
/// `IAM_ALLOWED_PRINCIPALS` `ALL` the database handed it
/// (the `creator_table_grants.rs` and `table_default_grants.rs` suites cover those grants
/// themselves).
///
/// A suite about the EXPLICIT grant store calls this after seeding a table, exactly as it
/// calls [`revoke_default_database_grant`] for a database, so its listings read back only
/// the records its own grants wrote.
pub async fn revoke_table_default_grants(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
) {
    let on_table = json!({ "Table": { "DatabaseName": database, "Name": table } });
    let creator = table_created_by(router, database, table).await;
    // What the creator holds, named once for the whole crate by the suite that asserts it;
    // naming it in both lists takes the grant option with it.
    let creator_permissions = json!(crate::creator_table_grants::CREATOR_PERMISSIONS);
    let revokes = [
        json!({
            "Principal": { "DataLakePrincipalIdentifier": creator },
            "Resource": on_table,
            "Permissions": creator_permissions,
            "PermissionsWithGrantOption": creator_permissions,
        }),
        json!({
            "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
            "Resource": on_table,
            "Permissions": ["ALL"],
        }),
    ];
    for body in revokes {
        let (status, payload) = lake_formation(router, "RevokePermissions", &body).await;
        assert_eq!(
            status, 200,
            "case revoke {body:?} on {database}.{table}: expected 200, got {status} {payload:?}"
        );
    }
}

/// Revoke a database's creation-time default grant: `ALL` to `IAM_ALLOWED_PRINCIPALS`, the
/// account's "use only IAM access control" setting, which every database is created
/// carrying.
///
/// A suite about the EXPLICIT grant store calls this first, so its listings read back only
/// the records its own grants wrote. The default itself is covered by
/// `default_database_grant.rs` and `revoke_default_grant.rs`.
pub async fn revoke_default_database_grant(router: &MultiServiceRouter, database: &str) {
    let (status, body) = lake_formation(
        router,
        "RevokePermissions",
        &json!({
            "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
            "Resource": { "Database": { "Name": database } },
            "Permissions": ["ALL"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case revoke the default grant of {database:?}: expected 200, got {status} {body:?}"
    );
}

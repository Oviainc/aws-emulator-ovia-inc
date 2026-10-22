// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the grant a Data Catalog resource is CREATED carrying: the
//! account's "use only IAM access control" default, `ALL` to `IAM_ALLOWED_PRINCIPALS`.
//!
//! Nothing granted it, so it is not a record in the permission store — and a
//! `RevokePermissions` that names it has to take it away all the same, exactly once. The
//! suite in `permissions.rs` covers the explicit grant store; this one covers that
//! default, over the wire through the router that claims `lakeformation`, with its own
//! dispatch harness the way every suite under `tests/all/` carries one.
//!
//! The tables span what the rule generalises over: the kind of resource (only a DATABASE
//! is created carrying it — a table's default comes from its database's
//! `CreateTableDefaultPermissions` in the Data Catalog, which this service's store does
//! not hold), the principal, the permissions named, and whether the request is
//! condition-scoped.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lakeformation_awslakeformation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The group the default grant names, spelled as Lake Formation spells it on the wire.
const IAM_ALLOWED_PRINCIPALS: &str = "IAM_ALLOWED_PRINCIPALS";

/// A role ARN standing for an ordinary grantee — any principal other than the group the
/// default grant names.
const ANALYST: &str = "arn:aws:iam::123456789012:role/analyst";

/// One row of the "what the default covers" table: the case name, the resource, the
/// principal, the permissions and grant-option permissions the revoke names, the
/// condition it carries, and the status it must answer.
type DefaultGrantCase = (
    &'static str,
    Value,
    &'static str,
    Vec<&'static str>,
    Vec<&'static str>,
    Option<&'static str>,
    u16,
);

/// One emulator over one shared engine, reached exactly as the production router reaches
/// it: `MultiServiceRouter::dispatch` of a buffered HTTP request.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// `POST /<operation>` with `body` as the JSON payload, answering `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
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
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A `RevokePermissions` body.
fn revoke_body(
    principal: &str,
    resource: &Value,
    permissions: &[&str],
    permissions_with_grant_option: &[&str],
    condition: Option<&str>,
) -> Value {
    let mut body = json!({
        "Principal": { "DataLakePrincipalIdentifier": principal },
        "Resource": resource,
        "Permissions": permissions,
    });
    if !permissions_with_grant_option.is_empty() {
        body["PermissionsWithGrantOption"] = json!(permissions_with_grant_option);
    }
    if let Some(expression) = condition {
        body["Condition"] = json!({ "Expression": expression });
    }
    body
}

/// The `PrincipalResourcePermissions` entries `ListPermissions` answers for `resource`.
async fn list_permissions(router: &MultiServiceRouter, resource: &Value) -> Vec<Value> {
    let (status, payload) = call(router, "ListPermissions", &json!({ "Resource": resource })).await;
    assert_eq!(
        status, 200,
        "ListPermissions must answer 200, got {status} {payload:?}"
    );
    payload
        .get("PrincipalResourcePermissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn the_default_database_grant_is_revocable_exactly_once() {
    let router = emulator();
    let resource = json!({ "Database": { "Name": "sales" } });
    let body = revoke_body(IAM_ALLOWED_PRINCIPALS, &resource, &["ALL"], &[], None);

    let (status, payload) = call(&router, "RevokePermissions", &body).await;
    assert_eq!(
        status, 200,
        "case \"revoke the default once\": expected 200, got {status} {payload:?}"
    );

    // Nothing explicit was ever granted, so the store holds no entry either before or
    // after: what changed is that the default is gone, which the second revoke shows.
    let entries = list_permissions(&router, &resource).await;
    assert!(
        entries.is_empty(),
        "case \"revoke the default once\": expected no explicit grant entry, got {entries:?}"
    );

    let (status, payload) = call(&router, "RevokePermissions", &body).await;
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
async fn the_default_grant_covers_only_a_database_and_only_all_to_iam_allowed_principals() {
    // Rows differ along every part of the default's identity: the kind of resource that is
    // created carrying it, the principal that holds it, the permissions it carries, and
    // the condition it does not carry. Only the first row names the default itself.
    let cases: Vec<DefaultGrantCase> = vec![
        (
            "a database, the principal and the permission of the default",
            json!({ "Database": { "Name": "sales" } }),
            IAM_ALLOWED_PRINCIPALS,
            vec!["ALL"],
            vec![],
            None,
            200,
        ),
        (
            "a table, whose default comes from its database in the Data Catalog",
            json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } }),
            IAM_ALLOWED_PRINCIPALS,
            vec!["ALL"],
            vec![],
            None,
            400,
        ),
        (
            "every table in a database",
            json!({ "Table": { "DatabaseName": "sales", "TableWildcard": {} } }),
            IAM_ALLOWED_PRINCIPALS,
            vec!["ALL"],
            vec![],
            None,
            400,
        ),
        (
            "the catalog",
            json!({ "Catalog": {} }),
            IAM_ALLOWED_PRINCIPALS,
            vec!["ALL"],
            vec![],
            None,
            400,
        ),
        (
            "a data location",
            json!({ "DataLocation": { "ResourceArn": "arn:aws:s3:::lake-bucket/warehouse" } }),
            IAM_ALLOWED_PRINCIPALS,
            vec!["ALL"],
            vec![],
            None,
            400,
        ),
        (
            "an LF-tag",
            json!({ "LFTag": { "TagKey": "environment", "TagValues": ["dev"] } }),
            IAM_ALLOWED_PRINCIPALS,
            vec!["ALL"],
            vec![],
            None,
            400,
        ),
        (
            "named columns of a table",
            json!({
                "TableWithColumns": {
                    "DatabaseName": "sales",
                    "Name": "orders",
                    "ColumnNames": ["id"],
                }
            }),
            IAM_ALLOWED_PRINCIPALS,
            vec!["ALL"],
            vec![],
            None,
            400,
        ),
        (
            "a permission narrower than the default",
            json!({ "Database": { "Name": "sales" } }),
            IAM_ALLOWED_PRINCIPALS,
            vec!["SELECT"],
            vec![],
            None,
            400,
        ),
        (
            "the default plus a permission it does not carry",
            json!({ "Database": { "Name": "sales" } }),
            IAM_ALLOWED_PRINCIPALS,
            vec!["ALL", "DROP"],
            vec![],
            None,
            400,
        ),
        (
            "the grant option, which the default does not carry",
            json!({ "Database": { "Name": "sales" } }),
            IAM_ALLOWED_PRINCIPALS,
            vec![],
            vec!["ALL"],
            None,
            400,
        ),
        (
            "another principal",
            json!({ "Database": { "Name": "sales" } }),
            ANALYST,
            vec!["ALL"],
            vec![],
            None,
            400,
        ),
        (
            "a condition, which the default does not carry",
            json!({ "Database": { "Name": "sales" } }),
            IAM_ALLOWED_PRINCIPALS,
            vec!["ALL"],
            vec![],
            Some("tag.environment = 'prod'"),
            400,
        ),
    ];

    for (label, resource, principal, permissions, with_grant_option, condition, expected) in &cases
    {
        let router = emulator();
        let body = revoke_body(
            principal,
            resource,
            permissions,
            with_grant_option,
            *condition,
        );
        let (status, payload) = call(&router, "RevokePermissions", &body).await;
        assert_eq!(
            status, *expected,
            "case {label:?}: expected {expected}, got {status} {payload:?}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&payload),
                "InvalidInputException",
                "case {label:?}: expected InvalidInputException, got {payload:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_default_grant_and_an_explicit_grant_are_revoked_independently() {
    // The default is held ON TOP of what `GrantPermissions` wrote for the same principal
    // and resource, so each is revocable on its own and neither revoke touches the other.
    let router = emulator();
    let resource = json!({ "Database": { "Name": "sales" } });
    let grant = json!({
        "Principal": { "DataLakePrincipalIdentifier": IAM_ALLOWED_PRINCIPALS },
        "Resource": resource,
        "Permissions": ["SELECT"],
    });
    let (status, payload) = call(&router, "GrantPermissions", &grant).await;
    assert_eq!(
        status, 200,
        "case \"grant SELECT explicitly\": expected 200, got {status} {payload:?}"
    );

    // The default is still there to revoke, and revoking it leaves the explicit grant.
    let body = revoke_body(IAM_ALLOWED_PRINCIPALS, &resource, &["ALL"], &[], None);
    let (status, payload) = call(&router, "RevokePermissions", &body).await;
    assert_eq!(
        status, 200,
        "case \"revoke the default, keep the explicit grant\": expected 200, got \
         {status} {payload:?}"
    );
    let entries = list_permissions(&router, &resource).await;
    assert_eq!(
        entries.len(),
        1,
        "case \"revoke the default, keep the explicit grant\": expected the explicit \
         grant to survive, got {entries:?}"
    );
    assert_eq!(
        entries[0].get("Permissions"),
        Some(&json!(["SELECT"])),
        "case \"revoke the default, keep the explicit grant\": expected SELECT, got \
         {entries:?}"
    );

    // And the explicit grant is revoked as usual, which empties the store.
    let body = revoke_body(IAM_ALLOWED_PRINCIPALS, &resource, &["SELECT"], &[], None);
    let (status, payload) = call(&router, "RevokePermissions", &body).await;
    assert_eq!(
        status, 200,
        "case \"revoke the explicit grant\": expected 200, got {status} {payload:?}"
    );
    let entries = list_permissions(&router, &resource).await;
    assert!(
        entries.is_empty(),
        "case \"revoke the explicit grant\": expected no entry left, got {entries:?}"
    );

    // Neither is revocable twice: the default is remembered as gone, the record is gone.
    for (label, permissions) in [
        ("the default again", vec!["ALL"]),
        ("the explicit grant again", vec!["SELECT"]),
    ] {
        let body = revoke_body(IAM_ALLOWED_PRINCIPALS, &resource, &permissions, &[], None);
        let (status, payload) = call(&router, "RevokePermissions", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "InvalidInputException",
            "case {label:?}: expected InvalidInputException, got {payload:?}"
        );
    }
}

#[tokio::test]
async fn the_default_grant_is_per_database() {
    // Revoking one database's default says nothing about another's: the memory of the
    // revoke is keyed by the resource, so every other database still carries its own.
    let router = emulator();
    let revoked = json!({ "Database": { "Name": "sales" } });
    let untouched: Vec<(&str, Value)> = vec![
        ("another database", json!({ "Database": { "Name": "finance" } })),
        (
            "a database in another Data Catalog",
            json!({ "Database": { "CatalogId": "999988887777", "Name": "sales" } }),
        ),
    ];

    let body = revoke_body(IAM_ALLOWED_PRINCIPALS, &revoked, &["ALL"], &[], None);
    let (status, payload) = call(&router, "RevokePermissions", &body).await;
    assert_eq!(
        status, 200,
        "case \"revoke one database's default\": expected 200, got {status} {payload:?}"
    );

    for (label, resource) in &untouched {
        let body = revoke_body(IAM_ALLOWED_PRINCIPALS, resource, &["ALL"], &[], None);
        let (status, payload) = call(&router, "RevokePermissions", &body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: must still carry its own default, got {status} {payload:?}"
        );
    }
}

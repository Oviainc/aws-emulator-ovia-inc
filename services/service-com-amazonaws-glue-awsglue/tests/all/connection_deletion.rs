// SPDX-License-Identifier: CC0-1.0

//! `DeleteConnection`: what the Data Catalog holds after a connection is deleted.
//!
//! RULE: a delete of one named connection definition "[d]eletes a connection from the Data
//! Catalog" — it removes the WHOLE stored record of the name the request addresses in the
//! catalog it addresses, so every later read of that name reports it absent, the listing of
//! that catalog no longer carries it, the tag set it held is gone with it and the name is
//! free for another create; every other connection is untouched, and a name the addressed
//! catalog holds none under earns the modelled `EntityNotFoundException` while changing
//! nothing.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: the connection type and the
//! property maps of the deleted definition, whether it carried tags, whether the request
//! names the `CatalogId` or leaves it defaulted, how many connections the catalog holds
//! besides it, and the form of the name the delete addresses — including both published
//! bounds of `NameString` (len 1..255).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// The ARN a connection of this account and Region is tagged under, which is how a tag
/// read addresses it (`arn:aws:glue:<region>:<account>:connection/<name>`).
fn connection_arn(name: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:connection/{name}")
}

/// A name of exactly the longest a `NameString` may be: the published upper bound of
/// `DeleteConnectionRequest$ConnectionName` (len 1..255, from
/// `smithy_query operation=DeleteConnection`).
fn longest_name() -> String {
    "l".repeat(255)
}

/// One deletion row: the definition created, the `CatalogId` all of the row's calls name,
/// and the tags the create recorded for it.
struct DeleteRow {
    case: &'static str,
    name: String,
    /// The `CatalogId` every call of the row names, or `None` to leave it defaulted.
    catalog_id: Option<&'static str>,
    input: Value,
    tags: Option<Value>,
    /// The other connections the catalog holds while the row's delete runs, which the
    /// delete must leave exactly as they were.
    bystanders: Vec<String>,
}

/// A plain JDBC definition under `name` — enough for a row that cares only about which
/// records survive the delete.
fn jdbc_input(name: &str) -> Value {
    json!({
        "Name": name,
        "ConnectionType": "JDBC",
        "ConnectionProperties": {
            "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
            "USERNAME": "etl",
        },
    })
}

fn delete_rows() -> Vec<DeleteRow> {
    vec![
        DeleteRow {
            case: "the only connection the catalog holds, addressed by the default catalog",
            name: "warehouse".to_string(),
            catalog_id: None,
            input: jdbc_input("warehouse"),
            tags: None,
            bystanders: Vec::new(),
        },
        DeleteRow {
            case: "one of several, with the catalog named explicitly",
            name: "events".to_string(),
            catalog_id: Some(ACCOUNT_ID),
            input: json!({
                "Name": "events",
                "ConnectionType": "KAFKA",
                "ConnectionProperties": { "KAFKA_BOOTSTRAP_SERVERS": "broker.internal:9094" },
                "MatchCriteria": ["streaming", "events"],
                "Description": "the event bus",
            }),
            tags: None,
            bystanders: vec![
                "archive".to_string(),
                "warehouse".to_string(),
                "zookeeper".to_string(),
            ],
        },
        DeleteRow {
            case: "a tagged connection carrying per-compute-environment property maps",
            name: "salesforce".to_string(),
            catalog_id: None,
            input: json!({
                "Name": "salesforce",
                "ConnectionType": "SALESFORCE",
                "ConnectionProperties": { "INSTANCE_URL": "https://example.my.salesforce.com" },
                "SparkProperties": { "INSTANCE_URL": "https://spark.my.salesforce.com" },
                "PythonProperties": { "API_VERSION": "60.0" },
                "AuthenticationConfiguration": {
                    "AuthenticationType": "BASIC",
                    "SecretArn": "arn:aws:secretsmanager:us-east-1:000000000000:secret:sf",
                },
            }),
            tags: Some(json!({ "team": "data-platform", "env": "test" })),
            bystanders: vec!["warehouse".to_string()],
        },
        DeleteRow {
            case: "a name of the shortest a NameString may be",
            name: "a".to_string(),
            catalog_id: None,
            input: jdbc_input("a"),
            tags: Some(json!({ "only": "tag" })),
            bystanders: vec!["b".to_string()],
        },
        DeleteRow {
            case: "a name of the longest a NameString may be",
            name: longest_name(),
            catalog_id: Some(ACCOUNT_ID),
            input: jdbc_input(&longest_name()),
            tags: None,
            bystanders: vec!["warehouse".to_string()],
        },
        DeleteRow {
            case: "a name carrying a space and mixed case, stored exactly as sent",
            name: "Reporting Replica".to_string(),
            catalog_id: None,
            input: json!({
                "Name": "Reporting Replica",
                "ConnectionType": "MONGODB",
                "ConnectionProperties": { "CONNECTION_URL": "mongodb://db.internal:27017" },
            }),
            tags: Some(json!({ "tier": "replica" })),
            bystanders: vec!["reporting replica".to_string()],
        },
    ]
}

/// Add the row's `CatalogId` to a request body, when it names one.
fn addressed(mut request: Value, catalog_id: Option<&str>) -> Value {
    if let Some(catalog) = catalog_id {
        request["CatalogId"] = json!(catalog);
    }
    request
}

/// The `Name` of every connection the addressed catalog lists, in the order it listed them.
async fn listed_names(
    router: &MultiServiceRouter,
    case: &str,
    catalog_id: Option<&str>,
) -> Vec<String> {
    let page = ok(router, "GetConnections", &addressed(json!({}), catalog_id)).await;
    page["ConnectionList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: GetConnections must report a list, got {page}"))
        .iter()
        .map(|connection| {
            connection["Name"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every listed connection must carry a Name, got {page}")
                })
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_delete_removes_the_addressed_connection_and_leaves_every_other_one() {
    for row in delete_rows() {
        // A router per row, so the listing assertions are over exactly the records the
        // row created.
        let router = emulator();
        let mut create = json!({ "ConnectionInput": row.input.clone() });
        if let Some(tags) = &row.tags {
            create["Tags"] = tags.clone();
        }
        ok(&router, "CreateConnection", &addressed(create, row.catalog_id)).await;
        for bystander in &row.bystanders {
            ok(
                &router,
                "CreateConnection",
                &addressed(
                    json!({
                        "ConnectionInput": jdbc_input(bystander),
                        "Tags": { "kept": bystander },
                    }),
                    row.catalog_id,
                ),
            )
            .await;
        }
        let before = ok(
            &router,
            "GetConnection",
            &addressed(json!({ "Name": row.name }), row.catalog_id),
        )
        .await;
        let created_at = before
            .get("Connection")
            .and_then(|connection| connection.get("CreationTime"))
            .and_then(Value::as_f64);
        assert!(
            created_at.is_some(),
            "case {:?}: the connection must be readable before the delete, got {before}",
            row.case
        );

        let (status, body) = call(
            &router,
            "DeleteConnection",
            &addressed(json!({ "ConnectionName": row.name }), row.catalog_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DeleteConnection must answer 200, got {status} with body {body}",
            row.case
        );
        // `DeleteConnectionResponse` models no member, so a served delete carries none.
        assert!(
            body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty),
            "case {:?}: a served delete answers no member, got {body}",
            row.case
        );

        // The record is gone: the read of that name reports the miss, and the refusal
        // names the connection the request addressed.
        let (status, body) = call(
            &router,
            "GetConnection",
            &addressed(json!({ "Name": row.name }), row.catalog_id),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: the deleted connection must no longer be readable, got {status} with \
             body {body}",
            row.case
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {:?}: expected EntityNotFoundException, got {}",
            row.case,
            error_code(&body)
        );
        // The read's refusal names the connection the request addressed and the account
        // whose catalog was searched, which is the `CatalogId` the request resolved to.
        let searched = row.catalog_id.unwrap_or(ACCOUNT_ID);
        assert_eq!(
            error_message(&body),
            format!(
                "Connection with name {} in account {searched} is not found",
                row.name
            ),
            "case {:?}: unexpected wording for the read's miss",
            row.case
        );

        // The listing of that catalog carries every other connection and not this one.
        let listed = listed_names(&router, row.case, row.catalog_id).await;
        assert!(
            !listed.contains(&row.name),
            "case {:?}: the deleted connection must not be listed, got {listed:?}",
            row.case
        );
        for bystander in &row.bystanders {
            assert!(
                listed.contains(bystander),
                "case {:?}: the delete must leave {bystander:?} alone, got {listed:?}",
                row.case
            );
            let tags = ok(
                &router,
                "GetTags",
                &json!({ "ResourceArn": connection_arn(bystander) }),
            )
            .await;
            assert_eq!(
                tags.get("Tags"),
                Some(&json!({ "kept": bystander })),
                "case {:?}: the delete must leave {bystander:?}'s tags alone, got {tags}",
                row.case
            );
        }

        // The tag set went with the record: the resource the ARN names is no longer there
        // to report tags for.
        let (status, body) = call(
            &router,
            "GetTags",
            &json!({ "ResourceArn": connection_arn(&row.name) }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: the deleted connection holds no tags to read, got {status} with body \
             {body}",
            row.case
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {:?}: expected EntityNotFoundException from the tag read, got {}",
            row.case,
            error_code(&body)
        );

        // The name is free: a create under it is served, and the new record carries none
        // of the deleted one's tags.
        ok(
            &router,
            "CreateConnection",
            &addressed(
                json!({ "ConnectionInput": jdbc_input(&row.name) }),
                row.catalog_id,
            ),
        )
        .await;
        let again = ok(
            &router,
            "GetConnection",
            &addressed(json!({ "Name": row.name }), row.catalog_id),
        )
        .await;
        let connection = again
            .get("Connection")
            .unwrap_or_else(|| panic!("case {:?}: expected a Connection, got {again}", row.case));
        assert_eq!(
            connection.get("ConnectionType").and_then(Value::as_str),
            Some("JDBC"),
            "case {:?}: the re-created connection is the one this create sent, got {connection}",
            row.case
        );
        let tags = ok(
            &router,
            "GetTags",
            &json!({ "ResourceArn": connection_arn(&row.name) }),
        )
        .await;
        assert_eq!(
            tags.get("Tags"),
            Some(&json!({})),
            "case {:?}: the deleted connection's tags must not survive into the name's next \
             create, got {tags}",
            row.case
        );
    }
}

/// One refused delete: the request, and the error code it must earn.
struct RefusalRow {
    case: &'static str,
    request: Value,
    code: &'static str,
}

fn refusal_rows() -> Vec<RefusalRow> {
    vec![
        RefusalRow {
            case: "a name this catalog holds no connection under",
            request: json!({ "ConnectionName": "absent" }),
            code: "EntityNotFoundException",
        },
        RefusalRow {
            case: "a name differing from a stored one only in case",
            request: json!({ "ConnectionName": "STANDING" }),
            code: "EntityNotFoundException",
        },
        RefusalRow {
            case: "the longest name a NameString may be, which nothing created",
            request: json!({ "ConnectionName": longest_name() }),
            code: "EntityNotFoundException",
        },
        RefusalRow {
            case: "a CatalogId of another account, which no resource policy grants",
            request: json!({ "CatalogId": "210987654321", "ConnectionName": "standing" }),
            code: "AccessDeniedException",
        },
    ]
}

#[tokio::test]
async fn a_refused_delete_removes_nothing() {
    let router = emulator();
    ok(
        &router,
        "CreateConnection",
        &json!({ "ConnectionInput": jdbc_input("standing") }),
    )
    .await;
    let before = ok(&router, "GetConnection", &json!({ "Name": "standing" })).await;

    for row in refusal_rows() {
        let (status, body) = call(&router, "DeleteConnection", &row.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: the delete must be refused with 400, got {status} with body {body}",
            row.case
        );
        assert!(
            error_code(&body).ends_with(row.code),
            "case {:?}: expected {}, got {}",
            row.case,
            row.code,
            error_code(&body)
        );
        // A miss states the resource kind the delete could not resolve and nothing else —
        // a different sentence from the read's, which names the connection and the account,
        // and from the update's, which names the connection.
        if row.code == "EntityNotFoundException" {
            assert_eq!(
                error_message(&body),
                "Connection not found",
                "case {:?}: unexpected wording for the delete's miss",
                row.case
            );
        }
        assert_eq!(
            ok(&router, "GetConnection", &json!({ "Name": "standing" })).await,
            before,
            "case {:?}: a refused delete must leave the stored definition alone",
            row.case
        );
    }
}

#[tokio::test]
async fn deleting_the_same_connection_twice_reports_the_second_as_a_miss() {
    let router = emulator();
    ok(
        &router,
        "CreateConnection",
        &json!({ "ConnectionInput": jdbc_input("transient") }),
    )
    .await;

    // The first delete is served; a second one addresses a name the catalog holds nothing
    // under, which is the same miss any other absent name earns — this operation models no
    // idempotent delete.
    ok(
        &router,
        "DeleteConnection",
        &json!({ "ConnectionName": "transient" }),
    )
    .await;
    let (status, body) = call(
        &router,
        "DeleteConnection",
        &json!({ "ConnectionName": "transient" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a second delete of one connection must be refused, got {status} with body {body}"
    );
    assert!(
        error_code(&body).ends_with("EntityNotFoundException"),
        "expected EntityNotFoundException, got {}",
        error_code(&body)
    );
    assert_eq!(
        error_message(&body),
        "Connection not found",
        "the second delete's refusal is the delete's own fixed sentence"
    );
}

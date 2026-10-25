// SPDX-License-Identifier: CC0-1.0

//! `UpdateConnection`: the connection definition a read reports after an update.
//!
//! The rule under test: a `ConnectionInput` is "a structure that is used to specify a
//! connection to create or update", i.e. the WHOLE client-settable definition, so an
//! update writes every member it carries over the stored definition and CLEARS every
//! member it omits — a later read reports exactly what the update sent, with
//! `ConnectionSchemaVersion` recomputed from the per-compute-environment maps it carries.
//! The server-set members keep their own rules: the create's `CreationTime` never moves,
//! `LastUpdatedTime` becomes the instant of the update, and the tags the create recorded
//! are not touched by it.
//!
//! The rows span what the rule generalises over: a property value rewritten, a property
//! dropped, optional members cleared, the connection type changed, per-environment maps
//! added (so the schema version rises) and dropped (so it falls), the authentication
//! configuration cleared, and the catalog named explicitly rather than defaulted.

use serde_json::{json, Value};

use crate::wire::{assert_echoed, call, emulator, error_code, ok, ACCOUNT_ID};

/// The members a `ConnectionInput` sets that a `Connection` reports back, so a row can
/// assert both directions of the rule: a member the update sent round-trips, a member it
/// omitted is gone.
///
/// Source: the members shared by `com.amazonaws.glue#ConnectionInput` and
/// `com.amazonaws.glue#Connection` (`smithy_query shape=ConnectionInput,Connection`),
/// minus `Name`, which addresses the connection rather than being rewritten by it.
const CLIENT_SETTABLE_MEMBERS: [&str; 7] = [
    "Description",
    "ConnectionType",
    "MatchCriteria",
    "ConnectionProperties",
    "SparkProperties",
    "AthenaProperties",
    "PythonProperties",
];

/// One update row: the definition created, the definition the update submits, and the
/// `ConnectionSchemaVersion` the updated definition must report.
struct UpdateRow {
    case: &'static str,
    name: &'static str,
    /// The `CatalogId` all three calls of the row name, or `None` to leave it defaulted.
    catalog_id: Option<&'static str>,
    created: Value,
    updated: Value,
    schema_version: i64,
}

fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            case: "a rewritten property value, with the password dropped",
            name: "warehouse",
            catalog_id: None,
            created: json!({
                "Name": "warehouse",
                "ConnectionType": "JDBC",
                "ConnectionProperties": {
                    "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                    "USERNAME": "etl",
                    "PASSWORD": "a-secret",
                },
            }),
            updated: json!({
                "Name": "warehouse",
                "ConnectionType": "JDBC",
                "ConnectionProperties": {
                    "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/reporting",
                    "USERNAME": "reporting",
                },
            }),
            schema_version: 1,
        },
        UpdateRow {
            case: "the optional members the update omits are cleared",
            name: "events",
            catalog_id: None,
            created: json!({
                "Name": "events",
                "ConnectionType": "KAFKA",
                "ConnectionProperties": { "KAFKA_BOOTSTRAP_SERVERS": "broker.internal:9094" },
                "MatchCriteria": ["streaming", "events"],
                "Description": "the event bus",
            }),
            updated: json!({
                "Name": "events",
                "ConnectionType": "KAFKA",
                "ConnectionProperties": { "KAFKA_BOOTSTRAP_SERVERS": "broker.internal:9094" },
            }),
            schema_version: 1,
        },
        UpdateRow {
            case: "per-environment property maps the update adds raise the schema version",
            name: "salesforce",
            catalog_id: None,
            created: json!({
                "Name": "salesforce",
                "ConnectionType": "SALESFORCE",
                "ConnectionProperties": { "INSTANCE_URL": "https://example.my.salesforce.com" },
            }),
            updated: json!({
                "Name": "salesforce",
                "ConnectionType": "SALESFORCE",
                "ConnectionProperties": { "INSTANCE_URL": "https://example.my.salesforce.com" },
                "SparkProperties": { "INSTANCE_URL": "https://spark.my.salesforce.com" },
                "PythonProperties": { "API_VERSION": "60.0" },
            }),
            schema_version: 2,
        },
        UpdateRow {
            case: "per-environment property maps the update omits drop the schema version",
            name: "athena-backed",
            catalog_id: None,
            created: json!({
                "Name": "athena-backed",
                "ConnectionType": "JDBC",
                "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:awsathena://here" },
                "AthenaProperties": { "workgroup": "primary" },
                "AuthenticationConfiguration": {
                    "AuthenticationType": "BASIC",
                    "SecretArn": "arn:aws:secretsmanager:us-east-1:000000000000:secret:db",
                },
            }),
            updated: json!({
                "Name": "athena-backed",
                "ConnectionType": "JDBC",
                "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:awsathena://there" },
            }),
            schema_version: 1,
        },
        UpdateRow {
            case: "the connection type, requirements and authentication are all rewritten",
            name: "mongo",
            catalog_id: Some(ACCOUNT_ID),
            created: json!({
                "Name": "mongo",
                "ConnectionType": "MONGODB",
                "ConnectionProperties": { "CONNECTION_URL": "mongodb://db.internal:27017" },
                "PhysicalConnectionRequirements": {
                    "SubnetId": "subnet-0123456789abcdef0",
                    "SecurityGroupIdList": ["sg-0123456789abcdef0"],
                    "AvailabilityZone": "us-east-1a",
                },
            }),
            updated: json!({
                "Name": "mongo",
                "ConnectionType": "JDBC",
                "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://db.internal:3306/x" },
                "PhysicalConnectionRequirements": {
                    "SubnetId": "subnet-0fedcba9876543210",
                    "SecurityGroupIdList": ["sg-0fedcba9876543210", "sg-00000000000000000"],
                    "AvailabilityZone": "us-east-1b",
                },
                "AuthenticationConfiguration": {
                    "AuthenticationType": "BASIC",
                    "SecretArn": "arn:aws:secretsmanager:us-east-1:000000000000:secret:mongo",
                },
            }),
            schema_version: 1,
        },
    ]
}

/// Read one connection back, with the `CatalogId` the row addresses.
async fn read_connection(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    catalog_id: Option<&str>,
) -> Value {
    let mut request = json!({ "Name": name });
    if let Some(catalog) = catalog_id {
        request["CatalogId"] = json!(catalog);
    }
    let answer = ok(router, "GetConnection", &request).await;
    answer
        .get("Connection")
        .cloned()
        .unwrap_or_else(|| panic!("GetConnection must report a Connection, got {answer}"))
}

#[tokio::test]
async fn an_update_replaces_the_whole_stored_connection_definition() {
    let router = emulator();

    for row in update_rows() {
        let mut create = json!({ "ConnectionInput": row.created });
        let mut update = json!({ "Name": row.name, "ConnectionInput": row.updated.clone() });
        if let Some(catalog) = row.catalog_id {
            create["CatalogId"] = json!(catalog);
            update["CatalogId"] = json!(catalog);
        }

        let (status, body) = call(&router, "CreateConnection", &create).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateConnection must answer 200, got {status} with body {body}",
            row.case
        );
        let before = read_connection(&router, row.name, row.catalog_id).await;
        let created_at = before.get("CreationTime").and_then(Value::as_f64);
        assert!(
            created_at.is_some(),
            "case {:?}: CreationTime must be reported before the update, got {before}",
            row.case
        );

        let (status, body) = call(&router, "UpdateConnection", &update).await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateConnection must answer 200, got {status} with body {body}",
            row.case
        );

        let after = read_connection(&router, row.name, row.catalog_id).await;
        assert_eq!(
            after.get("Name").and_then(Value::as_str),
            Some(row.name),
            "case {:?}: the updated connection keeps the name it was addressed by, got \
             {after}",
            row.case
        );
        // Every client-settable member: what the update sent is reported, and what it
        // omitted is gone — the update redefines the connection, it does not merge into it.
        for member in CLIENT_SETTABLE_MEMBERS {
            let sent = row.updated.get(member);
            let got = after.get(member);
            match sent {
                Some(sent) => assert_eq!(
                    got,
                    Some(sent),
                    "case {:?}: member {member} must report what the update sent; expected \
                     {sent}, got {got:?}",
                    row.case
                ),
                None => assert!(
                    got.is_none_or(Value::is_null),
                    "case {:?}: member {member} was not submitted by the update, so it must \
                     be cleared, got {got:?}",
                    row.case
                ),
            }
        }
        // The two structure members, checked the same way: present ones round-trip (the
        // response shape of an authentication configuration carries no credentials), and
        // an omitted one is cleared.
        for member in ["PhysicalConnectionRequirements", "AuthenticationConfiguration"] {
            match row.updated.get(member) {
                Some(sent) => assert_echoed(
                    row.case,
                    sent,
                    after.get(member).unwrap_or_else(|| {
                        panic!(
                            "case {:?}: member {member} must be reported, got {after}",
                            row.case
                        )
                    }),
                ),
                None => assert!(
                    after.get(member).is_none_or(Value::is_null),
                    "case {:?}: member {member} was not submitted by the update, so it must \
                     be cleared, got {after}",
                    row.case
                ),
            }
        }
        assert_eq!(
            after.get("ConnectionSchemaVersion").and_then(Value::as_i64),
            Some(row.schema_version),
            "case {:?}: expected ConnectionSchemaVersion {}, got {after}",
            row.case,
            row.schema_version
        );
        assert_eq!(
            after.get("CreationTime").and_then(Value::as_f64),
            created_at,
            "case {:?}: the instant of the create must not move; expected {created_at:?}, \
             got {after}",
            row.case
        );
        let last_updated = after.get("LastUpdatedTime").and_then(Value::as_f64);
        assert!(
            last_updated > created_at,
            "case {:?}: LastUpdatedTime must be the instant of the update, later than the \
             create's {created_at:?}, got {last_updated:?}",
            row.case
        );
        assert!(
            after
                .get("LastUpdatedBy")
                .and_then(Value::as_str)
                .is_some_and(|by| !by.is_empty()),
            "case {:?}: the principal that updated the connection must be reported, got \
             {after}",
            row.case
        );
        // Nothing connected to the data source, so the update reports no validation
        // outcome, exactly as the create does not.
        for member in ["Status", "StatusReason", "LastConnectionValidationTime"] {
            assert!(
                after.get(member).is_none_or(Value::is_null),
                "case {:?}: nothing validated this connection, so {member} must be absent, \
                 got {after}",
                row.case
            );
        }
    }
}

/// One refused update: the request, and the error code it must earn.
struct RefusalRow {
    case: &'static str,
    request: Value,
    code: &'static str,
}

fn refusal_rows() -> Vec<RefusalRow> {
    vec![
        RefusalRow {
            case: "a connection this catalog does not hold",
            request: json!({
                "Name": "absent",
                "ConnectionInput": {
                    "Name": "absent",
                    "ConnectionType": "JDBC",
                    "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://nowhere" },
                },
            }),
            code: "EntityNotFoundException",
        },
        RefusalRow {
            case: "a ConnectionInput naming a different connection",
            request: json!({
                "Name": "standing",
                "ConnectionInput": {
                    "Name": "moved",
                    "ConnectionType": "JDBC",
                    "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://moved" },
                },
            }),
            code: "InvalidInputException",
        },
        RefusalRow {
            case: "a CatalogId no Data Catalog in this account answers to",
            request: json!({
                "CatalogId": "210987654321",
                "Name": "standing",
                "ConnectionInput": {
                    "Name": "standing",
                    "ConnectionType": "JDBC",
                    "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://elsewhere" },
                },
            }),
            code: "InvalidInputException",
        },
    ]
}

#[tokio::test]
async fn a_refused_update_changes_nothing() {
    let router = emulator();
    let standing = json!({
        "Name": "standing",
        "ConnectionType": "KAFKA",
        "ConnectionProperties": { "KAFKA_BOOTSTRAP_SERVERS": "broker.internal:9094" },
        "Description": "left alone",
    });
    ok(
        &router,
        "CreateConnection",
        &json!({ "ConnectionInput": standing.clone() }),
    )
    .await;
    let before = read_connection(&router, "standing", None).await;

    for row in refusal_rows() {
        let (status, body) = call(&router, "UpdateConnection", &row.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: the update must be refused with 400, got {status} with body {body}",
            row.case
        );
        assert!(
            error_code(&body).ends_with(row.code),
            "case {:?}: expected {}, got {}",
            row.case,
            row.code,
            error_code(&body)
        );
        assert_eq!(
            read_connection(&router, "standing", None).await,
            before,
            "case {:?}: a refused update must leave the stored definition alone",
            row.case
        );
        // The name a refused rename would have moved the connection to holds nothing.
        let (status, body) = call(&router, "GetConnection", &json!({ "Name": "moved" })).await;
        assert_eq!(
            status, 400,
            "case {:?}: a refused update must not have written another connection, got \
             {status} with body {body}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_update_leaves_the_connections_tags_alone() {
    let router = emulator();
    let input = json!({
        "Name": "tagged",
        "ConnectionType": "JDBC",
        "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://db.internal:3306/a" },
    });
    ok(
        &router,
        "CreateConnection",
        &json!({
            "ConnectionInput": input,
            "Tags": { "team": "data-platform", "env": "test" },
        }),
    )
    .await;

    ok(
        &router,
        "UpdateConnection",
        &json!({
            "Name": "tagged",
            "ConnectionInput": {
                "Name": "tagged",
                "ConnectionType": "JDBC",
                "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://db.internal:3306/b" },
            },
        }),
    )
    .await;

    // `UpdateConnection` carries no `Tags`, so the tags the create recorded are still
    // the resource's tags after it.
    let tags = ok(
        &router,
        "GetTags",
        &json!({ "ResourceArn": format!("arn:aws:glue:us-east-1:{ACCOUNT_ID}:connection/tagged") }),
    )
    .await;
    assert_eq!(
        tags.get("Tags"),
        Some(&json!({ "team": "data-platform", "env": "test" })),
        "an update must leave the connection's tags alone, got {tags}"
    );
}

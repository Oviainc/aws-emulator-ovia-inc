// SPDX-License-Identifier: CC0-1.0

//! The `Endpoint` projection every DMS operation that answers with an endpoint goes
//! through: `CreateEndpoint` and `DescribeEndpoints`.
//!
//! The rule under test: an `Endpoint` reports every member the stored endpoint
//! determines — `EndpointType` in the uppercase form a response carries, the expanded
//! `EngineDisplayName` for its `EngineName`, `IsReadOnly`, the encryption key (the
//! region's default DMS key when the request named none) and the single engine settings
//! structure `EngineName` selects, carrying the connection parameters the request
//! supplied — and reports them identically on every such operation.
//!
//! Each case drives the real wire path through the router, so what is asserted is what
//! a client sees.

use serde_json::{json, Value};

use crate::migration_task_slice::{text, Emulator, ACCOUNT, REGION};

/// A password every request that carries one uses, so a case can assert it appears
/// nowhere in any response.
const SECRET: &str = "never-echoed-secret";

/// One endpoint the projection is asserted over.
struct Case {
    /// What this row varies.
    name: &'static str,
    /// The `EngineName` the request carries.
    engine_name: &'static str,
    /// The `EndpointType` the request carries, in the lowercase request form.
    endpoint_type: &'static str,
    /// The `EngineDisplayName` the response must carry.
    display_name: &'static str,
    /// The one `Endpoint` member that must carry an engine settings structure.
    settings_member: &'static str,
    /// Request members beyond `EndpointIdentifier`/`EndpointType`/`EngineName`.
    extra: Value,
    /// The engine settings structure the response must report, member for member: a
    /// member this does not name must not be invented either.
    expected_settings: Value,
}

/// The engine display names and settings structures the rows below expect come from
/// `Endpoint$EngineDisplayName` ("The expanded name for the engine name. For example,
/// if the EngineName parameter is \"aurora\", this value would be \"Amazon Aurora
/// MySQL\"") and from the per-engine settings members `Endpoint` models, over the
/// `EngineName` values `CreateEndpoint$EngineName` publishes.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "mysql source, connection parameters at top level",
            engine_name: "mysql",
            endpoint_type: "source",
            display_name: "MySQL",
            settings_member: "MySQLSettings",
            extra: json!({
                "ServerName": "db.example.internal",
                "Port": 3306,
                "DatabaseName": "hr",
                "Username": "reader",
                "Password": SECRET,
            }),
            expected_settings: json!({
                "ServerName": "db.example.internal",
                "Port": 3306,
                "DatabaseName": "hr",
                "Username": "reader",
                "AuthenticationMethod": "password",
            }),
        },
        Case {
            name: "mysql source, no connection parameters at all",
            engine_name: "mysql",
            endpoint_type: "source",
            display_name: "MySQL",
            settings_member: "MySQLSettings",
            extra: json!({}),
            expected_settings: json!({ "AuthenticationMethod": "password" }),
        },
        Case {
            name: "mysql target, parameters in the settings member, iam authentication",
            engine_name: "mysql",
            endpoint_type: "target",
            display_name: "MySQL",
            settings_member: "MySQLSettings",
            extra: json!({
                "ServerName": "top-level.example.internal",
                "MySQLSettings": {
                    "ServerName": "settings.example.internal",
                    "Port": 3307,
                    "Username": "writer",
                    "Password": SECRET,
                    // `MySQLAuthenticationMethod` models password | iam.
                    "AuthenticationMethod": "iam",
                    "ServerTimezone": "US/Pacific",
                },
            }),
            expected_settings: json!({
                // What the settings member carries wins over the top-level member.
                "ServerName": "settings.example.internal",
                "Port": 3307,
                "Username": "writer",
                "AuthenticationMethod": "iam",
                "ServerTimezone": "US/Pacific",
            }),
        },
        Case {
            // The example the `EngineDisplayName` doc itself gives, and an engine whose
            // settings structure is MySQL's.
            name: "aurora source",
            engine_name: "aurora",
            endpoint_type: "source",
            display_name: "Amazon Aurora MySQL",
            settings_member: "MySQLSettings",
            extra: json!({ "ServerName": "aurora.example.internal", "Port": 3306 }),
            expected_settings: json!({
                "ServerName": "aurora.example.internal",
                "Port": 3306,
                "AuthenticationMethod": "password",
            }),
        },
        Case {
            name: "mariadb target",
            engine_name: "mariadb",
            endpoint_type: "target",
            display_name: "MariaDB",
            settings_member: "MySQLSettings",
            extra: json!({ "ServerName": "maria.example.internal", "Username": "loader" }),
            expected_settings: json!({
                "ServerName": "maria.example.internal",
                "Username": "loader",
                "AuthenticationMethod": "password",
            }),
        },
        Case {
            name: "postgres target",
            engine_name: "postgres",
            endpoint_type: "target",
            display_name: "PostgreSQL",
            settings_member: "PostgreSQLSettings",
            extra: json!({
                "ServerName": "warehouse.example.internal",
                "Port": 5432,
                "DatabaseName": "analytics",
                "Username": "etl",
                "Password": SECRET,
            }),
            expected_settings: json!({
                "ServerName": "warehouse.example.internal",
                "Port": 5432,
                "DatabaseName": "analytics",
                "Username": "etl",
                "AuthenticationMethod": "password",
            }),
        },
        Case {
            name: "oracle source",
            engine_name: "oracle",
            endpoint_type: "source",
            display_name: "Oracle",
            settings_member: "OracleSettings",
            extra: json!({
                "ServerName": "ora.example.internal",
                "Port": 1521,
                "DatabaseName": "ORCL",
                "Username": "dms",
                "Password": SECRET,
            }),
            expected_settings: json!({
                "ServerName": "ora.example.internal",
                "Port": 1521,
                "DatabaseName": "ORCL",
                "Username": "dms",
                "AuthenticationMethod": "password",
            }),
        },
        Case {
            name: "sqlserver source, caller-named encryption key",
            engine_name: "sqlserver",
            endpoint_type: "source",
            display_name: "Microsoft SQL Server",
            settings_member: "MicrosoftSQLServerSettings",
            extra: json!({
                "ServerName": "mssql.example.internal",
                "Port": 1433,
                "KmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/caller-named-key",
            }),
            expected_settings: json!({
                "ServerName": "mssql.example.internal",
                "Port": 1433,
                "AuthenticationMethod": "password",
            }),
        },
        Case {
            name: "db2 source",
            engine_name: "db2",
            endpoint_type: "source",
            display_name: "IBM Db2 LUW",
            settings_member: "IBMDb2Settings",
            extra: json!({
                "ServerName": "db2.example.internal",
                "Port": 50000,
                "DatabaseName": "SAMPLE",
                "Username": "db2inst1",
            }),
            expected_settings: json!({
                "ServerName": "db2.example.internal",
                "Port": 50000,
                "DatabaseName": "SAMPLE",
                "Username": "db2inst1",
            }),
        },
        Case {
            name: "mongodb source",
            engine_name: "mongodb",
            endpoint_type: "source",
            display_name: "MongoDB",
            settings_member: "MongoDbSettings",
            extra: json!({
                "ServerName": "mongo.example.internal",
                "Port": 27017,
                "DatabaseName": "catalog",
                "Username": "reader",
                // `AuthMechanismValue` models default | mongodb_cr | scram_sha_1.
                "MongoDbSettings": { "AuthMechanism": "scram_sha_1", "Password": SECRET },
            }),
            expected_settings: json!({
                "ServerName": "mongo.example.internal",
                "Port": 27017,
                "DatabaseName": "catalog",
                "Username": "reader",
                "AuthMechanism": "scram_sha_1",
            }),
        },
        Case {
            // An engine whose settings structure is nothing like a database
            // connection: it is reported as the request supplied it.
            name: "s3 target",
            engine_name: "s3",
            endpoint_type: "target",
            display_name: "Amazon S3",
            settings_member: "S3Settings",
            extra: json!({
                "S3Settings": { "BucketName": "landing-zone", "BucketFolder": "cdc" },
            }),
            expected_settings: json!({ "BucketName": "landing-zone", "BucketFolder": "cdc" }),
        },
        Case {
            name: "dynamodb target",
            engine_name: "dynamodb",
            endpoint_type: "target",
            display_name: "Amazon DynamoDB",
            settings_member: "DynamoDbSettings",
            extra: json!({
                "DynamoDbSettings": {
                    "ServiceAccessRoleArn": "arn:aws:iam::000000000000:role/dms-dynamodb",
                },
            }),
            expected_settings: json!({
                "ServiceAccessRoleArn": "arn:aws:iam::000000000000:role/dms-dynamodb",
            }),
        },
        Case {
            // `NeptuneSettings` models required members, so it is reported exactly as
            // the request supplied it and the endpoint's own connection parameters are
            // not merged into it.
            name: "neptune target",
            engine_name: "neptune",
            endpoint_type: "target",
            display_name: "Amazon Neptune",
            settings_member: "NeptuneSettings",
            extra: json!({
                "ServerName": "ignored-by-neptune.example.internal",
                "NeptuneSettings": {
                    "S3BucketName": "neptune-staging",
                    "S3BucketFolder": "bulk",
                    "MaxFileSize": 1024,
                },
            }),
            expected_settings: json!({
                "S3BucketName": "neptune-staging",
                "S3BucketFolder": "bulk",
                "MaxFileSize": 1024,
            }),
        },
    ]
}

/// The engine settings members an `Endpoint` body carries.
fn settings_members(endpoint: &Value) -> Vec<String> {
    endpoint
        .as_object()
        .map(|members| {
            members
                .keys()
                .filter(|name| name.ends_with("Settings"))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Assert the engine settings structure reports exactly `expected` — every member it
/// must carry, and no member the projection would have had to invent.
fn assert_settings(case: &str, member: &str, actual: &Value, expected: &Value) {
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {member} to report exactly {expected}, got {actual}"
    );
}

/// Whether `key_id` has the form of a KMS key id: five hyphen-separated hex groups of
/// 8-4-4-4-12 characters.
fn is_key_id(key_id: &str) -> bool {
    let groups: Vec<&str> = key_id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[tokio::test]
async fn endpoint_projection_reports_every_member_the_record_determines() {
    let emulator = Emulator::new();
    let default_key_prefix = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/");
    let mut default_keys: Vec<String> = Vec::new();

    for (index, case) in cases().into_iter().enumerate() {
        let identifier = format!("ep-{index}");
        let mut request = json!({
            "EndpointIdentifier": identifier,
            "EndpointType": case.endpoint_type,
            "EngineName": case.engine_name,
        });
        for (key, value) in case
            .extra
            .as_object()
            .expect("the table's request members are objects")
        {
            request[key] = value.clone();
        }

        let created = emulator.ok(case.name, "CreateEndpoint", &request).await;
        let endpoint = created
            .get("Endpoint")
            .unwrap_or(&Value::Null)
            .clone();

        // EndpointType comes back uppercased, whatever case the request used.
        assert_eq!(
            text(&endpoint, "/EndpointType"),
            case.endpoint_type.to_uppercase(),
            "case {:?}: expected EndpointType uppercased, got {endpoint}",
            case.name
        );
        assert_eq!(
            text(&endpoint, "/EngineDisplayName"),
            case.display_name,
            "case {:?}: expected the expanded engine name, got {endpoint}",
            case.name
        );
        assert_eq!(
            endpoint.get("IsReadOnly").and_then(Value::as_bool),
            Some(false),
            "case {:?}: expected an endpoint the caller created to report IsReadOnly \
             false, got {endpoint}",
            case.name
        );

        // The encryption key: the caller's when the request named one, otherwise the
        // region's default DMS key, which every endpoint in the region shares.
        let kms_key_id = text(&endpoint, "/KmsKeyId");
        match case.extra.get("KmsKeyId").and_then(Value::as_str) {
            Some(named) => assert_eq!(
                kms_key_id, named,
                "case {:?}: expected the named KMS key, got {endpoint}",
                case.name
            ),
            None => {
                let key_id = kms_key_id
                    .strip_prefix(&default_key_prefix)
                    .unwrap_or_default();
                assert!(
                    is_key_id(key_id),
                    "case {:?}: expected the region's default DMS key under \
                     {default_key_prefix}, got {kms_key_id:?}",
                    case.name
                );
                default_keys.push(kms_key_id.clone());
            }
        }

        // Exactly one engine settings structure, the one EngineName selects.
        assert_eq!(
            settings_members(&endpoint),
            vec![case.settings_member.to_string()],
            "case {:?}: expected only {} on the endpoint, got {endpoint}",
            case.name,
            case.settings_member
        );
        assert_settings(
            case.name,
            case.settings_member,
            endpoint
                .get(case.settings_member)
                .unwrap_or(&Value::Null),
            &case.expected_settings,
        );

        // No password a request carried is ever reported back.
        assert!(
            !created.to_string().contains(SECRET),
            "case {:?}: a response must never echo a password, got {created}",
            case.name
        );

        // The describe path answers from the same projection, member for member.
        let described = emulator
            .ok(
                case.name,
                "DescribeEndpoints",
                &json!({
                    "Filters": [{ "Name": "endpoint-id", "Values": [identifier] }],
                }),
            )
            .await;
        assert_eq!(
            described.pointer("/Endpoints/0"),
            Some(&endpoint),
            "case {:?}: expected DescribeEndpoints to report the endpoint CreateEndpoint \
             returned, got {described}",
            case.name
        );
    }

    // One default key per region, so two endpoints in the same region report the same
    // one and a read repeated later reports it again.
    assert!(
        default_keys.len() > 1 && default_keys.windows(2).all(|pair| pair[0] == pair[1]),
        "expected every endpoint in {REGION} to share the region's default DMS key, got \
         {default_keys:?}"
    );
}

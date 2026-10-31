// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Studio-notebook surface that has to consult the Glue Data
//! Catalog, and for `UpdateApplication`.
//!
//! The rule under test: a write that persists or re-uses an application's
//! `GlueDataCatalogConfiguration` resolves the database its `DatabaseARN` names in the
//! Glue Data Catalog as it stands at that moment, and is refused with
//! `InvalidArgumentException` (HTTP 400) — persisting nothing — when the catalog holds no
//! such database. The resolution is LIVE: the same ARN resolves once Glue's
//! `CreateDatabase` has written the database and stops resolving once `DeleteDatabase` has
//! dropped it.
//!
//! The second rule: an accepted `UpdateApplication` applies the `…Update` members the
//! request carries to the stored application and mints the next `ApplicationVersionId`
//! ("Managed Service for Apache Flink updates the ApplicationVersionId each time you
//! update your application"), leaving every member no update named as it was; a refused
//! one changes nothing at all.
//!
//! Both suites are TABLES whose rows differ along what the rules generalise over: whether
//! the catalog holds the database, the case the database was written under, which catalog
//! (account) the ARN names, the form the ARN takes, and — for the update — which member of
//! the configuration the update names and how the version is addressed. Glue is mounted
//! beside this service on ONE shared engine and driven over its own wire, so what is
//! asserted is cross-service HTTP behaviour and not a direct call into a handler.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, describe, error_code, error_message, ACCOUNT};

/// The region every call in this suite is served in — the one [`super::harness::call`]
/// signs for, so the ARNs here carry it too.
const REGION: &str = "us-east-1";

/// The notebook runtime and mode a Studio notebook is created with
/// (`smithy_query(query="shape",shape="RuntimeEnvironment")`: the `ZEPPELIN-FLINK-`
/// family, run `INTERACTIVE`).
const NOTEBOOK_RUNTIME: &str = "ZEPPELIN-FLINK-3_0";

/// The whole `Message` a catalog reference that resolves to no database is refused with.
/// The fixed text is written out here, so this suite pins the wording rather than reading
/// the service's own constant.
fn missing_database_message(database: &str) -> String {
    format!("Cannot find database with name '{database}' in Glue Data Catalog")
}

/// The whole `Message` a `DatabaseARN` that is not a Glue database ARN is refused with,
/// pinned here the same way.
fn invalid_database_arn_message(database_arn: &str) -> String {
    format!("The DatabaseARN '{database_arn}' is not the ARN of a Glue Data Catalog database")
}

/// An emulator with this service AND Glue mounted on one shared engine: the catalog
/// resolution reads the database out of Glue's own store, so Glue has to be there for
/// there to be anything to resolve.
fn emulator_with_glue() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        engine,
    ));
    router
}

/// Call one Glue operation the way a client does: `POST /` with
/// `X-Amz-Target: AWSGlue.<Operation>`, signed for the `glue` scope the router picks the
/// service by.
async fn glue_call(router: &MultiServiceRouter, op: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("AWSGlue.{op}"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/glue/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Write `database` into the Glue Data Catalog, asserting the create succeeded.
async fn create_database(router: &MultiServiceRouter, case: &str, database: &str) {
    let (status, body) = glue_call(
        router,
        "CreateDatabase",
        &json!({"DatabaseInput": {"Name": database}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateDatabase({database:?}) must answer 200, got {status} with {body}"
    );
}

/// Drop `database` from the Glue Data Catalog, asserting the delete succeeded.
async fn delete_database(router: &MultiServiceRouter, case: &str, database: &str) {
    let (status, body) = glue_call(router, "DeleteDatabase", &json!({"Name": database})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteDatabase({database:?}) must answer 200, got {status} with {body}"
    );
}

/// The ARN of a database of this emulator's own Data Catalog.
fn database_arn(database: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT}:database/{database}")
}

/// A `CreateApplication` body for a Studio notebook whose catalog configuration names
/// `database_arn`, with its Zeppelin log level set to `log_level`.
fn notebook_body(name: &str, database_arn: &str, log_level: &str) -> Value {
    json!({
        "ApplicationName": name,
        "RuntimeEnvironment": NOTEBOOK_RUNTIME,
        "ApplicationMode": "INTERACTIVE",
        "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
        "ApplicationConfiguration": {
            "ZeppelinApplicationConfiguration": {
                "MonitoringConfiguration": {"LogLevel": log_level},
                "CatalogConfiguration": {
                    "GlueDataCatalogConfiguration": {"DatabaseARN": database_arn}
                }
            }
        }
    })
}

/// The `DatabaseARN` a described application reports for its notebook's catalog.
fn described_database_arn(detail: &Value) -> Option<&str> {
    detail
        .pointer(
            "/ApplicationConfigurationDescription/ZeppelinApplicationConfigurationDescription\
             /CatalogConfigurationDescription/GlueDataCatalogConfigurationDescription/DatabaseARN",
        )
        .and_then(Value::as_str)
}

/// The Zeppelin log level a described application reports.
fn described_log_level(detail: &Value) -> Option<&str> {
    detail
        .pointer(
            "/ApplicationConfigurationDescription/ZeppelinApplicationConfigurationDescription\
             /MonitoringConfigurationDescription/LogLevel",
        )
        .and_then(Value::as_str)
}

/// What a row of the catalog table expects of the create it drives.
enum Expect {
    /// The catalog resolves, so the notebook is created.
    Accepted,
    /// The catalog does not resolve: refused, and the refusal quotes this database name.
    Refused(&'static str),
    /// Refused because the value is not the ARN of a Glue database at all, so it names no
    /// database to look for: the refusal quotes the value instead.
    RefusedAsNoDatabaseArn,
}

/// One row: the databases the catalog holds, the `DatabaseARN` the notebook names and
/// what that earns it.
struct CatalogRow {
    case: &'static str,
    /// Databases written into Glue before the create.
    seed: &'static [&'static str],
    /// The ARN the notebook's `GlueDataCatalogConfiguration` carries.
    database_arn: String,
    expect: Expect,
}

#[tokio::test]
async fn a_notebook_catalog_is_resolved_against_the_glue_data_catalog() {
    let router = emulator_with_glue();
    let rows = [
        CatalogRow {
            case: "a database the catalog holds",
            seed: &["probe_db"],
            database_arn: database_arn("probe_db"),
            expect: Expect::Accepted,
        },
        CatalogRow {
            case: "a database the catalog does not hold",
            seed: &[],
            database_arn: database_arn("absent_db"),
            expect: Expect::Refused("absent_db"),
        },
        CatalogRow {
            case: "one of several databases the catalog holds",
            seed: &["analytics_db", "metrics_db_2024", "staging"],
            database_arn: database_arn("metrics_db_2024"),
            expect: Expect::Accepted,
        },
        CatalogRow {
            case: "a sibling name the catalog does not hold",
            seed: &["analytics_db_v2"],
            database_arn: database_arn("analytics_db_v3"),
            expect: Expect::Refused("analytics_db_v3"),
        },
        CatalogRow {
            // The catalog keys a database by its FOLDED name, which is why Glue's own
            // GetDatabase answers for either spelling; the notebook resolves it the same
            // way.
            case: "a database named in another case than it was created in",
            seed: &["MixedCase_DB"],
            database_arn: database_arn("mixedcase_db"),
            expect: Expect::Accepted,
        },
        CatalogRow {
            case: "another account's Data Catalog",
            seed: &["shared_db"],
            database_arn: format!("arn:aws:glue:{REGION}:999999999999:database/shared_db"),
            expect: Expect::Refused("shared_db"),
        },
        CatalogRow {
            case: "an ARN that names a table rather than a database",
            seed: &["probe_db"],
            database_arn: format!("arn:aws:glue:{REGION}:{ACCOUNT}:table/probe_db/events"),
            expect: Expect::RefusedAsNoDatabaseArn,
        },
        CatalogRow {
            case: "an ARN of another service's resource",
            seed: &["probe_db"],
            database_arn: "arn:aws:s3:::probe_db".to_string(),
            expect: Expect::RefusedAsNoDatabaseArn,
        },
        CatalogRow {
            case: "an ARN with no resource at all",
            seed: &["probe_db"],
            database_arn: format!("arn:aws:glue:{REGION}:{ACCOUNT}:"),
            expect: Expect::RefusedAsNoDatabaseArn,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let CatalogRow {
            case,
            seed,
            database_arn,
            expect,
        } = row;
        for database in *seed {
            // A row's seed may name a database an earlier row already wrote; the catalog
            // refuses a duplicate, so only what is missing is written.
            let (status, _) = glue_call(
                &router,
                "GetDatabase",
                &json!({"Name": database.to_string()}),
            )
            .await;
            if status != 200 {
                create_database(&router, case, database).await;
            }
        }

        let name = format!("notebook-{index}");
        let (status, body) = call(
            &router,
            "CreateApplication",
            &notebook_body(&name, database_arn, "INFO"),
        )
        .await;

        match expect {
            Expect::Accepted => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the create to be accepted, got {status} with {body}"
                );
                // Round trip: the stored notebook reports the catalog it was created with.
                let detail = describe(&router, &name).await;
                assert_eq!(
                    described_database_arn(&detail),
                    Some(database_arn.as_str()),
                    "case {case:?}: expected the stored DatabaseARN, got {detail}"
                );
            }
            Expect::Refused(database) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the create to be refused, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidArgumentException",
                    "case {case:?}: expected InvalidArgumentException, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    missing_database_message(database),
                    "case {case:?}: expected the catalog refusal's wording, got {body}"
                );
            }
            Expect::RefusedAsNoDatabaseArn => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the create to be refused, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidArgumentException",
                    "case {case:?}: expected InvalidArgumentException, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    invalid_database_arn_message(database_arn),
                    "case {case:?}: expected the invalid-ARN refusal's wording, got {body}"
                );
            }
        }

        if !matches!(expect, Expect::Accepted) {
            // A refused create leaves nothing behind: no application, and so no ARN.
            let (status, body) = call(
                &router,
                "DescribeApplication",
                &json!({"ApplicationName": name}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a refused create must store no application, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn dropping_the_catalog_database_strands_the_notebook_until_it_is_recreated() {
    let router = emulator_with_glue();
    let case = "a notebook whose database is dropped and re-created";
    let database = "notebook_db";
    let name = "stranded-notebook";
    create_database(&router, case, database).await;

    let (status, body) = call(
        &router,
        "CreateApplication",
        &notebook_body(name, &database_arn(database), "INFO"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create must be accepted while the database exists, got {status} with {body}"
    );

    // The update the caller makes, identical in both attempts below.
    let update = json!({
        "ApplicationName": name,
        "CurrentApplicationVersionId": 1,
        "ApplicationConfigurationUpdate": {
            "ZeppelinApplicationConfigurationUpdate": {
                "MonitoringConfigurationUpdate": {"LogLevelUpdate": "WARN"}
            }
        }
    });

    delete_database(&router, case, database).await;
    // The notebook still DESCRIBES the now-dangling ARN: dropping the database does not
    // rewrite the notebook's configuration.
    let detail = describe(&router, name).await;
    assert_eq!(
        described_database_arn(&detail),
        Some(database_arn(database).as_str()),
        "case {case:?}: expected the notebook to still name its database, got {detail}"
    );

    let (status, body) = call(&router, "UpdateApplication", &update).await;
    assert_eq!(
        status, 400,
        "case {case:?}: an update must be refused while the database is absent, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidArgumentException",
        "case {case:?}: expected InvalidArgumentException, got {body}"
    );
    assert_eq!(
        error_message(&body),
        missing_database_message(database),
        "case {case:?}: expected the catalog refusal's wording, got {body}"
    );

    // The refusal is side-effect free: neither the version nor the log level moved.
    let detail = describe(&router, name).await;
    assert_eq!(
        detail["ApplicationVersionId"].as_i64(),
        Some(1),
        "case {case:?}: a refused update must not mint a version, got {detail}"
    );
    assert_eq!(
        described_log_level(&detail),
        Some("INFO"),
        "case {case:?}: a refused update must not change the log level, got {detail}"
    );

    create_database(&router, case, database).await;
    let (status, body) = call(&router, "UpdateApplication", &update).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the identical update must be accepted once the database exists, got {status} with {body}"
    );
    let updated = &body["ApplicationDetail"];
    assert_eq!(
        updated["ApplicationVersionId"].as_i64(),
        Some(2),
        "case {case:?}: an accepted update mints the next version, got {updated}"
    );
    assert_eq!(
        updated["ApplicationVersionUpdatedFrom"].as_i64(),
        Some(1),
        "case {case:?}: the version it was updated from is reported, got {updated}"
    );
    assert!(
        body["OperationId"].as_str().is_some_and(|id| !id.is_empty()),
        "case {case:?}: expected an OperationId for the update, got {body}"
    );

    let detail = describe(&router, name).await;
    assert_eq!(
        described_log_level(&detail),
        Some("WARN"),
        "case {case:?}: expected the updated log level, got {detail}"
    );
    assert_eq!(
        detail["ApplicationVersionId"].as_i64(),
        Some(2),
        "case {case:?}: expected the minted version, got {detail}"
    );
    // The members no update named are untouched.
    assert_eq!(
        described_database_arn(&detail),
        Some(database_arn(database).as_str()),
        "case {case:?}: expected the catalog to survive the update, got {detail}"
    );
}

/// A notebook's monitoring description reports the verbosity in force: the one the write
/// configured, and the service default for one that configured none.
///
/// The rows differ along WHERE the level in force comes from: a create that named it, a
/// create that did not, and an update that set it on either of those.
#[tokio::test]
async fn a_notebook_reports_the_log_level_in_force() {
    let router = emulator_with_glue();
    let case = "a notebook that configured no monitoring";
    let database = "default_level_db";
    create_database(&router, case, database).await;

    // (case, the ZeppelinApplicationConfiguration to create with, the level described)
    let rows: [(&str, Value, &str); 2] = [
        (
            "no monitoring configuration at all",
            json!({
                "CatalogConfiguration": {
                    "GlueDataCatalogConfiguration": {"DatabaseARN": database_arn(database)}
                }
            }),
            "INFO",
        ),
        (
            "a monitoring configuration naming DEBUG",
            json!({
                "MonitoringConfiguration": {"LogLevel": "DEBUG"},
                "CatalogConfiguration": {
                    "GlueDataCatalogConfiguration": {"DatabaseARN": database_arn(database)}
                }
            }),
            "DEBUG",
        ),
    ];

    for (index, (case, zeppelin, expected)) in rows.iter().enumerate() {
        let name = format!("log-level-notebook-{index}");
        let (status, body) = call(
            &router,
            "CreateApplication",
            &json!({
                "ApplicationName": name,
                "RuntimeEnvironment": NOTEBOOK_RUNTIME,
                "ApplicationMode": "INTERACTIVE",
                "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
                "ApplicationConfiguration": {"ZeppelinApplicationConfiguration": zeppelin},
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApplication got {status} with {body}"
        );
        let detail = describe(&router, &name).await;
        assert_eq!(
            described_log_level(&detail),
            Some(*expected),
            "case {case:?}: expected the level in force, got {detail}"
        );

        // An update moves it, from either starting point.
        let (status, body) = call(
            &router,
            "UpdateApplication",
            &json!({
                "ApplicationName": name,
                "CurrentApplicationVersionId": 1,
                "ApplicationConfigurationUpdate": {
                    "ZeppelinApplicationConfigurationUpdate": {
                        "MonitoringConfigurationUpdate": {"LogLevelUpdate": "ERROR"}
                    }
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateApplication got {status} with {body}"
        );
        let detail = describe(&router, &name).await;
        assert_eq!(
            described_log_level(&detail),
            Some("ERROR"),
            "case {case:?}: expected the updated level, got {detail}"
        );
    }
}

/// One row of the update table: the `UpdateApplication` members to send, and a pointer
/// into the described application plus the value it must report afterwards.
struct UpdateRow {
    case: &'static str,
    /// Members merged into the `UpdateApplication` body, beside `ApplicationName` and
    /// `CurrentApplicationVersionId`.
    update: Value,
    /// What the described application must report, as (pointer, value) pairs.
    expected: Vec<(&'static str, Value)>,
}

#[tokio::test]
async fn an_accepted_update_applies_what_it_names_and_mints_the_next_version() {
    let router = emulator_with_glue();
    // The rows differ along WHICH member of the application an update names: a nested
    // Flink sub-configuration, a whole structure, a scalar of the application itself, the
    // code it runs, and its environment properties.
    let rows = [
        UpdateRow {
            case: "the Flink monitoring configuration",
            update: json!({
                "ApplicationConfigurationUpdate": {
                    "FlinkApplicationConfigurationUpdate": {
                        "MonitoringConfigurationUpdate": {
                            "ConfigurationTypeUpdate": "CUSTOM",
                            "LogLevelUpdate": "DEBUG",
                            "MetricsLevelUpdate": "TASK",
                        }
                    }
                }
            }),
            expected: vec![
                (
                    "/ApplicationConfigurationDescription/FlinkApplicationConfigurationDescription/MonitoringConfigurationDescription/LogLevel",
                    json!("DEBUG"),
                ),
                (
                    "/ApplicationConfigurationDescription/FlinkApplicationConfigurationDescription/MonitoringConfigurationDescription/MetricsLevel",
                    json!("TASK"),
                ),
                (
                    "/ApplicationConfigurationDescription/FlinkApplicationConfigurationDescription/MonitoringConfigurationDescription/ConfigurationType",
                    json!("CUSTOM"),
                ),
            ],
        },
        UpdateRow {
            case: "the checkpoint configuration",
            update: json!({
                "ApplicationConfigurationUpdate": {
                    "FlinkApplicationConfigurationUpdate": {
                        "CheckpointConfigurationUpdate": {
                            "ConfigurationTypeUpdate": "CUSTOM",
                            "CheckpointingEnabledUpdate": true,
                            "CheckpointIntervalUpdate": 120000,
                            "MinPauseBetweenCheckpointsUpdate": 0,
                        }
                    }
                }
            }),
            expected: vec![
                (
                    "/ApplicationConfigurationDescription/FlinkApplicationConfigurationDescription/CheckpointConfigurationDescription/CheckpointInterval",
                    json!(120_000),
                ),
                (
                    "/ApplicationConfigurationDescription/FlinkApplicationConfigurationDescription/CheckpointConfigurationDescription/CheckpointingEnabled",
                    json!(true),
                ),
                (
                    "/ApplicationConfigurationDescription/FlinkApplicationConfigurationDescription/CheckpointConfigurationDescription/MinPauseBetweenCheckpoints",
                    json!(0),
                ),
            ],
        },
        UpdateRow {
            case: "the parallelism configuration",
            update: json!({
                "ApplicationConfigurationUpdate": {
                    "FlinkApplicationConfigurationUpdate": {
                        "ParallelismConfigurationUpdate": {
                            "ConfigurationTypeUpdate": "CUSTOM",
                            "ParallelismUpdate": 4,
                            "ParallelismPerKPUUpdate": 2,
                            "AutoScalingEnabledUpdate": false,
                        }
                    }
                }
            }),
            expected: vec![
                (
                    "/ApplicationConfigurationDescription/FlinkApplicationConfigurationDescription/ParallelismConfigurationDescription/Parallelism",
                    json!(4),
                ),
                (
                    "/ApplicationConfigurationDescription/FlinkApplicationConfigurationDescription/ParallelismConfigurationDescription/CurrentParallelism",
                    json!(4),
                ),
                (
                    "/ApplicationConfigurationDescription/FlinkApplicationConfigurationDescription/ParallelismConfigurationDescription/AutoScalingEnabled",
                    json!(false),
                ),
            ],
        },
        UpdateRow {
            case: "the snapshot configuration",
            update: json!({
                "ApplicationConfigurationUpdate": {
                    "ApplicationSnapshotConfigurationUpdate": {"SnapshotsEnabledUpdate": true}
                }
            }),
            expected: vec![(
                "/ApplicationConfigurationDescription/ApplicationSnapshotConfigurationDescription/SnapshotsEnabled",
                json!(true),
            )],
        },
        UpdateRow {
            case: "the environment properties",
            update: json!({
                "ApplicationConfigurationUpdate": {
                    "EnvironmentPropertyUpdates": {
                        "PropertyGroups": [
                            {"PropertyGroupId": "producer", "PropertyMap": {"topic": "orders"}}
                        ]
                    }
                }
            }),
            expected: vec![(
                "/ApplicationConfigurationDescription/EnvironmentPropertyDescriptions/PropertyGroupDescriptions/0/PropertyGroupId",
                json!("producer"),
            )],
        },
        UpdateRow {
            case: "the code the application runs",
            update: json!({
                "ApplicationConfigurationUpdate": {
                    "ApplicationCodeConfigurationUpdate": {
                        "CodeContentTypeUpdate": "PLAINTEXT",
                        "CodeContentUpdate": {"TextContentUpdate": "-- the new code"}
                    }
                }
            }),
            expected: vec![
                (
                    "/ApplicationConfigurationDescription/ApplicationCodeConfigurationDescription/CodeContentDescription/TextContent",
                    json!("-- the new code"),
                ),
                (
                    "/ApplicationConfigurationDescription/ApplicationCodeConfigurationDescription/CodeContentDescription/CodeSize",
                    json!("-- the new code".len()),
                ),
            ],
        },
        UpdateRow {
            case: "the service execution role of the application itself",
            update: json!({
                "ServiceExecutionRoleUpdate": "arn:aws:iam::000000000000:role/kda-other-role"
            }),
            expected: vec![(
                "/ServiceExecutionRole",
                json!("arn:aws:iam::000000000000:role/kda-other-role"),
            )],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let UpdateRow {
            case,
            update,
            expected,
        } = row;
        // One application per row, so each row's update is the FIRST one that application
        // sees and the version it mints is always 2.
        let name = format!("updated-app-{index}");
        let (status, body) = call(
            &router,
            "CreateApplication",
            &json!({
                "ApplicationName": name,
                "RuntimeEnvironment": "FLINK-1_20",
                "ApplicationMode": "STREAMING",
                "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
                "ApplicationConfiguration": {
                    "ApplicationCodeConfiguration": {
                        "CodeContentType": "PLAINTEXT",
                        "CodeContent": {"TextContent": "-- the original code"}
                    }
                }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApplication got {status} with {body}"
        );

        let mut request = json!({"ApplicationName": name, "CurrentApplicationVersionId": 1});
        for (member, value) in update.as_object().expect("the row's update is an object") {
            request[member] = value.clone();
        }
        let (status, body) = call(&router, "UpdateApplication", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be accepted, got {status} with {body}"
        );
        assert_eq!(
            body["ApplicationDetail"]["ApplicationVersionId"].as_i64(),
            Some(2),
            "case {case:?}: an accepted update mints the next version, got {body}"
        );

        let detail = describe(&router, &name).await;
        for (pointer, value) in expected {
            assert_eq!(
                detail.pointer(pointer),
                Some(value),
                "case {case:?}: expected {pointer} to report {value}, got {detail}"
            );
        }
        // Nothing the update did not name moved: the application keeps its name, its
        // runtime and its create-time configuration members.
        assert_eq!(
            detail["RuntimeEnvironment"].as_str(),
            Some("FLINK-1_20"),
            "case {case:?}: the runtime was not updated, got {detail}"
        );
        if *case != "the code the application runs" {
            assert_eq!(
                detail.pointer(
                    "/ApplicationConfigurationDescription/ApplicationCodeConfigurationDescription\
                     /CodeContentDescription/TextContent"
                ),
                Some(&json!("-- the original code")),
                "case {case:?}: the stored code was not updated, got {detail}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_is_refused_unless_it_names_the_version_in_force() {
    let router = emulator_with_glue();
    let name = "concurrency-app";
    let (status, body) = call(
        &router,
        "CreateApplication",
        &json!({
            "ApplicationName": name,
            "RuntimeEnvironment": "FLINK-1_20",
            "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication got {status} with {body}");
    let token = body["ApplicationDetail"]["ConditionalToken"]
        .as_str()
        .expect("a created application carries a ConditionalToken")
        .to_string();

    // (case, the members addressing the version, expected status, expected error code)
    let rows: [(&str, Value, u16, &str); 5] = [
        (
            "neither the version nor the token",
            json!({}),
            400,
            "InvalidArgumentException",
        ),
        (
            "a version that is not the one in force",
            json!({"CurrentApplicationVersionId": 7}),
            409,
            "ConcurrentModificationException",
        ),
        (
            "a token that is not the one in force",
            json!({"ConditionalToken": "not-the-stored-token"}),
            409,
            "ConcurrentModificationException",
        ),
        (
            "the version in force",
            json!({"CurrentApplicationVersionId": 1}),
            200,
            "",
        ),
        (
            "the token of a version that has been consumed",
            json!({"ConditionalToken": token}),
            409,
            "ConcurrentModificationException",
        ),
    ];

    for (case, addressing, expected_status, expected_code) in rows {
        let mut request = json!({
            "ApplicationName": name,
            "ApplicationConfigurationUpdate": {
                "ApplicationSnapshotConfigurationUpdate": {"SnapshotsEnabledUpdate": true}
            }
        });
        for (member, value) in addressing.as_object().expect("an object") {
            request[member] = value.clone();
        }

        let (status, body) = call(&router, "UpdateApplication", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        if expected_code.is_empty() {
            continue;
        }
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Exactly ONE of those updates was accepted, so the application is on version 2.
    let detail = describe(&router, name).await;
    assert_eq!(
        detail["ApplicationVersionId"].as_i64(),
        Some(2),
        "expected exactly one accepted update, got {detail}"
    );
}

#[tokio::test]
async fn an_update_naming_an_element_the_application_does_not_hold_changes_nothing() {
    let router = emulator_with_glue();
    let name = "unknown-element-app";
    let (status, body) = call(
        &router,
        "CreateApplication",
        &json!({
            "ApplicationName": name,
            "RuntimeEnvironment": "FLINK-1_20",
            "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication got {status} with {body}");

    // (case, the update that names an element by an id the application holds none under)
    let rows: [(&str, Value); 3] = [
        (
            "a CloudWatch logging option",
            json!({"CloudWatchLoggingOptionUpdates": [{
                "CloudWatchLoggingOptionId": "cw-absent",
                "LogStreamARNUpdate": "arn:aws:logs:us-east-1:000000000000:log-group:g:log-stream:s",
            }]}),
        ),
        (
            "a VPC configuration",
            json!({"ApplicationConfigurationUpdate": {"VpcConfigurationUpdates": [{
                "VpcConfigurationId": "vpc-absent",
                "SubnetIdUpdates": ["subnet-1"],
            }]}}),
        ),
        (
            "an SQL input",
            json!({"ApplicationConfigurationUpdate": {"SqlApplicationConfigurationUpdate": {
                "InputUpdates": [{"InputId": "input-absent", "NamePrefixUpdate": "RENAMED"}],
            }}}),
        ),
    ];

    for (case, update) in rows {
        let mut request = json!({"ApplicationName": name, "CurrentApplicationVersionId": 1});
        for (member, value) in update.as_object().expect("an object") {
            request[member] = value.clone();
        }

        let (status, body) = call(&router, "UpdateApplication", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidArgumentException",
            "case {case:?}: expected InvalidArgumentException, got {body}"
        );

        let detail = describe(&router, name).await;
        assert_eq!(
            detail["ApplicationVersionId"].as_i64(),
            Some(1),
            "case {case:?}: a refused update must not mint a version, got {detail}"
        );
    }
}

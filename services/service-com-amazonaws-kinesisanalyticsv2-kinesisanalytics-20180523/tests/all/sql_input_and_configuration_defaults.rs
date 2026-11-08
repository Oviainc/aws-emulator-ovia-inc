// SPDX-License-Identifier: CC0-1.0

//! Adding a streaming source, and the configuration an application RUNS under.
//!
//! Every test drives the service the way a client does — one `POST /` per call through the
//! real `MultiServiceRouter`, with `X-Amz-Target: KinesisAnalytics_20180523.<Operation>`.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! * a streaming source belongs to a SQL-based application, so `AddApplicationInput`
//!   against an application of any other runtime family is refused with the modelled
//!   `InvalidArgumentException` and stores nothing;
//! * an accepted `AddApplicationInput` is taken only against the version the application is
//!   on now, stores the input with the id and in-app stream names the service assigns, and
//!   moves the application on to a new version;
//! * an application always runs under a FULL configuration: every sub-configuration that
//!   applies to its runtime is described, with `ConfigurationType` `DEFAULT` and the
//!   documented default values wherever the request supplied none — and a `DEFAULT`
//!   sub-configuration reports those defaults whatever else the request carried.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::ops::types::RUNTIME_ENVIRONMENT_VALUES;
use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::state;

use super::harness::{call, create, create_body, describe, error_code, error_message, router, router_on};

// ---------------------------------------------------------------------------
// The streaming sources a request can carry
// ---------------------------------------------------------------------------

/// An `InputSchema` over `record_format`: the schema member is required on every `Input`,
/// and the column list is what a SQL application selects from.
fn input_schema(record_format: Value) -> Value {
    json!({
        "RecordFormat": record_format,
        "RecordColumns": [{"Name": "COL_A", "SqlType": "VARCHAR(16)", "Mapping": "$.a"}]
    })
}

/// One streaming source a caller can add: the case name, the `Input` itself, and the
/// in-app stream names the service must report for it.
///
/// The rows span the forms `Input` takes (`smithy_query(query="shape",shape="Input")`): a
/// Kinesis data stream source, a Kinesis Data Firehose delivery stream source, a source
/// with a Lambda pre-processor, both record formats, and the parallelism counts that size
/// the in-app stream list — so the rule is asserted about the shape class and not about the
/// one source the issue cited.
fn streaming_sources() -> Vec<(&'static str, Value, Vec<String>)> {
    vec![
        (
            "a kinesis data stream, JSON records, default parallelism",
            json!({
                "NamePrefix": "SOURCE_SQL_STREAM",
                "KinesisStreamsInput": {
                    "ResourceARN": "arn:aws:kinesis:us-east-1:000000000000:stream/in"
                },
                "InputSchema": input_schema(json!({
                    "RecordFormatType": "JSON",
                    "MappingParameters": {"JSONMappingParameters": {"RecordRowPath": "$"}}
                }))
            }),
            vec!["SOURCE_SQL_STREAM_001".to_string()],
        ),
        (
            "a firehose delivery stream, CSV records, parallelism 3",
            json!({
                "NamePrefix": "FIREHOSE_IN",
                "KinesisFirehoseInput": {
                    "ResourceARN": "arn:aws:firehose:us-east-1:000000000000:deliverystream/in"
                },
                "InputParallelism": {"Count": 3},
                "InputSchema": input_schema(json!({
                    "RecordFormatType": "CSV",
                    "MappingParameters": {
                        "CSVMappingParameters": {
                            "RecordRowDelimiter": "\n",
                            "RecordColumnDelimiter": ","
                        }
                    }
                }))
            }),
            (1..=3).map(|n| format!("FIREHOSE_IN_{n:03}")).collect(),
        ),
        (
            "a pre-processed source at the top of the parallelism range",
            json!({
                "NamePrefix": "PRE",
                "KinesisStreamsInput": {
                    "ResourceARN": "arn:aws:kinesis:us-east-1:000000000000:stream/pre"
                },
                "InputProcessingConfiguration": {
                    "InputLambdaProcessor": {
                        "ResourceARN": "arn:aws:lambda:us-east-1:000000000000:function:pre"
                    }
                },
                "InputParallelism": {"Count": 64},
                "InputSchema": input_schema(json!({"RecordFormatType": "JSON"}))
            }),
            (1..=64).map(|n| format!("PRE_{n:03}")).collect(),
        ),
    ]
}

/// Ask for a streaming source to be added and return `(status, body)`.
async fn add_input(
    router: &MultiServiceRouter,
    application: &str,
    version: i64,
    source: &Value,
) -> (u16, Value) {
    call(
        router,
        "AddApplicationInput",
        &json!({
            "ApplicationName": application,
            "CurrentApplicationVersionId": version,
            "Input": source,
        }),
    )
    .await
}

/// The `InputDescriptions` an application's stored configuration reports, as a JSON value
/// (`Null` when it holds none).
fn stored_inputs(detail: &Value) -> Value {
    detail["ApplicationConfigurationDescription"]["SqlApplicationConfigurationDescription"]
        ["InputDescriptions"]
        .clone()
}

// ---------------------------------------------------------------------------
// A streaming source belongs to a SQL-based application
// ---------------------------------------------------------------------------

/// `AddApplicationInput` against an application whose runtime is not a SQL one is refused
/// with HTTP 400 `InvalidArgumentException` and the sentence AWS states for that class, and
/// the refusal is inert: the application keeps its version and stores no input description.
///
/// The table is every runtime the model declares that this API still creates applications
/// for — read from the generated `RUNTIME_ENVIRONMENT_VALUES`
/// (`src/ops/types/shared_types.rs`, which the model's `RuntimeEnvironment` shape
/// generates), so a runtime the issue never cited is covered — crossed with the forms an
/// `Input` takes.
#[tokio::test]
async fn adding_a_streaming_source_to_a_non_sql_application_is_refused() {
    let router = router();
    let sources = streaming_sources();

    for runtime in RUNTIME_ENVIRONMENT_VALUES
        .iter()
        .filter(|runtime| !state::is_sql_runtime(runtime))
    {
        let name = format!("no-sql-{}", runtime.replace(['-', '_'], "."));
        create(&router, &name, runtime).await;

        for (source_case, source, _) in &sources {
            let case = format!("{runtime}/{source_case}");
            let (status, refused) = add_input(&router, &name, 1, source).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a non-SQL runtime, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidArgumentException",
                "case {case:?}: expected InvalidArgumentException, got {refused}"
            );
            assert_eq!(
                error_message(&refused),
                "You cannot add sql configuration to a Flink application.",
                "case {case:?}: expected the SQL-on-Flink refusal sentence, got {refused}"
            );

            // Inert: no new version, and nothing stored under the SQL configuration.
            let described = describe(&router, &name).await;
            assert_eq!(
                described["ApplicationVersionId"].as_i64(),
                Some(1),
                "case {case:?}: a refused add must not version the application, got {described}"
            );
            assert_eq!(
                stored_inputs(&described),
                Value::Null,
                "case {case:?}: a refused add must store no input description, got {described}"
            );
        }
    }
}

/// A name no application is stored under is reported as the modelled
/// `ResourceNotFoundException`, in this service's one sentence for a lookup miss.
///
/// The names span the `ApplicationName` space (len 1..128, `^[a-zA-Z0-9_.-]+$`), with one
/// stored application present so a miss is a miss about the store and not about an empty
/// one.
#[tokio::test]
async fn adding_a_streaming_source_to_an_unstored_application_is_a_lookup_miss() {
    let router = router();
    create(&router, "stored-app", "FLINK-1_20").await;
    let longest = "n".repeat(128);
    let source = streaming_sources().remove(0).1;

    for name in ["a", "no-such-app", "probe.app_2-absent", longest.as_str()] {
        let (status, refused) = add_input(&router, name, 1, &source).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!("Application '{name}' does not exist."),
            "case {name:?}: expected the lookup-miss sentence, got {refused}"
        );
    }
}

// ---------------------------------------------------------------------------
// The accepted path, and the version token it is taken against
// ---------------------------------------------------------------------------

/// Store a SQL-based application under `name` and return the router it is reachable
/// through.
///
/// `CreateApplication` no longer provisions the retired SQL runtime family (see
/// `modeled_client_errors.rs`), so the application is created through the real create path
/// and its stored `RuntimeEnvironment` is then re-stamped in the store — which is exactly
/// what an application created before that family was retired looks like, and what
/// `AddApplicationInput` reads when it decides.
async fn sql_application(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    name: &str,
    runtime: &str,
) {
    create(router, name, "FLINK-1_20").await;
    let store = state::service_state(&**engine).expect("the service holds state");
    let mut guard = store.lock();
    let application = guard
        .applications
        .get_mut(name)
        .unwrap_or_else(|| panic!("case {name:?}: the application was just created"));
    application.runtime_environment = runtime.to_string();
    application.configuration = None;
}

/// An accepted `AddApplicationInput` stores the source with the id and in-app stream names
/// the service assigns, answers them out of the store, and moves the application on to a
/// new version; a second source is refused, since a SQL-based application reads from one.
///
/// The rows are the `Input` forms of [`streaming_sources`], each on its own application,
/// and the stored source is read back through `DescribeApplication` — the round trip that
/// proves the answer came from state.
#[tokio::test]
async fn an_accepted_streaming_source_is_stored_and_versions_the_application() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    for (case, source, expected_streams) in streaming_sources() {
        let name = format!("sql-app-{}", expected_streams.len());
        sql_application(&engine, &router, &name, "SQL-1_0").await;
        let before = describe(&router, &name).await;

        let (status, added) = add_input(&router, &name, 1, &source).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the source to be added, got {status} {added}"
        );
        assert_eq!(
            added["ApplicationARN"].as_str(),
            before["ApplicationARN"].as_str(),
            "case {case:?}: expected the stored application ARN, got {added}"
        );
        assert_eq!(
            added["ApplicationVersionId"].as_i64(),
            Some(2),
            "case {case:?}: a configuration update results in a new version, got {added}"
        );
        let descriptions = added["InputDescriptions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected InputDescriptions, got {added}"))
            .clone();
        assert_eq!(
            descriptions.len(),
            1,
            "case {case:?}: expected the one stored source, got {added}"
        );
        let description = &descriptions[0];
        assert_eq!(
            description["NamePrefix"].as_str(),
            source["NamePrefix"].as_str(),
            "case {case:?}: expected the supplied name prefix, got {description}"
        );
        assert!(
            description["InputId"]
                .as_str()
                .is_some_and(|id| !id.is_empty() && id.len() <= 50),
            "case {case:?}: expected a server-assigned InputId (len 1..50), got {description}"
        );
        assert_eq!(
            description["InAppStreamNames"],
            json!(expected_streams),
            "case {case:?}: expected one in-app stream per unit of parallelism, got \
             {description}"
        );
        assert_eq!(
            description["InputSchema"]["RecordFormat"]["RecordFormatType"].as_str(),
            source["InputSchema"]["RecordFormat"]["RecordFormatType"].as_str(),
            "case {case:?}: expected the supplied record format, got {description}"
        );

        // The round trip: the reader reports what the writer stored, on the new version.
        let described = describe(&router, &name).await;
        assert_eq!(
            stored_inputs(&described),
            json!(descriptions),
            "case {case:?}: expected describe to report the stored source, got {described}"
        );
        assert_eq!(
            described["ApplicationVersionId"].as_i64(),
            Some(2),
            "case {case:?}: expected the new version, got {described}"
        );
        assert_eq!(
            described["ApplicationVersionUpdatedFrom"].as_i64(),
            Some(1),
            "case {case:?}: expected the version the update moved on from, got {described}"
        );
        assert_ne!(
            described["ConditionalToken"].as_str(),
            before["ConditionalToken"].as_str(),
            "case {case:?}: the token naming the current version must be re-minted, got \
             {described}"
        );

        // One streaming source per SQL-based application: a second one is refused, on the
        // version the application is now on, and changes nothing.
        let (status, refused) = add_input(&router, &name, 2, &source).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidArgumentException"),
            "case {case:?}: expected a second source to be refused, got {status} {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!(
                "Application {name} already has a streaming source. A SQL-based application \
                 supports only one streaming source."
            ),
            "case {case:?}: expected the one-source sentence, got {refused}"
        );
        let after = describe(&router, &name).await;
        assert_eq!(
            after, described,
            "case {case:?}: a refused add must leave the application as it was, got {after}"
        );
    }
}

/// A `CurrentApplicationVersionId` that is not the version the application is on is refused
/// with HTTP 409 `ConcurrentModificationException` and the fixed sentence for that class,
/// and stores nothing.
///
/// The rows span the stale values a caller can send inside the modelled range
/// (`ApplicationVersionId` 1..999999999): the version after the current one, a version far
/// ahead, the published upper bound, and — after an accepted add has moved the application
/// on — the version that has just been CONSUMED.
#[tokio::test]
async fn a_stale_application_version_is_refused_as_a_concurrent_modification() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    let name = "version-token-app";
    sql_application(&engine, &router, name, "SQL-1_0").await;
    let source = streaming_sources().remove(0).1;

    for stale in [2_i64, 7, 999_999_999] {
        let (status, refused) = add_input(&router, name, stale, &source).await;
        assert_eq!(
            status, 409,
            "case {stale}: expected HTTP 409 for a stale version, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConcurrentModificationException",
            "case {stale}: expected ConcurrentModificationException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            "The provided application version ID does not match the most recent version \
             for the application.",
            "case {stale}: expected the stale-version sentence, got {refused}"
        );
        let described = describe(&router, name).await;
        assert_eq!(
            described["ApplicationVersionId"].as_i64(),
            Some(1),
            "case {stale}: a refused add must not version the application, got {described}"
        );
        assert_eq!(
            stored_inputs(&described),
            Value::Null,
            "case {stale}: a refused add must store no input description, got {described}"
        );
    }

    // The current version is accepted…
    let (status, added) = add_input(&router, name, 1, &source).await;
    assert_eq!(status, 200, "case current: AddApplicationInput got {added}");

    // …and replaying that now-consumed version is refused like any other stale one.
    let (status, refused) = add_input(&router, name, 1, &source).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (409, "ConcurrentModificationException"),
        "case consumed: expected a consumed version to be refused, got {status} {refused}"
    );
}

// ---------------------------------------------------------------------------
// The configuration an application runs under
// ---------------------------------------------------------------------------

/// The checkpointing a `DEFAULT` checkpoint configuration runs under, from the
/// `CheckpointConfiguration$ConfigurationType` doc
/// (`smithy_query(query="doc",shape="CheckpointConfiguration",member="ConfigurationType")`).
fn default_checkpoint() -> Value {
    json!({
        "ConfigurationType": "DEFAULT",
        "CheckpointingEnabled": true,
        "CheckpointInterval": 60_000,
        "MinPauseBetweenCheckpoints": 5_000
    })
}

/// The same `DEFAULT` checkpoint table for an application the service runs INTERACTIVELY
/// (a Studio notebook): the intervals are the service's, and checkpointing itself is off,
/// because what `CheckpointingEnabled` switches is a feature of a Managed Service for
/// Apache Flink application
/// (`smithy_query(query="doc",shape="CheckpointConfiguration",member="CheckpointingEnabled")`).
fn interactive_default_checkpoint() -> Value {
    let mut checkpoint = default_checkpoint();
    checkpoint["CheckpointingEnabled"] = json!(false);
    checkpoint
}

/// The CloudWatch logging a `DEFAULT` monitoring configuration runs under: the service's
/// published defaults, each a member of its modelled enum (`MetricsLevel` / `LogLevel`).
fn default_monitoring() -> Value {
    json!({
        "ConfigurationType": "DEFAULT",
        "MetricsLevel": "APPLICATION",
        "LogLevel": "INFO"
    })
}

/// The parallelism a `DEFAULT` parallelism configuration runs under: the service's
/// published defaults, with the parallelism in force equal to the configured one.
fn default_parallelism() -> Value {
    json!({
        "ConfigurationType": "DEFAULT",
        "Parallelism": 1,
        "ParallelismPerKPU": 1,
        "CurrentParallelism": 1,
        "AutoScalingEnabled": true
    })
}

/// Assert every member of `expected` is reported with that value in `actual`.
fn assert_members(case: &str, member: &str, actual: &Value, expected: &Value) {
    let members = expected
        .as_object()
        .expect("the expectation is a JSON object");
    for (key, value) in members {
        assert_eq!(
            &actual[key], value,
            "case {case:?}: expected {member}.{key} to be {value}, got {actual}"
        );
    }
}

/// One application to create: what configuration it is created with, and what its
/// configuration description must then report.
struct ConfigurationCase {
    case: &'static str,
    runtime: &'static str,
    mode: Option<&'static str>,
    /// The `ApplicationConfiguration` the create carries, or `Null` for none at all.
    configuration: Value,
    checkpoint: Value,
    monitoring: Value,
    parallelism: Value,
    snapshots_enabled: bool,
}

/// Every created application reports a FULL `ApplicationConfigurationDescription`: the
/// sub-configurations its runtime runs under, with `ConfigurationType` `DEFAULT` and the
/// documented default values wherever the request supplied none, the caller's values where
/// it supplied `CUSTOM` ones, and the defaults again for a `CUSTOM` member left unset.
///
/// The rows span the ways a configuration can be incomplete — no `ApplicationConfiguration`
/// at all, one with no Flink configuration, a Flink configuration carrying only one of its
/// three sub-configurations, a `DEFAULT` sub-configuration that also carries values (which
/// `DEFAULT` means the service ignores), a `CUSTOM` one with members left unset, and an
/// explicit `SnapshotsEnabled false` which is the caller's value and not a default — across
/// three runtime families.
#[tokio::test]
async fn a_created_application_reports_the_configuration_it_runs_under() {
    let cases = [
        ConfigurationCase {
            case: "no application configuration at all",
            runtime: "FLINK-1_15",
            mode: Some("STREAMING"),
            configuration: Value::Null,
            checkpoint: default_checkpoint(),
            monitoring: default_monitoring(),
            parallelism: default_parallelism(),
            snapshots_enabled: true,
        },
        ConfigurationCase {
            case: "a configuration with no flink configuration",
            runtime: "FLINK-1_20",
            mode: None,
            configuration: json!({
                "ApplicationCodeConfiguration": {
                    "CodeContentType": "PLAINTEXT",
                    "CodeContent": {"TextContent": "env.execute();"}
                }
            }),
            checkpoint: default_checkpoint(),
            monitoring: default_monitoring(),
            parallelism: default_parallelism(),
            snapshots_enabled: true,
        },
        ConfigurationCase {
            case: "a flink configuration carrying only monitoring",
            runtime: "FLINK-1_8",
            mode: None,
            configuration: json!({
                "FlinkApplicationConfiguration": {
                    "MonitoringConfiguration": {
                        "ConfigurationType": "CUSTOM",
                        "LogLevel": "DEBUG",
                        "MetricsLevel": "TASK"
                    }
                }
            }),
            checkpoint: default_checkpoint(),
            monitoring: json!({
                "ConfigurationType": "CUSTOM",
                "LogLevel": "DEBUG",
                "MetricsLevel": "TASK"
            }),
            parallelism: default_parallelism(),
            snapshots_enabled: true,
        },
        ConfigurationCase {
            case: "a DEFAULT sub-configuration that also carries values",
            runtime: "FLINK-2_3",
            mode: None,
            configuration: json!({
                "FlinkApplicationConfiguration": {
                    "CheckpointConfiguration": {
                        "ConfigurationType": "DEFAULT",
                        "CheckpointingEnabled": false,
                        "CheckpointInterval": 120_000,
                        "MinPauseBetweenCheckpoints": 90_000
                    }
                }
            }),
            checkpoint: default_checkpoint(),
            monitoring: default_monitoring(),
            parallelism: default_parallelism(),
            snapshots_enabled: true,
        },
        ConfigurationCase {
            case: "a CUSTOM sub-configuration with members left unset",
            runtime: "FLINK-1_19",
            mode: None,
            configuration: json!({
                "FlinkApplicationConfiguration": {
                    "ParallelismConfiguration": {
                        "ConfigurationType": "CUSTOM",
                        "Parallelism": 8
                    }
                }
            }),
            checkpoint: default_checkpoint(),
            monitoring: default_monitoring(),
            parallelism: json!({
                "ConfigurationType": "CUSTOM",
                "Parallelism": 8,
                "ParallelismPerKPU": 1,
                "CurrentParallelism": 8,
                "AutoScalingEnabled": true
            }),
            snapshots_enabled: true,
        },
        ConfigurationCase {
            case: "snapshots turned off by the caller",
            runtime: "ZEPPELIN-FLINK-3_0",
            mode: Some("INTERACTIVE"),
            configuration: json!({"ApplicationSnapshotConfiguration": {"SnapshotsEnabled": false}}),
            checkpoint: interactive_default_checkpoint(),
            monitoring: default_monitoring(),
            parallelism: default_parallelism(),
            snapshots_enabled: false,
        },
    ];

    let router = router();
    for ConfigurationCase {
        case,
        runtime,
        mode,
        configuration,
        checkpoint,
        monitoring,
        parallelism,
        snapshots_enabled,
    } in cases
    {
        let name = format!("defaults-{}", case.replace(' ', "-"));
        let mut body = create_body(&name, runtime);
        if let Some(mode) = mode {
            body["ApplicationMode"] = json!(mode);
        }
        if !configuration.is_null() {
            body["ApplicationConfiguration"] = configuration;
        }
        let (status, created) = call(&router, "CreateApplication", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApplication got {status} {created}"
        );
        let detail = created["ApplicationDetail"].clone();

        let described = describe(&router, &name).await;
        assert_eq!(
            described, detail,
            "case {case:?}: expected describe to repeat the created detail, got {described}"
        );

        let config = &detail["ApplicationConfigurationDescription"];
        assert!(
            config.is_object(),
            "case {case:?}: expected an ApplicationConfigurationDescription, got {detail}"
        );
        let flink = &config["FlinkApplicationConfigurationDescription"];
        assert_members(
            case,
            "CheckpointConfigurationDescription",
            &flink["CheckpointConfigurationDescription"],
            &checkpoint,
        );
        assert_members(
            case,
            "MonitoringConfigurationDescription",
            &flink["MonitoringConfigurationDescription"],
            &monitoring,
        );
        assert_members(
            case,
            "ParallelismConfigurationDescription",
            &flink["ParallelismConfigurationDescription"],
            &parallelism,
        );
        assert_eq!(
            config["ApplicationSnapshotConfigurationDescription"]["SnapshotsEnabled"].as_bool(),
            Some(snapshots_enabled),
            "case {case:?}: expected the snapshot setting the application runs under, got {config}"
        );
        assert_eq!(
            config["ApplicationSystemRollbackConfigurationDescription"]["RollbackEnabled"]
                .as_bool(),
            Some(false),
            "case {case:?}: the service rolls an application back only when asked, got {config}"
        );
        assert_eq!(
            config["ApplicationEncryptionConfigurationDescription"]["KeyType"].as_str(),
            Some("AWS_OWNED_KEY"),
            "case {case:?}: an application with no customer key is encrypted under the \
             AWS-owned one, got {config}"
        );
    }
}

/// A caller's own encryption configuration is reported as given, rather than replaced by
/// the AWS-owned-key default.
#[tokio::test]
async fn a_supplied_encryption_configuration_is_reported_as_given() {
    let router = router();
    let name = "customer-key-app";
    let mut body = create_body(name, "FLINK-1_20");
    body["ApplicationConfiguration"] = json!({
        "ApplicationEncryptionConfiguration": {
            "KeyType": "CUSTOMER_MANAGED_KEY",
            "KeyId": "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555"
        }
    });
    let (status, created) = call(&router, "CreateApplication", &body).await;
    assert_eq!(status, 200, "CreateApplication got {status} {created}");

    let encryption = describe(&router, name).await["ApplicationConfigurationDescription"]
        ["ApplicationEncryptionConfigurationDescription"]
        .clone();
    assert_eq!(
        encryption["KeyType"].as_str(),
        Some("CUSTOMER_MANAGED_KEY"),
        "case customer-managed: expected the supplied key type, got {encryption}"
    );
    assert_eq!(
        encryption["KeyId"].as_str(),
        body["ApplicationConfiguration"]["ApplicationEncryptionConfiguration"]["KeyId"].as_str(),
        "case customer-managed: expected the supplied key id, got {encryption}"
    );
}

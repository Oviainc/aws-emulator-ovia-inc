// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523 service.
//!
//! The store is the single source of truth for every response this service computes:
//! `CreateApplication` writes an [`Application`] here, and `DescribeApplication`,
//! `ListApplications`, `StartApplication`, `StopApplication` and `DeleteApplication`
//! all read or mutate THIS struct — nothing is echoed from a request or canned.
//!
//! Beside the records themselves this module owns the two projections every reader
//! needs ([`Application::detail`], [`Application::summary`]) and the clock-owed status
//! settlement ([`ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State::settle`]),
//! so an operation handler never re-derives them.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    ApplicationCodeConfigurationDescription, ApplicationConfiguration,
    ApplicationConfigurationDescription, ApplicationDetail,
    ApplicationEncryptionConfigurationDescription, ApplicationSnapshotConfigurationDescription,
    ApplicationSummary, ApplicationSystemRollbackConfigurationDescription,
    CatalogConfigurationDescription, CheckpointConfigurationDescription, CodeContentDescription,
    CustomArtifactConfigurationDescription, DeployAsApplicationConfigurationDescription,
    EnvironmentPropertyDescriptions, FlinkApplicationConfigurationDescription,
    GlueDataCatalogConfigurationDescription, InputDescription,
    InputProcessingConfigurationDescription, InputLambdaProcessorDescription,
    KinesisFirehoseInputDescription, KinesisFirehoseOutputDescription,
    KinesisStreamsInputDescription, KinesisStreamsOutputDescription, LambdaOutputDescription,
    MonitoringConfigurationDescription, OutputDescription, ParallelismConfigurationDescription,
    ReferenceDataSourceDescription, S3ApplicationCodeLocationDescription,
    S3ContentBaseLocationDescription, S3ReferenceDataSourceDescription,
    SqlApplicationConfigurationDescription, VpcConfigurationDescription,
    ZeppelinApplicationConfigurationDescription, ZeppelinMonitoringConfigurationDescription,
};
use crate::ops::types::{CloudWatchLoggingOption, CloudWatchLoggingOptionDescription};

/// `ApplicationStatus` values this slice drives, spelled exactly as the model's
/// `ApplicationStatus` enum does (`smithy_query(query="shape",shape="ApplicationStatus")`).
pub const STATUS_READY: &str = "READY";
pub const STATUS_STARTING: &str = "STARTING";
pub const STATUS_RUNNING: &str = "RUNNING";
pub const STATUS_STOPPING: &str = "STOPPING";

/// `OperationStatus` values, from the model's `OperationStatus` enum.
pub const OPERATION_IN_PROGRESS: &str = "IN_PROGRESS";
pub const OPERATION_SUCCESSFUL: &str = "SUCCESSFUL";

/// How long a status transition the clock owes stays pending.
///
/// `StartApplication` and `StopApplication` are asynchronous: they answer while the
/// application is still `STARTING`/`STOPPING` and the terminal status becomes due
/// without any further call. AWS documents no duration for that settle, so this span is
/// CHOSEN — inside the emulator's 250–750 ms window — and is deliberately non-zero so a
/// caller that reads straight back still observes the transient status.
const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The status change the clock owes an application, and the operation record it completes.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The `ApplicationStatus` the application takes once `due_at` has passed.
    pub to_status: String,
    /// The `AppOperation` this transition completes.
    pub operation_id: String,
    /// When the transition becomes due, on the monotonic clock.
    pub due_at: Instant,
    /// The same instant on the wall clock, so the completion timestamps a reader sees
    /// are the instant the change was OWED and do not drift with when it is observed.
    pub due_wall: DateTime,
}

/// One asynchronous application operation (`StartApplication`, `StopApplication`, …).
///
/// The `OperationId` those operations return is the id of a record held HERE, so the
/// value handed to a client is backed by state rather than minted and forgotten.
#[derive(Debug, Clone)]
pub struct AppOperation {
    pub operation_id: String,
    /// The API operation that created this record, e.g. `"StartApplication"`.
    pub operation: String,
    /// An `OperationStatus` value: `IN_PROGRESS` | `SUCCESSFUL` | `FAILED` | `CANCELLED`.
    pub status: String,
    pub start_time: DateTime,
    pub end_time: Option<DateTime>,
}

/// One stored Managed Service for Apache Flink application.
#[derive(Debug, Clone)]
pub struct Application {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    /// A `RuntimeEnvironment` enum value, as supplied at create.
    pub runtime_environment: String,
    pub service_execution_role: String,
    /// An `ApplicationMode` enum value, when the creator supplied one.
    pub application_mode: Option<String>,
    /// An `ApplicationStatus` enum value.
    pub status: String,
    /// The status change the clock owes, while one is outstanding.
    pub pending: Option<PendingTransition>,
    pub version_id: i64,
    pub create_timestamp: DateTime,
    /// When the current application version was created; equal to `create_timestamp`
    /// until an operation that mints a new version lands.
    pub version_create_timestamp: DateTime,
    pub last_update_timestamp: Option<DateTime>,
    pub configuration: Option<ApplicationConfigurationDescription>,
    pub cloud_watch_logging_options: Vec<CloudWatchLoggingOptionDescription>,
    /// The optimistic-concurrency token for the current version.
    pub conditional_token: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub operations: Vec<AppOperation>,
}

impl Application {
    /// Apply the status change the clock owes, if it is due at `now`.
    ///
    /// This is the ONLY write a reporting operation may make: the change was owed at
    /// `due_at` whether or not anybody called, so materialising it here is not a
    /// read-driven mutation. Everything derived from it (the completion timestamps) is
    /// stamped at `due_wall`, so repeated reads report identical values.
    fn settle(&mut self, now: Instant) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if now < pending.due_at {
            return;
        }
        self.status = pending.to_status;
        self.last_update_timestamp = Some(pending.due_wall);
        if let Some(op) = self
            .operations
            .iter_mut()
            .find(|op| op.operation_id == pending.operation_id)
        {
            op.status = OPERATION_SUCCESSFUL.to_string();
            op.end_time = Some(pending.due_wall);
        }
        self.pending = None;
    }

    /// The `ApplicationDetail` `CreateApplication` and `DescribeApplication` return.
    pub fn detail(&self) -> ApplicationDetail {
        ApplicationDetail {
            application_arn: self.arn.clone(),
            application_description: self.description.clone(),
            application_name: self.name.clone(),
            runtime_environment: self.runtime_environment.clone(),
            service_execution_role: Some(self.service_execution_role.clone()),
            application_status: self.status.clone(),
            application_version_id: self.version_id,
            create_timestamp: Some(self.create_timestamp),
            last_update_timestamp: self.last_update_timestamp,
            application_configuration_description: self.configuration.clone(),
            cloud_watch_logging_option_descriptions: (!self.cloud_watch_logging_options.is_empty())
                .then(|| self.cloud_watch_logging_options.clone()),
            // UpdateApplicationMaintenanceConfiguration is still scaffolded, so no
            // maintenance window has ever been set on a stored application.
            application_maintenance_configuration_description: None,
            // Only an update or a rollback fills these; neither is in this slice.
            application_version_updated_from: None,
            application_version_rolled_back_from: None,
            application_version_create_timestamp: Some(self.version_create_timestamp),
            conditional_token: self.conditional_token.clone(),
            application_version_rolled_back_to: None,
            application_mode: self.application_mode.clone(),
        }
    }

    /// The `ApplicationSummary` `ListApplications` returns.
    pub fn summary(&self) -> ApplicationSummary {
        ApplicationSummary {
            application_name: self.name.clone(),
            application_arn: self.arn.clone(),
            application_status: self.status.clone(),
            application_version_id: self.version_id,
            runtime_environment: self.runtime_environment.clone(),
            application_mode: self.application_mode.clone(),
        }
    }
}

/// The ceiling on how many applications this store holds, and on how many operation
/// records one application retains: the two caps that bound caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process and a
/// create is the only thing that adds one, so the store needs a maximum entry count —
/// neither a TTL nor an eviction policy is available to it, because `DescribeApplication`
/// must still answer what `CreateApplication` created. `CreateApplication` refuses past
/// [`MAX_APPLICATIONS`] with the service's own modelled `LimitExceededException`
/// (HTTP 400, "The number of allowed resources has been exceeded"), which the model
/// declares on `CreateApplication` and on no read. The operation log, which no caller can
/// drain, is trimmed oldest-first at [`MAX_OPERATIONS_PER_APPLICATION`] instead: losing
/// the oldest completed `StartApplication`/`StopApplication` record is not a refusal a
/// real client would see, and `Start`/`Stop` declare no quota error.
///
/// Both numbers are the emulator's own bound, not a published AWS default: AWS publishes
/// the application-per-region quota as adjustable, so no single value is the contract,
/// and the caps are set high enough that no ordinary workflow meets one.
pub const MAX_APPLICATIONS: usize = 5_000;

/// See [`MAX_APPLICATIONS`].
pub const MAX_OPERATIONS_PER_APPLICATION: usize = 200;

/// Per-service state for com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523. A handler reaches it through [`service_state`].
///
/// `applications` is a `BTreeMap`, so `ListApplications` walks a stable, name-ordered
/// sequence and a pagination token can resume from a name rather than an index. Its entry
/// count is bounded by [`MAX_APPLICATIONS`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State {
    pub applications: BTreeMap<String, Application>,
    /// Bumped for every server-assigned id, so two ids minted in the same instant differ.
    id_sequence: u64,
}

impl ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State {
    /// Apply the status change the clock owes `name`, if there is one.
    ///
    /// Call this FIRST in a handler that names a single application, so the operation
    /// decides against the status that application has now. It touches only that entry,
    /// so the work a request does under the state lock does not grow with the store.
    pub fn settle_named(&mut self, name: &str) {
        let now = Instant::now();
        if let Some(app) = self.applications.get_mut(name) {
            app.settle(now);
        }
    }

    /// Apply every status change the clock owes. For `ListApplications`, which reports on
    /// every stored application at once; the walk is bounded by [`MAX_APPLICATIONS`].
    pub fn settle(&mut self) {
        let now = Instant::now();
        for app in self.applications.values_mut() {
            app.settle(now);
        }
    }

    /// A fresh server-assigned id in the 8-4-4-4-12 hex form AWS uses for the ids this
    /// API hands out (`OperationId`, `InputId`, `ConditionalToken`, …).
    ///
    /// Unique by construction: the digest covers a monotonically increasing sequence
    /// number, so no two ids from one store collide, and the form satisfies both `Id`
    /// (`^[a-zA-Z0-9_.-]+$`, 1..50) and `ConditionalToken` (`^[a-zA-Z0-9-_+/=]+$`, 1..512).
    pub fn mint_id(&mut self, kind: &str) -> String {
        self.id_sequence += 1;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        hex_id(&format!("{kind}:{}:{nanos}", self.id_sequence))
    }

    /// Record a new operation on `name` and return its `OperationId`.
    ///
    /// The id is the primary key of the stored [`AppOperation`], never a loose value. The
    /// log is capped at [`MAX_OPERATIONS_PER_APPLICATION`] and trimmed oldest-first, so
    /// repeated Start/Stop calls cannot grow one application's record without limit.
    pub fn record_operation(&mut self, name: &str, operation: &str, now: DateTime) -> String {
        let operation_id = self.mint_id(operation);
        if let Some(app) = self.applications.get_mut(name) {
            app.operations.push(AppOperation {
                operation_id: operation_id.clone(),
                operation: operation.to_string(),
                status: OPERATION_IN_PROGRESS.to_string(),
                start_time: now,
                end_time: None,
            });
            if app.operations.len() > MAX_OPERATIONS_PER_APPLICATION {
                let excess = app.operations.len() - MAX_OPERATIONS_PER_APPLICATION;
                app.operations.drain(..excess);
            }
        }
        operation_id
    }

    /// Move `name` into `transient_status` now and owe `settled_status` to the clock.
    pub fn begin_transition(
        &mut self,
        name: &str,
        transient_status: &str,
        settled_status: &str,
        operation_id: String,
        now: DateTime,
    ) {
        // The change becomes due `TRANSITION_DELAY` after this call; it is stamped as
        // the next whole second, the resolution this service reports timestamps at.
        let due_wall = DateTime::from_secs(now.secs() + 1);
        if let Some(app) = self.applications.get_mut(name) {
            app.status = transient_status.to_string();
            app.pending = Some(PendingTransition {
                to_status: settled_status.to_string(),
                operation_id,
                due_at: Instant::now() + TRANSITION_DELAY,
                due_wall,
            });
        }
    }

    /// The `ApplicationConfigurationDescription` for a configuration supplied to
    /// `CreateApplication`: every member the creator set, plus the ids and derived
    /// values the service assigns.
    pub fn describe_configuration(
        &mut self,
        cfg: &ApplicationConfiguration,
    ) -> ApplicationConfigurationDescription {
        let sql = cfg.sql_application_configuration.as_ref().map(|sql| {
            SqlApplicationConfigurationDescription {
                input_descriptions: sql.inputs.as_ref().map(|inputs| {
                    inputs
                        .iter()
                        .map(|input| InputDescription {
                            input_id: Some(self.mint_id("Input")),
                            name_prefix: Some(input.name_prefix.clone()),
                            in_app_stream_names: in_app_stream_names(
                                &input.name_prefix,
                                input
                                    .input_parallelism
                                    .as_ref()
                                    .and_then(|p| p.count)
                                    .unwrap_or(1),
                            ),
                            input_processing_configuration_description: input
                                .input_processing_configuration
                                .as_ref()
                                .map(|ipc| InputProcessingConfigurationDescription {
                                    input_lambda_processor_description: Some(
                                        InputLambdaProcessorDescription {
                                            resource_arn: ipc
                                                .input_lambda_processor
                                                .resource_arn
                                                .clone(),
                                            role_arn: None,
                                        },
                                    ),
                                }),
                            kinesis_streams_input_description: input
                                .kinesis_streams_input
                                .as_ref()
                                .map(|k| KinesisStreamsInputDescription {
                                    resource_arn: k.resource_arn.clone(),
                                    role_arn: None,
                                }),
                            kinesis_firehose_input_description: input
                                .kinesis_firehose_input
                                .as_ref()
                                .map(|k| KinesisFirehoseInputDescription {
                                    resource_arn: k.resource_arn.clone(),
                                    role_arn: None,
                                }),
                            input_schema: Some(input.input_schema.clone()),
                            input_parallelism: input.input_parallelism.clone(),
                            // Set by the RunConfiguration a StartApplication call carries.
                            input_starting_position_configuration: None,
                        })
                        .collect()
                }),
                output_descriptions: sql.outputs.as_ref().map(|outputs| {
                    outputs
                        .iter()
                        .map(|output| OutputDescription {
                            output_id: Some(self.mint_id("Output")),
                            name: Some(output.name.clone()),
                            kinesis_streams_output_description: output
                                .kinesis_streams_output
                                .as_ref()
                                .map(|k| KinesisStreamsOutputDescription {
                                    resource_arn: k.resource_arn.clone(),
                                    role_arn: None,
                                }),
                            kinesis_firehose_output_description: output
                                .kinesis_firehose_output
                                .as_ref()
                                .map(|k| KinesisFirehoseOutputDescription {
                                    resource_arn: k.resource_arn.clone(),
                                    role_arn: None,
                                }),
                            lambda_output_description: output.lambda_output.as_ref().map(|l| {
                                LambdaOutputDescription {
                                    resource_arn: l.resource_arn.clone(),
                                    role_arn: None,
                                }
                            }),
                            destination_schema: Some(output.destination_schema.clone()),
                        })
                        .collect()
                }),
                reference_data_source_descriptions: sql.reference_data_sources.as_ref().map(
                    |sources| {
                        sources
                            .iter()
                            // A reference data source is described through its S3
                            // location, whose `BucketARN` (`^arn:`) and `FileKey`
                            // (len 1..1024) the description REQUIRES. A source that
                            // supplied neither cannot be described, and empty strings
                            // would break both constraints, so it is left out rather
                            // than reported with placeholder members.
                            .filter_map(|source| {
                                let s3 = source.s3_reference_data_source.as_ref()?;
                                Some(ReferenceDataSourceDescription {
                                    reference_id: self.mint_id("Reference"),
                                    table_name: source.table_name.clone(),
                                    s3_reference_data_source_description:
                                        S3ReferenceDataSourceDescription {
                                            bucket_arn: s3.bucket_arn.clone()?,
                                            file_key: s3.file_key.clone()?,
                                            reference_role_arn: None,
                                        },
                                    reference_schema: Some(source.reference_schema.clone()),
                                })
                            })
                            .collect()
                    },
                ),
            }
        });

        let vpc_configuration_descriptions = cfg.vpc_configurations.as_ref().map(|vpcs| {
            vpcs.iter()
                .map(|vpc| VpcConfigurationDescription {
                    vpc_configuration_id: self.mint_id("VpcConfiguration"),
                    // The VPC a set of subnets belongs to. This emulator holds no VPC
                    // inventory, so the id is derived from the subnet set itself: the
                    // same subnets always report the same VPC, which is the invariant a
                    // caller can rely on.
                    vpc_id: vpc_id_for(&vpc.subnet_ids),
                    subnet_ids: vpc.subnet_ids.clone(),
                    security_group_ids: vpc.security_group_ids.clone(),
                })
                .collect()
        });

        ApplicationConfigurationDescription {
            sql_application_configuration_description: sql,
            application_code_configuration_description: cfg
                .application_code_configuration
                .as_ref()
                .map(|code| ApplicationCodeConfigurationDescription {
                    code_content_type: code.code_content_type.clone(),
                    code_content_description: code.code_content.as_ref().map(|content| {
                        CodeContentDescription {
                            text_content: content.text_content.clone(),
                            // AWS reports the code's MD5 digest. Inline code is in hand,
                            // but S3-hosted code is not, and this emulator has no MD5
                            // implementation to report either with, so the member is
                            // left absent rather than filled with another digest.
                            code_md5: None,
                            code_size: content
                                .text_content
                                .as_ref()
                                .map(|t| t.len() as i64)
                                .or_else(|| content.zip_file_content.as_ref().map(|z| z.len() as i64)),
                            s3_application_code_location_description: content
                                .s3_content_location
                                .as_ref()
                                .map(|s3| S3ApplicationCodeLocationDescription {
                                    bucket_arn: s3.bucket_arn.clone(),
                                    file_key: s3.file_key.clone(),
                                    object_version: s3.object_version.clone(),
                                }),
                        }
                    }),
                }),
            // Filled by StartApplication's RunConfiguration, not at create time.
            run_configuration_description: None,
            flink_application_configuration_description: cfg
                .flink_application_configuration
                .as_ref()
                .map(|flink| FlinkApplicationConfigurationDescription {
                    checkpoint_configuration_description: flink
                        .checkpoint_configuration
                        .as_ref()
                        .map(|c| CheckpointConfigurationDescription {
                            configuration_type: Some(c.configuration_type.clone()),
                            checkpointing_enabled: c.checkpointing_enabled,
                            checkpoint_interval: c.checkpoint_interval,
                            min_pause_between_checkpoints: c.min_pause_between_checkpoints,
                        }),
                    monitoring_configuration_description: flink
                        .monitoring_configuration
                        .as_ref()
                        .map(|m| MonitoringConfigurationDescription {
                            configuration_type: Some(m.configuration_type.clone()),
                            metrics_level: m.metrics_level.clone(),
                            log_level: m.log_level.clone(),
                        }),
                    parallelism_configuration_description: flink
                        .parallelism_configuration
                        .as_ref()
                        .map(|p| ParallelismConfigurationDescription {
                            configuration_type: Some(p.configuration_type.clone()),
                            parallelism: p.parallelism,
                            parallelism_per_kpu: p.parallelism_per_kpu,
                            // The parallelism in force right now: the configured value
                            // until autoscaling moves it, and autoscaling is not modelled.
                            current_parallelism: p.parallelism,
                            auto_scaling_enabled: p.auto_scaling_enabled,
                        }),
                    // The Flink job plan exists only once a job has been submitted to a
                    // running cluster; this emulator runs no Flink job.
                    job_plan_description: None,
                }),
            environment_property_descriptions: cfg.environment_properties.as_ref().map(|env| {
                EnvironmentPropertyDescriptions {
                    property_group_descriptions: Some(env.property_groups.clone()),
                }
            }),
            application_snapshot_configuration_description: cfg
                .application_snapshot_configuration
                .as_ref()
                .map(|s| ApplicationSnapshotConfigurationDescription {
                    snapshots_enabled: s.snapshots_enabled,
                }),
            application_system_rollback_configuration_description: cfg
                .application_system_rollback_configuration
                .as_ref()
                .map(|s| ApplicationSystemRollbackConfigurationDescription {
                    rollback_enabled: s.rollback_enabled,
                }),
            vpc_configuration_descriptions,
            zeppelin_application_configuration_description: cfg
                .zeppelin_application_configuration
                .as_ref()
                .map(|z| ZeppelinApplicationConfigurationDescription {
                    monitoring_configuration_description: ZeppelinMonitoringConfigurationDescription {
                        log_level: z.monitoring_configuration.as_ref().map(|m| m.log_level.clone()),
                    },
                    catalog_configuration_description: z.catalog_configuration.as_ref().map(|c| {
                        CatalogConfigurationDescription {
                            glue_data_catalog_configuration_description:
                                GlueDataCatalogConfigurationDescription {
                                    database_arn: c
                                        .glue_data_catalog_configuration
                                        .database_arn
                                        .clone(),
                                },
                        }
                    }),
                    deploy_as_application_configuration_description: z
                        .deploy_as_application_configuration
                        .as_ref()
                        .map(|d| DeployAsApplicationConfigurationDescription {
                            s3_content_location_description: S3ContentBaseLocationDescription {
                                bucket_arn: d.s3_content_location.bucket_arn.clone(),
                                base_path: d.s3_content_location.base_path.clone(),
                            },
                        }),
                    custom_artifacts_configuration_description: z
                        .custom_artifacts_configuration
                        .as_ref()
                        .map(|artifacts| {
                            artifacts
                                .iter()
                                .map(|a| CustomArtifactConfigurationDescription {
                                    artifact_type: Some(a.artifact_type.clone()),
                                    s3_content_location_description: a
                                        .s3_content_location
                                        .clone(),
                                    maven_reference_description: a.maven_reference.clone(),
                                })
                                .collect()
                        }),
                }),
            application_encryption_configuration_description: cfg
                .application_encryption_configuration
                .as_ref()
                .map(|e| ApplicationEncryptionConfigurationDescription {
                    key_id: e.key_id.clone(),
                    key_type: e.key_type.clone(),
                }),
        }
    }

    /// The stored descriptions for the CloudWatch log streams a create call attached.
    pub fn describe_logging_options(
        &mut self,
        options: &[CloudWatchLoggingOption],
    ) -> Vec<CloudWatchLoggingOptionDescription> {
        options
            .iter()
            .map(|option| CloudWatchLoggingOptionDescription {
                cloud_watch_logging_option_id: Some(self.mint_id("CloudWatchLoggingOption")),
                log_stream_arn: option.log_stream_arn.clone(),
                role_arn: None,
            })
            .collect()
    }
}

/// The current wall-clock instant as a modelled timestamp, truncated to whole seconds.
///
/// Whole seconds keep the awsJson1_1 encoding an integer, so a `CreateTimestamp` a
/// client reads back and passes to `DeleteApplication` is bit-identical to the stored
/// one rather than a float that lost precision in transit.
pub fn now_timestamp() -> DateTime {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    DateTime::from_secs(secs as i64)
}

/// The region every ARN this service mints carries: the one the caller signed for.
///
/// Falls back to the emulator's default region when the request carries no SigV4
/// credential scope to read it from.
pub fn region_from_headers(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request is answered in, matching the region the emulator
/// writes into the AWS config it hands clients (`vendor/server/src/lib.rs`).
const DEFAULT_REGION: &str = "us-east-1";

/// The ARN of an application: `arn:aws:kinesisanalytics:<region>:<account>:application/<name>`.
///
/// `kinesisanalytics` is this service's `arnNamespace`
/// (`smithy_query(query="service_info")`), and the account segment is the emulator's
/// canonical one, which [`::engine::emulator_arn`] owns.
pub fn application_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("kinesisanalytics", region, &format!("application/{name}"))
}

/// A digest-derived id in the 8-4-4-4-12 hex form.
fn hex_id(seed: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(seed.as_bytes());
    let hex = hex::encode(&digest[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The VPC id reported for a subnet set, derived from the subnets themselves so the
/// same subnets always report the same VPC.
fn vpc_id_for(subnet_ids: &[String]) -> String {
    let mut sorted: Vec<&str> = subnet_ids.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let digest = hex_id(&format!("vpc:{}", sorted.join(",")));
    format!("vpc-{}", &digest[0..8])
}

/// The in-app stream names an SQL input is exposed under: one per unit of input
/// parallelism, named `<NamePrefix>_NNN`.
///
/// `count` reaches here from the request's `InputParallelism$Count`, so it SIZES A LOOP:
/// it is clamped to [`MAX_INPUT_PARALLELISM`] before anything is allocated, so no
/// request-supplied integer can drive an unbounded allocation loop even if the boundary
/// range check that refuses it ever stops running.
///
/// `InAppStreamName` is bounded at 32 characters (`smithy_query(query="shape",
/// shape="InAppStreamName")`), so a prefix long enough to overflow that bound yields no
/// names at all rather than a value the model would reject.
fn in_app_stream_names(name_prefix: &str, count: i32) -> Option<Vec<String>> {
    let count = count.clamp(1, MAX_INPUT_PARALLELISM) as usize;
    let mut names: Vec<String> = Vec::with_capacity(count);
    names.extend(
        (1..=count)
            .map(|n| format!("{name_prefix}_{n:03}"))
            .filter(|name| name.len() <= 32),
    );
    (!names.is_empty()).then_some(names)
}

/// The largest `InputParallelism$Count` the model admits, from its `@range` of 1..64
/// (`smithy_query(query="shape",shape="InputParallelismCount")`). The boundary validator
/// already refuses a request outside that range; this is the defence-in-depth bound on
/// the loop the value sizes.
const MAX_INPUT_PARALLELISM: i32 = 64;

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State>)
}

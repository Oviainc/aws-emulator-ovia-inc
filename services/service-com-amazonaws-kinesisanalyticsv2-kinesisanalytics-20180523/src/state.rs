// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523 service.
//!
//! The store is the single source of truth for every response this service computes:
//! `CreateApplication` writes an [`Application`] here, and `DescribeApplication`,
//! `ListApplications`, `StartApplication`, `StopApplication` and `DeleteApplication`
//! all read or mutate THIS struct — nothing is echoed from a request or canned.
//! `CreateApplicationSnapshot` writes a [`Snapshot`] onto the application it names and
//! `DescribeApplicationSnapshot` reads it back from there.
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
    GlueDataCatalogConfigurationDescription, Input, InputDescription,
    InputProcessingConfigurationDescription, InputLambdaProcessorDescription,
    KinesisFirehoseInputDescription, KinesisFirehoseOutputDescription,
    KinesisStreamsInputDescription, KinesisStreamsOutputDescription, LambdaOutputDescription,
    MonitoringConfigurationDescription, OutputDescription, ParallelismConfigurationDescription,
    ReferenceDataSourceDescription, S3ApplicationCodeLocationDescription,
    S3ContentBaseLocationDescription, S3ReferenceDataSourceDescription, SnapshotDetails,
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

/// `SnapshotStatus` values this slice drives, spelled exactly as the model's
/// `SnapshotStatus` enum does (`smithy_query(query="shape",shape="SnapshotStatus")`).
pub const SNAPSHOT_CREATING: &str = "CREATING";
pub const SNAPSHOT_READY: &str = "READY";

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

/// One stored snapshot of an application's state.
///
/// `CreateApplicationSnapshot` writes one of these and `DescribeApplicationSnapshot`
/// reports it, so the `SnapshotDetails` a reader sees is the record the writer stored.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub name: String,
    /// A `SnapshotStatus` enum value.
    pub status: String,
    /// The application version the snapshot was taken of.
    pub application_version_id: i64,
    pub creation_timestamp: DateTime,
    /// The application's `RuntimeEnvironment` when the snapshot was taken.
    pub runtime_environment: String,
    /// When `CREATING` becomes `READY` on the monotonic clock, while that change is
    /// outstanding.
    pub ready_at: Option<Instant>,
}

impl Snapshot {
    /// Apply the status change the clock owes this snapshot, if it is due at `now`.
    ///
    /// The same rule as [`Application::settle`]: the change was owed at `ready_at`
    /// whether or not anybody called, so materialising it on a read is not a read-driven
    /// mutation.
    fn settle(&mut self, now: Instant) {
        if self.ready_at.is_some_and(|due| now >= due) {
            self.status = SNAPSHOT_READY.to_string();
            self.ready_at = None;
        }
    }

    /// The `SnapshotDetails` `DescribeApplicationSnapshot` and `ListApplicationSnapshots`
    /// report.
    pub fn details(&self) -> SnapshotDetails {
        SnapshotDetails {
            snapshot_name: self.name.clone(),
            snapshot_status: self.status.clone(),
            application_version_id: self.application_version_id,
            snapshot_creation_timestamp: Some(self.creation_timestamp),
            runtime_environment: Some(self.runtime_environment.clone()),
            // The emulator stores no customer-managed key for an application, so a
            // snapshot carries no encryption description of its own.
            application_encryption_configuration_description: None,
        }
    }
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
    /// The version this application was on before the latest configuration update, while
    /// one has landed: `ApplicationVersionUpdatedFrom`.
    pub version_updated_from: Option<i64>,
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
    /// The snapshots taken of this application, by `SnapshotName`, so a reader walks a
    /// stable name-ordered sequence. Bounded by [`MAX_SNAPSHOTS_PER_APPLICATION`].
    pub snapshots: BTreeMap<String, Snapshot>,
}

impl Application {
    /// Apply the status change the clock owes, if it is due at `now`.
    ///
    /// This is the ONLY write a reporting operation may make: the change was owed at
    /// `due_at` whether or not anybody called, so materialising it here is not a
    /// read-driven mutation. Everything derived from it (the completion timestamps) is
    /// stamped at `due_wall`, so repeated reads report identical values.
    fn settle(&mut self, now: Instant) {
        for snapshot in self.snapshots.values_mut() {
            snapshot.settle(now);
        }
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

    /// Whether this application's stored configuration carries the code its runtime would
    /// run: `ApplicationCodeConfigurationDescription.CodeContentDescription` naming
    /// non-empty inline text, an S3 object, or a non-empty zip the creator uploaded (which
    /// the description reports as a code size, since AWS does not echo the bytes back).
    ///
    /// Read off the STORED description rather than off a request, so what decides is the
    /// configuration a create or an update actually persisted. An EMPTY text content or an
    /// empty zip is no more code than an absent one, so neither counts.
    pub fn has_code_content(&self) -> bool {
        self.configuration
            .as_ref()
            .and_then(|cfg| cfg.application_code_configuration_description.as_ref())
            .and_then(|code| code.code_content_description.as_ref())
            .is_some_and(|content| {
                content
                    .text_content
                    .as_deref()
                    .is_some_and(|text| !text.is_empty())
                    || content.s3_application_code_location_description.is_some()
                    || content.code_size.is_some_and(|size| size > 0)
            })
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
            // The version an accepted configuration update moved this application on
            // from, as [`ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State::advance_version`]
            // recorded it.
            application_version_updated_from: self.version_updated_from,
            // Only a rollback fills this, and RollbackApplication is not in this slice.
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

/// The ceiling on how many snapshots one application retains. Same reasoning as
/// [`MAX_APPLICATIONS`]: a snapshot is held for the life of the process and only a caller
/// adds one, so the count needs a maximum — and `CreateApplicationSnapshot` declares the
/// modelled `LimitExceededException` (HTTP 400) for a caller that reaches it, so the
/// refusal is the service's own rather than a trim a reader could not explain.
pub const MAX_SNAPSHOTS_PER_APPLICATION: usize = 1_000;

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

    /// Take a snapshot of `application`'s current version: stored `CREATING`, with `READY`
    /// owed to the clock [`TRANSITION_DELAY`] from now, so a caller reading straight back
    /// still sees the transient status.
    ///
    /// The span is this crate's ONE clock-owed span — the same one `StartApplication` and
    /// `StopApplication` already settle on — rather than a second one of its own, so the
    /// emulator has a single answer to "how long does a transition this service starts stay
    /// pending", and the wire fixtures that poll one status poll them all the same way.
    ///
    /// The version and the runtime recorded are the application's as it stands now, so the
    /// `SnapshotDetails` a reader gets is about the application that was snapshotted and
    /// not about whatever it is later updated to.
    pub fn begin_snapshot(&mut self, application: &str, snapshot_name: &str, now: DateTime) {
        if let Some(app) = self.applications.get_mut(application) {
            app.snapshots.insert(
                snapshot_name.to_string(),
                Snapshot {
                    name: snapshot_name.to_string(),
                    status: SNAPSHOT_CREATING.to_string(),
                    application_version_id: app.version_id,
                    creation_timestamp: now,
                    runtime_environment: app.runtime_environment.clone(),
                    ready_at: Some(Instant::now() + TRANSITION_DELAY),
                },
            );
        }
    }

    /// Record a new version of `application` and return it.
    ///
    /// Every accepted configuration update results in a new version of the application
    /// (the `AddApplicationInput` doc says so for adding a streaming source, and
    /// `ApplicationDetail$ApplicationVersionId` states it for the API as a whole), so the
    /// version the caller's next optimistic-concurrency token must carry advances by one,
    /// the version it moved on from is remembered as `ApplicationVersionUpdatedFrom`, and
    /// the `ConditionalToken` — which names the CURRENT version — is minted afresh.
    ///
    /// Answers `None` when no application is stored under `application`, so a caller
    /// cannot read a version that was never recorded.
    pub fn advance_version(&mut self, application: &str, now: DateTime) -> Option<i64> {
        let conditional_token = self.mint_id("ConditionalToken");
        let app = self.applications.get_mut(application)?;
        app.version_updated_from = Some(app.version_id);
        app.version_id += 1;
        app.version_create_timestamp = now;
        app.last_update_timestamp = Some(now);
        app.conditional_token = Some(conditional_token);
        Some(app.version_id)
    }

    /// Store `input` as `application`'s streaming source and move the application on to a
    /// new version; answers that version and the input descriptions it now carries.
    ///
    /// The whole accepted mutation of `AddApplicationInput`, in one place beside the other
    /// writers of this store: the description (with the id and in-app stream names the
    /// service assigns), the version advance, and the answer read back out of what was
    /// stored. `None` when no application is stored under `application`.
    pub fn add_input(
        &mut self,
        application: &str,
        input: &Input,
        now: DateTime,
    ) -> Option<(i64, Vec<InputDescription>)> {
        let description = self.describe_input(input);
        let version_id = self.advance_version(application, now)?;
        let app = self.applications.get_mut(application)?;
        let descriptions = app
            .configuration
            .get_or_insert_with(ApplicationConfigurationDescription::default)
            .sql_application_configuration_description
            .get_or_insert_with(SqlApplicationConfigurationDescription::default)
            .input_descriptions
            .get_or_insert_with(Vec::new);
        descriptions.push(description);
        Some((version_id, descriptions.clone()))
    }

    /// The `ApplicationConfigurationDescription` an application RUNS under: every member
    /// the creator set, plus the ids and derived values the service assigns, completed
    /// with the service defaults for everything the request left out
    /// ([`complete_with_service_defaults`]).
    ///
    /// `cfg` is `None` when the request carried no `ApplicationConfiguration` at all,
    /// which is not an application with no configuration: the service still runs it under
    /// its defaults, so the description is materialised either way.
    pub fn describe_configuration(
        &mut self,
        cfg: Option<&ApplicationConfiguration>,
        runtime_environment: &str,
    ) -> ApplicationConfigurationDescription {
        let mut description = match cfg {
            Some(cfg) => self.describe_supplied_configuration(cfg),
            None => ApplicationConfigurationDescription::default(),
        };
        complete_with_service_defaults(&mut description, runtime_environment);
        description
    }

    /// The description of one streaming source, with the id and the in-app stream names
    /// the service assigns it.
    ///
    /// Shared by `CreateApplication`'s `SqlApplicationConfiguration` and by
    /// `AddApplicationInput`, so an input added after the fact is described exactly like
    /// one supplied at create time.
    pub fn describe_input(&mut self, input: &Input) -> InputDescription {
        InputDescription {
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
                    input_lambda_processor_description: Some(InputLambdaProcessorDescription {
                        resource_arn: ipc.input_lambda_processor.resource_arn.clone(),
                        role_arn: None,
                    }),
                }),
            kinesis_streams_input_description: input.kinesis_streams_input.as_ref().map(|k| {
                KinesisStreamsInputDescription {
                    resource_arn: k.resource_arn.clone(),
                    role_arn: None,
                }
            }),
            kinesis_firehose_input_description: input.kinesis_firehose_input.as_ref().map(|k| {
                KinesisFirehoseInputDescription {
                    resource_arn: k.resource_arn.clone(),
                    role_arn: None,
                }
            }),
            input_schema: Some(input.input_schema.clone()),
            input_parallelism: input.input_parallelism.clone(),
            // Set by the RunConfiguration a StartApplication call carries.
            input_starting_position_configuration: None,
        }
    }

    /// The description of the members a request actually supplied, before the service
    /// defaults complete it.
    fn describe_supplied_configuration(
        &mut self,
        cfg: &ApplicationConfiguration,
    ) -> ApplicationConfigurationDescription {
        let sql = cfg.sql_application_configuration.as_ref().map(|sql| {
            SqlApplicationConfigurationDescription {
                input_descriptions: sql.inputs.as_ref().map(|inputs| {
                    inputs
                        .iter()
                        .map(|input| self.describe_input(input))
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

/// The two `ConfigurationType` values, spelled as the model's enum spells them
/// (`smithy_query(query="shape",shape="ConfigurationType")`).
const CONFIGURATION_DEFAULT: &str = "DEFAULT";
const CONFIGURATION_CUSTOM: &str = "CUSTOM";

/// The checkpointing values a `DEFAULT` checkpoint configuration runs with, from the
/// `CheckpointConfiguration$ConfigurationType` doc: "If this value is set to `DEFAULT`,
/// the application will use the following values, even if they are set to other values
/// using APIs or application code: CheckpointingEnabled true, CheckpointInterval 60000,
/// MinPauseBetweenCheckpoints 5000".
const DEFAULT_CHECKPOINTING_ENABLED: bool = true;
/// See [`DEFAULT_CHECKPOINTING_ENABLED`]; milliseconds, as the member is documented.
const DEFAULT_CHECKPOINT_INTERVAL: i64 = 60_000;
/// See [`DEFAULT_CHECKPOINTING_ENABLED`]; milliseconds, as the member is documented.
const DEFAULT_MIN_PAUSE_BETWEEN_CHECKPOINTS: i64 = 5_000;

/// The CloudWatch logging a `DEFAULT` monitoring configuration runs with: the service's
/// own published defaults for the two members `CUSTOM` is required to set
/// (`MonitoringConfiguration$ConfigurationType`), each a value of its modelled enum
/// (`smithy_query(query="shape",shape="MetricsLevel")` / `shape="LogLevel"`).
const DEFAULT_METRICS_LEVEL: &str = "APPLICATION";
/// See [`DEFAULT_METRICS_LEVEL`].
const DEFAULT_LOG_LEVEL: &str = "INFO";

/// The parallelism a `DEFAULT` parallelism configuration runs with: the service's own
/// published defaults for the three members `CUSTOM` is required to set
/// (`ParallelismConfiguration$ConfigurationType`) — one parallel task, one task per KPU,
/// and the service free to scale the application out in response to load.
const DEFAULT_PARALLELISM: i32 = 1;
/// See [`DEFAULT_PARALLELISM`].
const DEFAULT_PARALLELISM_PER_KPU: i32 = 1;
/// See [`DEFAULT_PARALLELISM`].
const DEFAULT_AUTO_SCALING_ENABLED: bool = true;

/// Snapshots are taken for an application whose creator said nothing about them: the
/// service's default for `ApplicationSnapshotConfiguration$SnapshotsEnabled`.
const DEFAULT_SNAPSHOTS_ENABLED: bool = true;

/// The service does not roll an application back on its own unless its creator asked for
/// it: the default for `ApplicationSystemRollbackConfiguration$RollbackEnabled`.
const DEFAULT_ROLLBACK_ENABLED: bool = false;

/// An application whose creator named no KMS key is encrypted at rest under the key AWS
/// owns, which is the `KeyType` value the model declares for it
/// (`smithy_query(query="shape",shape="KeyType")`). No key id is reported with it,
/// because the caller has no key of their own to name.
const DEFAULT_KEY_TYPE: &str = "AWS_OWNED_KEY";

/// Complete `description` with the configuration the service runs an application under
/// when the request said nothing about it.
///
/// The rule: an application is always RUNNING under a full configuration, so every
/// sub-configuration that applies to its runtime is described — with `ConfigurationType`
/// `DEFAULT` and the documented default values where the request supplied none — and a
/// sub-configuration the request marked `DEFAULT` is described with those same defaults
/// whatever else it carried, which is what `ConfigurationType` `DEFAULT` MEANS. Only a
/// `CUSTOM` sub-configuration reports the caller's values, and the members it left unset
/// still report the defaults the service applies to them.
fn complete_with_service_defaults(
    description: &mut ApplicationConfigurationDescription,
    runtime_environment: &str,
) {
    // Every runtime family this API still creates applications for runs on Apache Flink
    // (`FLINK-` directly, `ZEPPELIN-FLINK-` as a Studio notebook), so all of them run
    // under a Flink configuration. A SQL application does not: its processing is
    // configured by its SQL configuration, so it is described with whatever Flink
    // configuration it was given and nothing is invented for it.
    if !is_sql_runtime(runtime_environment)
        || description
            .flink_application_configuration_description
            .is_some()
    {
        let flink = description
            .flink_application_configuration_description
            .get_or_insert_with(FlinkApplicationConfigurationDescription::default);
        complete_checkpoint_configuration(
            flink
                .checkpoint_configuration_description
                .get_or_insert_with(CheckpointConfigurationDescription::default),
        );
        complete_monitoring_configuration(
            flink
                .monitoring_configuration_description
                .get_or_insert_with(MonitoringConfigurationDescription::default),
        );
        complete_parallelism_configuration(
            flink
                .parallelism_configuration_description
                .get_or_insert_with(ParallelismConfigurationDescription::default),
        );
    }

    if description
        .application_snapshot_configuration_description
        .is_none()
    {
        description.application_snapshot_configuration_description =
            Some(ApplicationSnapshotConfigurationDescription {
                snapshots_enabled: DEFAULT_SNAPSHOTS_ENABLED,
            });
    }
    if description
        .application_system_rollback_configuration_description
        .is_none()
    {
        description.application_system_rollback_configuration_description =
            Some(ApplicationSystemRollbackConfigurationDescription {
                rollback_enabled: DEFAULT_ROLLBACK_ENABLED,
            });
    }
    if description
        .application_encryption_configuration_description
        .is_none()
    {
        description.application_encryption_configuration_description =
            Some(ApplicationEncryptionConfigurationDescription {
                key_id: None,
                key_type: DEFAULT_KEY_TYPE.to_string(),
            });
    }
}

/// Whether a sub-configuration described with `configuration_type` is the caller's own:
/// `CUSTOM` is, and anything else — `DEFAULT`, or no type at all because the request
/// supplied no such sub-configuration — runs on the service's defaults.
fn is_custom(configuration_type: Option<&String>) -> bool {
    configuration_type.map(String::as_str) == Some(CONFIGURATION_CUSTOM)
}

/// See [`complete_with_service_defaults`].
fn complete_checkpoint_configuration(description: &mut CheckpointConfigurationDescription) {
    if !is_custom(description.configuration_type.as_ref()) {
        *description = CheckpointConfigurationDescription {
            configuration_type: Some(CONFIGURATION_DEFAULT.to_string()),
            checkpointing_enabled: Some(DEFAULT_CHECKPOINTING_ENABLED),
            checkpoint_interval: Some(DEFAULT_CHECKPOINT_INTERVAL),
            min_pause_between_checkpoints: Some(DEFAULT_MIN_PAUSE_BETWEEN_CHECKPOINTS),
        };
        return;
    }
    description.checkpointing_enabled = description
        .checkpointing_enabled
        .or(Some(DEFAULT_CHECKPOINTING_ENABLED));
    description.checkpoint_interval = description
        .checkpoint_interval
        .or(Some(DEFAULT_CHECKPOINT_INTERVAL));
    description.min_pause_between_checkpoints = description
        .min_pause_between_checkpoints
        .or(Some(DEFAULT_MIN_PAUSE_BETWEEN_CHECKPOINTS));
}

/// See [`complete_with_service_defaults`].
fn complete_monitoring_configuration(description: &mut MonitoringConfigurationDescription) {
    if !is_custom(description.configuration_type.as_ref()) {
        *description = MonitoringConfigurationDescription {
            configuration_type: Some(CONFIGURATION_DEFAULT.to_string()),
            metrics_level: Some(DEFAULT_METRICS_LEVEL.to_string()),
            log_level: Some(DEFAULT_LOG_LEVEL.to_string()),
        };
        return;
    }
    description.metrics_level = description
        .metrics_level
        .take()
        .or_else(|| Some(DEFAULT_METRICS_LEVEL.to_string()));
    description.log_level = description
        .log_level
        .take()
        .or_else(|| Some(DEFAULT_LOG_LEVEL.to_string()));
}

/// See [`complete_with_service_defaults`]. `CurrentParallelism` is the parallelism in
/// force right now, which is the configured one until autoscaling moves it, and
/// autoscaling is not modelled.
fn complete_parallelism_configuration(description: &mut ParallelismConfigurationDescription) {
    if !is_custom(description.configuration_type.as_ref()) {
        *description = ParallelismConfigurationDescription {
            configuration_type: Some(CONFIGURATION_DEFAULT.to_string()),
            parallelism: Some(DEFAULT_PARALLELISM),
            parallelism_per_kpu: Some(DEFAULT_PARALLELISM_PER_KPU),
            current_parallelism: Some(DEFAULT_PARALLELISM),
            auto_scaling_enabled: Some(DEFAULT_AUTO_SCALING_ENABLED),
        };
        return;
    }
    description.parallelism = description.parallelism.or(Some(DEFAULT_PARALLELISM));
    description.parallelism_per_kpu = description
        .parallelism_per_kpu
        .or(Some(DEFAULT_PARALLELISM_PER_KPU));
    description.current_parallelism = description.current_parallelism.or(description.parallelism);
    description.auto_scaling_enabled = description
        .auto_scaling_enabled
        .or(Some(DEFAULT_AUTO_SCALING_ENABLED));
}

/// The `RuntimeEnvironment` family that names a SQL application, as the enum spells its
/// members (`smithy_query(query="shape",shape="RuntimeEnvironment")`: the SQL family is
/// `SQL-<major>_<minor>`, today `SQL-1_0`, beside the `FLINK-` and `ZEPPELIN-FLINK-`
/// families).
const SQL_RUNTIME_FAMILY: &str = "SQL-";

/// Whether `runtime` names a SQL application's runtime — any version of that family, not
/// one spelled-out value, so a later enum member of the same family is read the same way.
///
/// The value reaching here is either one the enum admits (the boundary validator refuses
/// anything else on the way in) or one read back off a stored application, and the enum's
/// members are the exact strings a client sends, so the comparison is on those bytes.
pub fn is_sql_runtime(runtime: &str) -> bool {
    runtime.starts_with(SQL_RUNTIME_FAMILY)
}

/// How this service words a request whose `CurrentApplicationVersionId` is not the
/// version the application is on now.
///
/// One wording for every operation that carries that optimistic-concurrency token, so
/// two operations cannot report the same stale token differently. The sentence is entirely
/// fixed text for this error class: naming the version the caller sent, or the one the
/// application is on, would hand a client a way to guess its way past the check.
pub const STALE_VERSION_MESSAGE: &str =
    "The provided application version ID does not match the most recent version for the \
     application.";

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

/// How this service words a lookup miss on an application: the name the caller asked for,
/// quoted, and the fact that no such application exists.
///
/// One wording for every operation that looks an application up by name, so two readers
/// of the same absent application cannot report it differently. The error CODE stays each
/// operation's own — the model does not declare the same error on all of them, and
/// `CreateApplicationSnapshot` reports the miss as `InvalidArgumentException` with its own
/// wording — but the sentence a `ResourceNotFoundException` carries is this one.
pub fn missing_application_message(name: &str) -> String {
    format!("Application '{name}' does not exist.")
}

/// How this service words a create refused because an application of that name is already
/// stored: the SAME sentence shape a lookup miss uses
/// ([`missing_application_message`]) — the resource the request named, quoted, then what
/// is true of it, as a sentence ending in a period.
pub fn existing_application_message(name: &str) -> String {
    format!("Application '{name}' already exists.")
}

/// How this service words a sentence about one application's SNAPSHOT: the snapshot the
/// request named is quoted, because it is what the sentence is about, and the application
/// it belongs to is named plainly as the scope it was looked up in.
///
/// `fact` is what is true of that snapshot (`does not exist`, `already exists`), so every
/// operation that reports on a snapshot by name states it the same way.
fn snapshot_message(snapshot: &str, fact: &str, application: &str) -> String {
    format!("Snapshot '{snapshot}' {fact} for application {application}.")
}

/// A snapshot the application holds no record of, for the operations that read one by name.
pub fn missing_snapshot_message(snapshot: &str, application: &str) -> String {
    snapshot_message(snapshot, "does not exist", application)
}

/// A snapshot name the application already holds a snapshot under, for the operations that
/// create one.
pub fn existing_snapshot_message(snapshot: &str, application: &str) -> String {
    snapshot_message(snapshot, "already exists", application)
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

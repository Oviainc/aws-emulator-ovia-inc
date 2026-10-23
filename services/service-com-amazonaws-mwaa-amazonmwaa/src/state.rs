// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mwaa-amazonmwaa service.
//!
//! The service's primary (and only) resource is the Amazon MWAA **environment**. Every
//! implemented operation computes its answer from the records below: nothing is canned,
//! and nothing is echoed straight back out of a request without being stored first.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Environment, LastUpdate, LoggingConfiguration, LoggingConfigurationInput,
    ModuleLoggingConfiguration, ModuleLoggingConfigurationInput, NetworkConfiguration,
};

/// The sigv4 signing name, endpoint prefix and ARN namespace of this service, from the
/// model's `aws.auth#sigv4` / `aws.api#service` traits (all three are `airflow`).
pub const ARN_SERVICE: &str = "airflow";

/// Region used when a request carries no SigV4 credential scope to read one from (an
/// unsigned request still routes by endpoint host).
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a new environment stays `CREATING`, and an updated one `UPDATING`, before
/// the clock owes it `AVAILABLE`.
///
/// `CREATING` and `UPDATING` are non-terminal `EnvironmentStatus` values, so the move to
/// `AVAILABLE` is a transition the clock owes. The span is the emulator's own choice (no
/// AWS-documented duration applies to MWAA provisioning) and is deliberately non-zero, so
/// a caller reading straight after the call still observes the transient status as it
/// does against the real service. The instant it comes due is stored on the record
/// ([`PendingTransition::due_at`]) and materialised by
/// [`ComAmazonawsMwaaAmazonmwaaState::settle`]; nothing sleeps.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// `MaxResults` upper bound on `ListEnvironments`, from its `@range(1, 25)`. Also the
/// page size used when a caller names none, so one unparameterised request cannot
/// materialise a response that grows with the stored environment count.
pub const MAX_RESULTS_LIMIT: i32 = 25;

/// How many environments this emulator will hold.
///
/// The real service caps environments per account per Region with a service quota. The
/// number here is NOT a fidelity claim about the published quota: it is the emulator's
/// own ceiling, set generously so no realistic workflow meets it, so that a caller
/// looping on create cannot grow this process's memory without bound.
pub const MAX_ENVIRONMENTS: usize = 1_000;

/// How many tags one environment can carry, from `TagMap`'s own `@length(1, 50)`.
pub const MAX_TAGS: usize = 50;

/// `EnvironmentStatus` while the environment is being created — "The request to create
/// the environment is in progress." (the `Status` member's own doc).
pub const STATUS_CREATING: &str = "CREATING";

/// `EnvironmentStatus` once the environment is ready — "The request was successful and
/// the environment is ready to use."
pub const STATUS_AVAILABLE: &str = "AVAILABLE";

/// `EnvironmentStatus` while an update is in progress — "The request to update the
/// environment is in progress."
pub const STATUS_UPDATING: &str = "UPDATING";

/// `UpdateStatus` while the update the clock owes has not landed yet.
pub const UPDATE_STATUS_PENDING: &str = "PENDING";

/// `UpdateStatus` once the update has landed.
pub const UPDATE_STATUS_SUCCESS: &str = "SUCCESS";

/// `LastUpdate.Source` for a change a caller asked for through `UpdateEnvironment`. The
/// member's doc: "The source of the last update to the environment. Includes internal
/// processes by Amazon MWAA, such as an environment maintenance update." — so the source
/// names what drove the change, which here is the API call itself.
pub const UPDATE_SOURCE_API: &str = "UpdateEnvironment";

/// The Apache Airflow version a create that names none gets. The `AirflowVersion` doc:
/// "If no value is specified, it defaults to the latest version. … Valid values: 2.7.2,
/// 2.8.1, 2.9.2, 2.10.1, 2.10.3, 2.11.0, and 3.0.6." — the last of which is the latest.
pub const LATEST_AIRFLOW_VERSION: &str = "3.0.6";

/// The one environment class whose default scheduler and web-server counts are 1 rather
/// than 2; see [`default_unit_count`].
pub const MICRO_ENVIRONMENT_CLASS: &str = "mw1.micro";

/// CloudWatch log-group suffix of each Apache Airflow log type, as MWAA names the groups
/// it publishes to (`airflow-<EnvironmentName>-<LogType>`).
const LOG_TYPE_DAG_PROCESSING: &str = "DAGProcessing";
/// Scheduler log type — see [`LOG_TYPE_DAG_PROCESSING`].
const LOG_TYPE_SCHEDULER: &str = "Scheduler";
/// Task log type — see [`LOG_TYPE_DAG_PROCESSING`].
const LOG_TYPE_TASK: &str = "Task";
/// Web server log type — see [`LOG_TYPE_DAG_PROCESSING`].
const LOG_TYPE_WEBSERVER: &str = "WebServer";
/// Worker log type — see [`LOG_TYPE_DAG_PROCESSING`].
const LOG_TYPE_WORKER: &str = "Worker";

/// The number of schedulers (and of minimum / maximum web servers) an environment runs
/// when the create names none.
///
/// The `Schedulers`, `MinWebservers` and `MaxWebservers` docs all state the same rule:
/// "Defaults to 2 for all environment sizes except mw1.micro, which defaults to 1."
pub fn default_unit_count(environment_class: Option<&str>) -> i32 {
    if environment_class == Some(MICRO_ENVIRONMENT_CLASS) {
        1
    } else {
        2
    }
}

/// One Apache Airflow log type's logging settings, as a create or update supplied them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleLoggingRecord {
    pub enabled: bool,
    pub log_level: String,
}

impl ModuleLoggingRecord {
    /// The record for one `ModuleLoggingConfigurationInput`; both its members are
    /// `@required`, so neither is optional here.
    fn from_input(input: &ModuleLoggingConfigurationInput) -> Self {
        Self {
            enabled: input.enabled,
            log_level: input.log_level.clone(),
        }
    }

    /// The modelled `ModuleLoggingConfiguration` for this log type.
    ///
    /// `CloudWatchLogGroupArn` is reported only for an ENABLED log type: MWAA publishes a
    /// log group for a log type it was asked to publish, and naming a group for a
    /// disabled type would report a resource nothing created.
    fn to_modelled(&self, region: &str, name: &str, log_type: &str) -> ModuleLoggingConfiguration {
        ModuleLoggingConfiguration {
            enabled: Some(self.enabled),
            log_level: Some(self.log_level.clone()),
            cloud_watch_log_group_arn: self
                .enabled
                .then(|| log_group_arn(region, name, log_type)),
        }
    }
}

/// The five Apache Airflow log types' settings, as a create or update left them. A log
/// type nobody configured stays `None` rather than being invented with a made-up level.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoggingConfigurationRecord {
    pub dag_processing_logs: Option<ModuleLoggingRecord>,
    pub scheduler_logs: Option<ModuleLoggingRecord>,
    pub task_logs: Option<ModuleLoggingRecord>,
    pub webserver_logs: Option<ModuleLoggingRecord>,
    pub worker_logs: Option<ModuleLoggingRecord>,
}

impl LoggingConfigurationRecord {
    /// Overwrite only the log types `input` names, leaving the rest as stored — the same
    /// "merge what the request carries" rule `UpdateEnvironment` follows for every other
    /// member.
    pub fn merge(&mut self, input: &LoggingConfigurationInput) {
        if let Some(module) = input.dag_processing_logs.as_ref() {
            self.dag_processing_logs = Some(ModuleLoggingRecord::from_input(module));
        }
        if let Some(module) = input.scheduler_logs.as_ref() {
            self.scheduler_logs = Some(ModuleLoggingRecord::from_input(module));
        }
        if let Some(module) = input.task_logs.as_ref() {
            self.task_logs = Some(ModuleLoggingRecord::from_input(module));
        }
        if let Some(module) = input.webserver_logs.as_ref() {
            self.webserver_logs = Some(ModuleLoggingRecord::from_input(module));
        }
        if let Some(module) = input.worker_logs.as_ref() {
            self.worker_logs = Some(ModuleLoggingRecord::from_input(module));
        }
    }

    /// True when no log type has been configured at all.
    fn is_empty(&self) -> bool {
        self.dag_processing_logs.is_none()
            && self.scheduler_logs.is_none()
            && self.task_logs.is_none()
            && self.webserver_logs.is_none()
            && self.worker_logs.is_none()
    }

    /// The modelled `LoggingConfiguration`, or `None` when nothing was configured.
    fn to_modelled(&self, region: &str, name: &str) -> Option<LoggingConfiguration> {
        if self.is_empty() {
            return None;
        }
        Some(LoggingConfiguration {
            dag_processing_logs: self
                .dag_processing_logs
                .as_ref()
                .map(|m| m.to_modelled(region, name, LOG_TYPE_DAG_PROCESSING)),
            scheduler_logs: self
                .scheduler_logs
                .as_ref()
                .map(|m| m.to_modelled(region, name, LOG_TYPE_SCHEDULER)),
            task_logs: self
                .task_logs
                .as_ref()
                .map(|m| m.to_modelled(region, name, LOG_TYPE_TASK)),
            webserver_logs: self
                .webserver_logs
                .as_ref()
                .map(|m| m.to_modelled(region, name, LOG_TYPE_WEBSERVER)),
            worker_logs: self
                .worker_logs
                .as_ref()
                .map(|m| m.to_modelled(region, name, LOG_TYPE_WORKER)),
        })
    }
}

/// The VPC networking components an environment was placed in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkConfigurationRecord {
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
}

impl NetworkConfigurationRecord {
    /// The modelled `NetworkConfiguration` for this record.
    fn to_modelled(&self) -> NetworkConfiguration {
        NetworkConfiguration {
            subnet_ids: Some(self.subnet_ids.clone()),
            security_group_ids: Some(self.security_group_ids.clone()),
        }
    }
}

/// Everything about an environment that a caller chose: the create's members, as every
/// later `UpdateEnvironment` merged into them.
///
/// Kept apart from the values the SERVICE minted (ARN, URLs, queue, endpoint service
/// names) so `CreateEnvironment` — which is `@idempotent` — can tell a replay of the same
/// create from a different create of a name that is already taken, by comparing specs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvironmentSpec {
    pub execution_role_arn: String,
    pub source_bucket_arn: String,
    pub dag_s3_path: String,
    pub network_configuration: NetworkConfigurationRecord,
    pub plugins_s3_path: Option<String>,
    pub plugins_s3_object_version: Option<String>,
    pub requirements_s3_path: Option<String>,
    pub requirements_s3_object_version: Option<String>,
    pub startup_script_s3_path: Option<String>,
    pub startup_script_s3_object_version: Option<String>,
    /// A `BTreeMap` so two equal option sets compare equal and read back in a stable
    /// order.
    pub airflow_configuration_options: BTreeMap<String, String>,
    pub environment_class: Option<String>,
    pub airflow_version: String,
    pub kms_key: Option<String>,
    pub max_workers: Option<i32>,
    pub min_workers: Option<i32>,
    pub schedulers: i32,
    pub min_webservers: i32,
    pub max_webservers: i32,
    pub webserver_access_mode: Option<String>,
    pub weekly_maintenance_window_start: Option<String>,
    pub endpoint_management: Option<String>,
    pub logging_configuration: LoggingConfigurationRecord,
}

/// A status change the clock owes an environment: where it lands, and when.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The `EnvironmentStatus` the environment settles into.
    pub to: String,
    /// The `UpdateStatus` `LastUpdate` settles into, when this transition is an update's.
    pub last_update_status: Option<String>,
    /// The instant the transition comes due.
    pub due_at: SystemTime,
}

/// The last update of an environment, as `UpdateEnvironment` recorded it.
#[derive(Debug, Clone)]
pub struct LastUpdateRecord {
    /// An `UpdateStatus` value.
    pub status: String,
    pub created_at: SystemTime,
    pub source: String,
    pub worker_replacement_strategy: Option<String>,
}

/// A stored environment: everything `CreateEnvironment` was given (as later updates left
/// it), plus what the service minted for it.
#[derive(Debug, Clone)]
pub struct EnvironmentRecord {
    pub name: String,
    pub arn: String,
    /// The region the create was signed for; every ARN and host this record reports
    /// carries it.
    pub region: String,
    /// An `EnvironmentStatus` value.
    pub status: String,
    /// The transition the clock owes, or `None` in a settled status.
    pub pending: Option<PendingTransition>,
    pub created_at: SystemTime,
    pub webserver_url: String,
    pub service_role_arn: String,
    pub celery_executor_queue: String,
    pub webserver_vpc_endpoint_service: String,
    pub database_vpc_endpoint_service: String,
    /// What the caller chose; see [`EnvironmentSpec`].
    pub spec: EnvironmentSpec,
    pub last_update: Option<LastUpdateRecord>,
    /// A `BTreeMap` so tags read back in a stable order.
    pub tags: BTreeMap<String, String>,
}

impl EnvironmentRecord {
    /// Materialise the transition the clock already owes this environment.
    ///
    /// This is the ONLY write a reporting operation makes: the change was owed at this
    /// instant whether or not anybody called.
    fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if now < pending.due_at {
            return;
        }
        self.status = pending.to;
        if let (Some(status), Some(last_update)) =
            (pending.last_update_status, self.last_update.as_mut())
        {
            last_update.status = status;
        }
        self.pending = None;
    }

    /// The modelled `Environment` for this record — the one shape `GetEnvironment`
    /// returns, built from what is stored rather than from the request asking for it.
    pub fn to_environment(&self) -> Environment {
        Environment {
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            webserver_url: Some(self.webserver_url.clone()),
            execution_role_arn: Some(self.spec.execution_role_arn.clone()),
            service_role_arn: Some(self.service_role_arn.clone()),
            kms_key: self.spec.kms_key.clone(),
            airflow_version: Some(self.spec.airflow_version.clone()),
            source_bucket_arn: Some(self.spec.source_bucket_arn.clone()),
            dag_s3_path: Some(self.spec.dag_s3_path.clone()),
            plugins_s3_path: self.spec.plugins_s3_path.clone(),
            plugins_s3_object_version: self.spec.plugins_s3_object_version.clone(),
            requirements_s3_path: self.spec.requirements_s3_path.clone(),
            requirements_s3_object_version: self.spec.requirements_s3_object_version.clone(),
            startup_script_s3_path: self.spec.startup_script_s3_path.clone(),
            startup_script_s3_object_version: self.spec.startup_script_s3_object_version.clone(),
            // `AirflowConfigurationOptions` has no modelled minimum, but an empty map
            // would claim an option set the caller never supplied, so absent is the
            // honest answer when none was.
            airflow_configuration_options: (!self.spec.airflow_configuration_options.is_empty())
                .then(|| map_of(&self.spec.airflow_configuration_options)),
            environment_class: self.spec.environment_class.clone(),
            max_workers: self.spec.max_workers,
            network_configuration: Some(self.spec.network_configuration.to_modelled()),
            logging_configuration: self
                .spec
                .logging_configuration
                .to_modelled(&self.region, &self.name),
            last_update: self.last_update.as_ref().map(|last_update| LastUpdate {
                status: Some(last_update.status.clone()),
                created_at: Some(::aws_smithy_types::DateTime::from(last_update.created_at)),
                // No update of this emulator fails, so there is no `UpdateError` to
                // report; inventing one would describe a failure that never happened.
                error: None,
                source: Some(last_update.source.clone()),
                worker_replacement_strategy: last_update.worker_replacement_strategy.clone(),
            }),
            weekly_maintenance_window_start: self.spec.weekly_maintenance_window_start.clone(),
            // `TagMap` is `@length(1, 50)`, so an untagged environment reports no `Tags`
            // member rather than an empty map outside that bound.
            tags: (!self.tags.is_empty()).then(|| map_of(&self.tags)),
            webserver_access_mode: self.spec.webserver_access_mode.clone(),
            min_workers: self.spec.min_workers,
            schedulers: Some(self.spec.schedulers),
            webserver_vpc_endpoint_service: Some(self.webserver_vpc_endpoint_service.clone()),
            database_vpc_endpoint_service: Some(self.database_vpc_endpoint_service.clone()),
            celery_executor_queue: Some(self.celery_executor_queue.clone()),
            endpoint_management: self.spec.endpoint_management.clone(),
            min_webservers: Some(self.spec.min_webservers),
            max_webservers: Some(self.spec.max_webservers),
        }
    }

    /// Enter `UPDATING` with a `PENDING` `LastUpdate`, and owe `AVAILABLE` with a
    /// `SUCCESS` one once [`TRANSITION_SPAN`] has elapsed.
    pub fn begin_update(
        &mut self,
        now: SystemTime,
        source: &str,
        worker_replacement_strategy: Option<String>,
    ) {
        self.status = STATUS_UPDATING.to_string();
        self.last_update = Some(LastUpdateRecord {
            status: UPDATE_STATUS_PENDING.to_string(),
            created_at: now,
            source: source.to_string(),
            worker_replacement_strategy,
        });
        self.pending = Some(PendingTransition {
            to: STATUS_AVAILABLE.to_string(),
            last_update_status: Some(UPDATE_STATUS_SUCCESS.to_string()),
            due_at: now + TRANSITION_SPAN,
        });
    }
}

/// A `BTreeMap` as the `HashMap` every modelled map member is generated as.
fn map_of(source: &BTreeMap<String, String>) -> HashMap<String, String> {
    source
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Per-service state for com-amazonaws-mwaa-amazonmwaa. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMwaaAmazonmwaaState {
    /// Every environment, keyed by its name — unique per account per Region — and ordered
    /// by it, so `ListEnvironments` paginates deterministically.
    pub environments: BTreeMap<String, EnvironmentRecord>,
    /// Environment ARN -> environment name, so the `/tags/{ResourceArn}` operations
    /// resolve an ARN back to the environment that owns the tags. Written and removed
    /// alongside `environments`, so it never names an environment that is gone.
    pub arn_index: HashMap<String, String>,
}

impl ComAmazonawsMwaaAmazonmwaaState {
    /// Materialise every status transition the clock already owes. Called at the top of
    /// each handler, reporting ones included: they may write only a change that was
    /// already due at this instant.
    pub fn settle(&mut self, now: SystemTime) {
        for environment in self.environments.values_mut() {
            environment.settle(now);
        }
    }

    /// The environment an `/tags/{ResourceArn}` request names, or `None` when the ARN
    /// resolves to nothing stored.
    pub fn environment_by_arn(&self, resource_arn: &str) -> Option<&EnvironmentRecord> {
        let name = self.arn_index.get(resource_arn)?;
        self.environments.get(name)
    }

    /// The environment an `/tags/{ResourceArn}` request names, for writing its tags.
    pub fn environment_by_arn_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut EnvironmentRecord> {
        let name = self.arn_index.get(resource_arn)?.clone();
        self.environments.get_mut(&name)
    }

    /// Store a new environment and index its ARN.
    pub fn insert(&mut self, record: EnvironmentRecord) {
        self.arn_index.insert(record.arn.clone(), record.name.clone());
        self.environments.insert(record.name.clone(), record);
    }

    /// Remove an environment and its ARN index entry (and with it its tags, which live on
    /// the record). `None` when no environment of that name is stored.
    pub fn remove(&mut self, name: &str) -> Option<EnvironmentRecord> {
        let record = self.environments.remove(name)?;
        self.arn_index.remove(&record.arn);
        Some(record)
    }
}

/// The message an environment-not-found refusal carries: the sentence names the
/// environment the REQUEST asked for.
///
/// Request-derived, so the form is the rule and the name is whatever the request carried.
/// Every CONTROL-PLANE operation addressed by `Name` — `GetEnvironment`,
/// `UpdateEnvironment`, `DeleteEnvironment`, `CreateCliToken`, `CreateWebLoginToken` —
/// words the same condition through this one builder, so none of them can drift from the
/// rest. `InvokeRestApi` is refused by the environment data endpoint it proxies through
/// instead, which words it differently: see
/// [`ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE`].
pub fn environment_not_found_message(name: &str) -> String {
    format!("Environment {name} not found.")
}

/// What the environment data endpoint says about an environment it could not resolve.
///
/// `InvokeRestApi` is served through that endpoint's proxy to an environment's Apache
/// Airflow web server, so its not-found refusal is worded by the front door that resolves
/// the request to a web server — ahead of the request-derived builder above — and names no
/// environment: the sentence is fixed, and it is carried as the endpoint's own document
/// payload rather than as a serialized exception (see [`crate::service::env_endpoint`]).
/// The text itself is only an example of that payload; what a client branches on is the 404
/// and the `x-amzn-errortype` code beside it.
pub const ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE: &str = "Environment not found";

/// The message the `/tags/{ResourceArn}` operations' not-found refusal carries.
///
/// A tagging request names its resource by an ARN the caller already holds, so there is
/// nothing in it to build a sentence from that would tell the caller anything new: the
/// refusal states the condition and nothing else. What a client branches on is the 404
/// and the `NotFoundException` wire code these operations answer with (see
/// [`crate::scaffolded::Scaffolded::not_found`]), not this prose.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// `AirflowIdentity`'s own `@length(1, 64)` maximum, which
/// [`airflow_identity`] keeps the reported identity inside.
const AIRFLOW_IDENTITY_MAX_LENGTH: usize = 64;

/// The region this request was signed for, read from its SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of an environment:
/// `arn:aws:airflow:<region>:<account>:environment/<Name>`, the shape `EnvironmentArn`'s
/// own pattern (`^arn:aws(-[a-z]+)?:airflow:[a-z0-9\-]+:\d{12}:environment/\w+`) requires.
pub fn environment_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("environment/{name}"))
}

/// The CloudWatch Logs group ARN an Apache Airflow log type publishes to. MWAA names the
/// group `airflow-<EnvironmentName>-<LogType>`, which is the shape the
/// `CloudWatchLogGroupArn` doc's own example carries
/// (`arn:aws:logs:us-east-1:…:log-group:airflow-MyMWAAEnvironment-…`).
pub fn log_group_arn(region: &str, name: &str, log_type: &str) -> String {
    ::engine::emulator_arn(
        "logs",
        region,
        &format!("log-group:airflow-{name}-{log_type}"),
    )
}

/// The service-linked role ARN of an environment, as the `ServiceRoleArn` doc describes
/// ("the service-linked role of the environment"). It is the same role for every
/// environment, so it is not content-derived.
pub fn service_role_arn() -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        "role/aws-service-role/airflow.amazonaws.com/AWSServiceRoleForAmazonMWAA",
    )
}

/// The SQS queue ARN of the environment's Celery Executor, matching
/// `CeleryExecutorQueue`'s pattern
/// (`^arn:aws(-[a-z]+)?:sqs:[a-z0-9\-]+:\d{12}:[a-zA-Z_0-9+=,.@\-_/]+$`).
pub fn celery_executor_queue(region: &str, name: &str) -> String {
    ::engine::emulator_arn("sqs", region, &format!("airflow-celery-{name}"))
}

/// The Apache Airflow web-server host of an environment, as `WebserverUrl`'s pattern
/// (`^https://.+$`) and MWAA's own host shape
/// (`https://<environment-id>.c2.<region>.airflow.amazonaws.com`) require.
///
/// The id segment is content-derived (a digest of the environment's own ARN, so it is
/// stable across reads, differs between two environments, and never leaks a clock or a
/// request id) and is laid out like the UUID AWS uses there.
pub fn webserver_url(region: &str, arn: &str) -> String {
    let id = uuid_shaped_id(&format!("webserver/{arn}"));
    format!("https://{id}.c2.{region}.airflow.amazonaws.com")
}

/// The `Hostname` of an environment's Apache Airflow web server: the host of the
/// environment's STORED [`webserver_url`], with the scheme and any path removed —
/// `Hostname`'s own pattern
/// (`^(([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\-]*[a-zA-Z0-9])\.)*([A-Za-z0-9]|…)$`) carries a
/// host and nothing else. Reading it off the stored URL is what keeps the host
/// `CreateCliToken` and `CreateWebLoginToken` report equal to the one `GetEnvironment`
/// reports for the same environment.
pub fn webserver_hostname(webserver_url: &str) -> &str {
    let without_scheme = webserver_url
        .split_once("://")
        .map_or(webserver_url, |(_scheme, rest)| rest);
    match without_scheme.split_once('/') {
        Some((host, _path)) => host,
        None => without_scheme,
    }
}

/// The IAM identity a request was signed by, as this emulator knows it.
///
/// MWAA reports the identity that asked for a token (`IamIdentity`: "This might be an IAM
/// user, or an assumed or federated identity"). The only identity fact this emulator
/// holds about a request is the access-key id its SigV4 credential presented, so that is
/// what is reported: no user, role or session is invented around it.
pub fn caller_iam_identity(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::engine::extract_akid(&ctx.parts().headers)
}

/// The Apache Airflow user name the calling IAM identity maps to.
///
/// MWAA maps the IAM identity to an Apache Airflow user (`AirflowIdentity`: "The user name
/// of the Apache Airflow identity creating the web login token"); the mapping here is the
/// identity itself, truncated to [`AIRFLOW_IDENTITY_MAX_LENGTH`] so the reported value
/// stays inside the modelled bound.
pub fn airflow_identity(iam_identity: &str) -> String {
    iam_identity
        .chars()
        .take(AIRFLOW_IDENTITY_MAX_LENGTH)
        .collect()
}

/// A fresh Apache Airflow token, for `CreateCliToken` and `CreateWebLoginToken`.
///
/// The token authenticates to the environment's Apache Airflow web server, NOT to this
/// emulator, so it must never be accepted as a SigV4 credential — but it is still a
/// secret handed to a caller, and nothing in a service may construct a secret itself. It
/// is therefore minted by the one component that generates secrets, the emulator's
/// credential store, and then removed from that store again: the caller gets the secret,
/// the emulator keeps no credential it would accept, and nothing accumulates per call.
///
/// `None` when the store minted no session-token secret, which leaves the operation to
/// refuse rather than report a token it made up.
pub fn mint_airflow_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(str::to_string);
    // The token is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    token
}

/// The VPC endpoint service name of the environment's web server, matching
/// `VpcEndpointServiceName`'s pattern
/// (`^([a-z.-]+)?com\.amazonaws\.vpce\.[a-z0-9\-]+\.[a-zA-Z_0-9+=,.@\-_/]+$`).
pub fn webserver_vpc_endpoint_service(region: &str, arn: &str) -> String {
    vpc_endpoint_service(region, &format!("webserver-vpce/{arn}"))
}

/// The VPC endpoint service name of the environment's Amazon RDS database; see
/// [`webserver_vpc_endpoint_service`].
pub fn database_vpc_endpoint_service(region: &str, arn: &str) -> String {
    vpc_endpoint_service(region, &format!("database-vpce/{arn}"))
}

/// `com.amazonaws.vpce.<region>.vpce-svc-<17 hex digits>` — the shape an EC2 endpoint
/// service name carries. The id is derived from `seed` so it is stable across reads and
/// differs between two environments.
fn vpc_endpoint_service(region: &str, seed: &str) -> String {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(seed.as_bytes());
    let suffix = ::hex::encode(&digest[..9]);
    format!("com.amazonaws.vpce.{region}.vpce-svc-{}", &suffix[..17])
}

/// A UUID-shaped (8-4-4-4-12 hex) id derived from `seed`.
fn uuid_shaped_id(seed: &str) -> String {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(seed.as_bytes());
    let hex = ::hex::encode(&digest[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// One page of a `@paginated` listing: the items to return, plus the cursor that resumes
/// exactly where the page ended (`None` when the page is the last one).
#[derive(Debug, Clone)]
pub struct Page {
    pub items: Vec<String>,
    pub next_token: Option<String>,
}

/// A `NextToken` no listing of this service ever minted.
#[derive(Debug, Clone, Copy)]
pub struct MalformedCursor;

/// Prefix the cursor plaintext carries, so a token this service never minted cannot
/// decode into a usable position.
const CURSOR_PREFIX: &str = "mwaa-after-environment:";

/// Encode the resume cursor that follows the environment named `name`.
///
/// Opaque to the caller (hex of the internal cursor text) and well within `NextToken`'s
/// `@length(0, 2048)`. The cursor names a POSITION in the name ordering rather than an
/// offset, so resuming never repeats or skips an environment when one is created or
/// deleted between pages.
///
/// It is self-describing, so `ListEnvironments` validates a token by decoding it and
/// never has to record the tokens it issued — which matters because `ListEnvironments` is
/// a reporting operation: remembering an issued token would make a read mutate state.
pub fn encode_cursor(name: &str) -> String {
    ::hex::encode(format!("{CURSOR_PREFIX}{name}"))
}

/// Decode a cursor minted by [`encode_cursor`]; `None` starts at the beginning.
pub fn decode_cursor(next_token: Option<&str>) -> Result<Option<String>, MalformedCursor> {
    let Some(token) = next_token else {
        return Ok(None);
    };
    if token.is_empty() {
        // `NextToken` is `@length(0, 2048)`, so the empty string is a legal value; it
        // names no position, which is the start.
        return Ok(None);
    }
    let bytes = ::hex::decode(token).map_err(|_| MalformedCursor)?;
    let text = String::from_utf8(bytes).map_err(|_| MalformedCursor)?;
    text.strip_prefix(CURSOR_PREFIX)
        .map(|name| Some(name.to_string()))
        .ok_or(MalformedCursor)
}

/// Take the page of `ordered` names (sorted) that follows `after`, at most `max_results`
/// long.
///
/// `max_results` is `@range(1, 25)`, so the boundary validator has already refused
/// anything outside it; an absent one takes a full page.
pub fn page_of(
    ordered: impl IntoIterator<Item = String>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> Page {
    let limit = max_results
        .unwrap_or(MAX_RESULTS_LIMIT)
        .clamp(1, MAX_RESULTS_LIMIT) as usize;
    let mut items: Vec<String> = Vec::new();
    let mut more_follow = false;
    for name in ordered
        .into_iter()
        .filter(|name| after.is_none_or(|after| name.as_str() > after))
    {
        if items.len() == limit {
            // One name beyond the page proves a next page exists; the cursor names where
            // this page ended, so resuming never repeats or skips a name.
            more_follow = true;
            break;
        }
        items.push(name);
    }
    let next_token = more_follow
        .then(|| items.last().map(|name| encode_cursor(name)))
        .flatten();
    Page { items, next_token }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMwaaAmazonmwaaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMwaaAmazonmwaaState>)
}

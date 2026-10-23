// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-emrcontainers-awschicagowebservice service.
//!
//! This holds the virtual-cluster / job-run core loop of EMR on EKS, the job templates
//! `StartJobRun` values are stored in, plus the security-configuration templates a
//! virtual cluster can reference. Every field a describe or a list answers with is read
//! back out of one of these records: nothing is echoed from the request that asked for
//! it, and nothing is canned.
//!
//! Two things live here beside the records because several operations need them and a
//! second copy would let two operations disagree:
//!
//! * [`ComAmazonawsEmrcontainersAwschicagowebserviceState::mint_id`] — the one place a
//!   `ResourceIdString` (`^[0-9a-z]+$`) is minted.
//! * [`ComAmazonawsEmrcontainersAwschicagowebserviceState::settle`] — the one place a
//!   status transition the clock has already made due is materialised. Every handler,
//!   reporting or mutating, calls it first, so a reader never has to know which other
//!   call started the transition.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    Configuration, ConfigurationOverrides, ContainerProvider, JobDriver, JobRun, JobTemplate,
    JobTemplateData, ParametricConfigurationOverrides, RetryPolicyConfiguration,
    SchedulerConfiguration, SecurityConfiguration, SecurityConfigurationData,
    TemplateParameterConfiguration, VirtualCluster,
};

/// The ARN namespace this service's ARNs carry (`OPERATIONS.md`: ARN namespace
/// `emr-containers`), e.g.
/// `arn:aws:emr-containers:{region}:{account}:/virtualclusters/{id}`.
pub const ARN_SERVICE: &str = "emr-containers";

/// The region an ARN is built in when the request carries no SigV4 credential scope to
/// read one from (an unsigned in-process call, say).
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a status transition the clock owes takes to come due: `PENDING` ->
/// `RUNNING` after `StartJobRun`, `CANCEL_PENDING` -> `CANCELLED` after `CancelJobRun`,
/// `TERMINATING` -> `TERMINATED` after `DeleteVirtualCluster`.
///
/// EMR on EKS documents no duration for any of them, so this span is the emulator's own
/// choice. It is deliberately non-zero: a caller that reads straight after the call that
/// started the transition observes the transient state, exactly as it would against AWS.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(400);

/// `VirtualClusterState` members this slice uses. Each is a value of the generated
/// `VIRTUAL_CLUSTER_STATE_VALUES` catalog in `ops::types::shared_types`.
pub const CLUSTER_RUNNING: &str = "RUNNING";
pub const CLUSTER_TERMINATING: &str = "TERMINATING";
pub const CLUSTER_TERMINATED: &str = "TERMINATED";

/// `JobRunState` members this slice uses. Each is a value of the generated
/// `JOB_RUN_STATE_VALUES` catalog in `ops::types::shared_types`.
///
/// `JOB_SUBMITTED` is the one state no handler here writes: EMR on EKS reports it for a
/// run handed to the scheduler, and this slice runs no scheduler (it accepts a run
/// PENDING and the clock moves it straight to RUNNING). It is named because
/// `job_state_is_terminal` and the queue-side members below are written against the whole
/// catalog, so a later slice that does schedule has one place to add it.
pub const JOB_PENDING: &str = "PENDING";
pub const JOB_SUBMITTED: &str = "SUBMITTED";
pub const JOB_RUNNING: &str = "RUNNING";
pub const JOB_CANCEL_PENDING: &str = "CANCEL_PENDING";
pub const JOB_CANCELLED: &str = "CANCELLED";
pub const JOB_FAILED: &str = "FAILED";
pub const JOB_COMPLETED: &str = "COMPLETED";

/// The `JobRunState` values no further transition leaves. A job run that reaches one of
/// them has a `finishedAt`.
pub const JOB_TERMINAL_STATES: &[&str] = &[JOB_CANCELLED, JOB_FAILED, JOB_COMPLETED];

/// Whether a `JobRunState` is one a job run never leaves.
pub fn job_state_is_terminal(state: &str) -> bool {
    JOB_TERMINAL_STATES.contains(&state)
}

/// The alphabet a minted id is drawn from: `ResourceIdString` is
/// `^[0-9a-z]+$` (len 1..64), so base-36 over `0-9a-z` is exactly its character set.
const ID_ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Characters in a minted id. Well inside `ResourceIdString`'s 1..64 length, and wide
/// enough that the wall clock plus the per-state counter below never collide.
const ID_WIDTH: usize = 25;

/// The most virtual clusters this emulator keeps at once.
///
/// Not an AWS quota: it is the emulator's own retention cap, so the store cannot grow
/// with the number of requests served. A retired cluster (TERMINATED and holding no job
/// runs) is reclaimed oldest-first to make room; once every slot holds a live cluster,
/// `CreateVirtualCluster` is refused rather than allocating further. Sized well above any
/// plausible test or demo workload, so a client never meets it by accident.
pub const MAX_VIRTUAL_CLUSTERS: usize = 200;

/// The most job runs this emulator keeps at once, on the same terms as
/// [`MAX_VIRTUAL_CLUSTERS`]: a run in a terminal state is reclaimed oldest-first, and
/// `StartJobRun` is refused once every slot holds a run that has not finished.
pub const MAX_JOB_RUNS: usize = 2_000;

/// The most job templates this emulator keeps at once.
///
/// Not an AWS quota: the emulator's own retention cap, so the store cannot grow with the
/// number of requests served. A job template has no lifecycle state, so nothing about it
/// is ever "retired" and there is nothing to reclaim: once every slot is taken,
/// `CreateJobTemplate` is refused until a caller releases one with `DeleteJobTemplate`.
pub const MAX_JOB_TEMPLATES: usize = 500;

/// The most security configurations this emulator keeps at once.
///
/// Not an AWS quota: the emulator's own retention cap, so the store cannot grow with the
/// number of requests served. A security configuration has no status and so no retired
/// form to reclaim — the only way one leaves the store is a caller deleting it — so once
/// every slot is taken `CreateSecurityConfiguration` is refused rather than allocating
/// further. Sized well above any plausible test or demo workload.
pub const MAX_SECURITY_CONFIGURATIONS: usize = 200;

/// The idempotency scope `CreateVirtualCluster` remembers a `clientToken` under.
///
/// A token is scoped to the operation that minted the resource, so the same token on
/// another operation is a different request.
pub const CREATE_VIRTUAL_CLUSTER_SCOPE: &str = "CreateVirtualCluster";

/// The idempotency scope `CreateSecurityConfiguration` remembers a `clientToken` under,
/// on the same terms as [`CREATE_VIRTUAL_CLUSTER_SCOPE`]: the same token replayed names
/// the security configuration the first call minted.
pub const CREATE_SECURITY_CONFIGURATION_SCOPE: &str = "CreateSecurityConfiguration";

/// The idempotency scope `StartJobRun` remembers a `clientToken` under: scoped to the
/// virtual cluster as well as the operation, because the token identifies one submission
/// to one cluster.
pub fn start_job_run_scope(virtual_cluster_id: &str) -> String {
    format!("StartJobRun:{virtual_cluster_id}")
}

/// The idempotency scope `CreateJobTemplate` remembers a `clientToken` under. A job
/// template is account-and-region wide (`POST /jobtemplates` names no parent resource),
/// so the operation alone scopes the token.
pub const CREATE_JOB_TEMPLATE_SCOPE: &str = "CreateJobTemplate";

/// Wall-clock milliseconds since the epoch, the unit every stored timestamp uses.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// A stored millisecond timestamp as the modelled `timestamp` type.
pub fn date_time(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// A modelled `timestamp` as the millisecond unit every stored timestamp uses, so a
/// query-string bound and a stored `createdAt` compare in one unit.
pub fn millis_of(value: &::aws_smithy_types::DateTime) -> i64 {
    value
        .secs()
        .saturating_mul(1_000)
        .saturating_add(i64::from(value.subsec_nanos()) / 1_000_000)
}

/// `arn:aws:emr-containers:{region}:{account}:/virtualclusters/{id}` — the
/// `VirtualClusterArn` shape's pattern.
pub fn virtual_cluster_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("/virtualclusters/{id}"))
}

/// `arn:aws:emr-containers:{region}:{account}:/virtualclusters/{vc}/jobruns/{id}` — the
/// `JobArn` shape's pattern.
pub fn job_run_arn(region: &str, virtual_cluster_id: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("/virtualclusters/{virtual_cluster_id}/jobruns/{id}"),
    )
}

/// `arn:aws:emr-containers:{region}:{account}:/jobtemplates/{id}` — the
/// `JobTemplateArn` shape's pattern.
pub fn job_template_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("/jobtemplates/{id}"))
}

/// `arn:aws:emr-containers:{region}:{account}:/securityconfigurations/{id}` — the
/// `SecurityConfigurationArn` shape's pattern.
pub fn security_configuration_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("/securityconfigurations/{id}"),
    )
}

/// The region to build an ARN in for this request: the region of its SigV4 credential
/// scope, so an ARN names the region the caller signed for.
pub fn request_region(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A status change the clock owes a resource: at [`due_at`](Self::due_at) its state
/// becomes [`next_state`](Self::next_state).
///
/// The monotonic `Instant` decides when it comes due; the wall-clock millisecond is what
/// a `finishedAt` reports, so the reported instant is the one the transition was owed at
/// rather than whenever a caller next looked.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due_at: Instant,
    pub due_at_millis: i64,
    pub next_state: &'static str,
}

impl PendingTransition {
    /// A transition to `next_state`, due [`TRANSITION_SPAN`] from now.
    pub fn due(next_state: &'static str) -> Self {
        Self {
            due_at: Instant::now() + TRANSITION_SPAN,
            due_at_millis: now_millis() + i64::try_from(TRANSITION_SPAN.as_millis()).unwrap_or(0),
            next_state,
        }
    }
}

/// One virtual cluster, as `CreateVirtualCluster` persisted it.
#[derive(Debug, Clone)]
pub struct VirtualClusterRecord {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub state: String,
    pub container_provider: ContainerProvider,
    pub created_at_millis: i64,
    pub tags: HashMap<String, String>,
    pub security_configuration_id: Option<String>,
    pub session_enabled: Option<bool>,
    pub scheduler_configuration: Option<SchedulerConfiguration>,
    pub client_token: String,
    pub pending: Option<PendingTransition>,
}

impl VirtualClusterRecord {
    /// Materialise this cluster's transition if the clock has made it due.
    fn settle(&mut self, now: Instant) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if now < pending.due_at {
            self.pending = Some(pending);
            return;
        }
        self.state = pending.next_state.to_string();
    }

    /// The modelled `VirtualCluster` this record represents.
    ///
    /// `schedulerStatus` stays absent: it reports the live queue of a scheduler, and
    /// this slice persists a `schedulerConfiguration` without scheduling against it
    /// (the queue-depth and concurrency limits the shape documents are not enforced
    /// yet), so reporting a queue state would be reporting work nothing did.
    fn to_virtual_cluster(&self) -> VirtualCluster {
        VirtualCluster {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            state: Some(self.state.clone()),
            container_provider: Some(self.container_provider.clone()),
            created_at: Some(date_time(self.created_at_millis)),
            tags: Some(self.tags.clone()),
            security_configuration_id: self.security_configuration_id.clone(),
            session_enabled: self.session_enabled,
            scheduler_configuration: self.scheduler_configuration.clone(),
            scheduler_status: None,
        }
    }
}

/// One job run, as `StartJobRun` persisted it.
#[derive(Debug, Clone)]
pub struct JobRunRecord {
    pub id: String,
    pub name: Option<String>,
    pub virtual_cluster_id: String,
    pub arn: String,
    pub state: String,
    pub client_token: String,
    pub execution_role_arn: Option<String>,
    pub release_label: Option<String>,
    pub job_driver: Option<JobDriver>,
    pub configuration_overrides: Option<ConfigurationOverrides>,
    pub retry_policy_configuration: Option<RetryPolicyConfiguration>,
    pub created_at_millis: i64,
    pub finished_at_millis: Option<i64>,
    pub state_details: Option<String>,
    pub tags: HashMap<String, String>,
    /// Creation sequence, so job runs minted within one millisecond still have a total
    /// order for `ListJobRuns` to page through.
    pub sequence: u64,
    pub pending: Option<PendingTransition>,
}

impl JobRunRecord {
    /// Materialise this job run's transition if the clock has made it due. A transition
    /// into a terminal state is what sets `finishedAt`, and it reports the instant the
    /// transition was owed at.
    fn settle(&mut self, now: Instant) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if now < pending.due_at {
            self.pending = Some(pending);
            return;
        }
        self.state = pending.next_state.to_string();
        if job_state_is_terminal(pending.next_state) {
            self.finished_at_millis = Some(pending.due_at_millis);
        }
    }

    /// The modelled `JobRun` this record represents.
    ///
    /// `createdBy`, `failureReason` and `retryPolicyExecution` stay absent: no identity
    /// created this run, nothing failed, and no attempt was executed — this emulator
    /// runs no Spark job, so reporting an attempt count would be fabricated.
    pub fn to_job_run(&self) -> JobRun {
        JobRun {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            virtual_cluster_id: Some(self.virtual_cluster_id.clone()),
            arn: Some(self.arn.clone()),
            state: Some(self.state.clone()),
            client_token: Some(self.client_token.clone()),
            execution_role_arn: self.execution_role_arn.clone(),
            release_label: self.release_label.clone(),
            configuration_overrides: self.configuration_overrides.clone(),
            job_driver: self.job_driver.clone(),
            created_at: Some(date_time(self.created_at_millis)),
            created_by: None,
            finished_at: self.finished_at_millis.map(date_time),
            state_details: self.state_details.clone(),
            failure_reason: None,
            tags: Some(self.tags.clone()),
            retry_policy_configuration: self.retry_policy_configuration.clone(),
            retry_policy_execution: None,
        }
    }
}

/// Append one component of a fingerprint rendering, behind its own byte length.
///
/// The length prefix is what keeps the rendering UNAMBIGUOUS: without it a value could
/// run into its neighbour (`"a" + "bc"` and `"ab" + "c"` render alike) and two different
/// requests would fingerprint the same, so a changed parameter could be answered as a
/// replay.
fn push_part(rendered: &mut String, part: &str) {
    rendered.push_str(&part.len().to_string());
    rendered.push(':');
    rendered.push_str(part);
    rendered.push(';');
}

/// Append an optional component, with a present/absent marker of its own, so an absent
/// member and a member whose value is the empty string are told apart.
fn push_option(rendered: &mut String, part: Option<&str>) {
    match part {
        Some(value) => {
            push_part(rendered, "1");
            push_part(rendered, value);
        }
        None => push_part(rendered, "0"),
    }
}

/// Append a `TagMap`-shaped member, its entries in key order so the fingerprint does not
/// depend on hash iteration order. An absent map and an empty one render alike, because
/// they persist the same record.
fn push_string_map(rendered: &mut String, map: Option<&HashMap<String, String>>) {
    let sorted: BTreeMap<&str, &str> = map
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    push_part(rendered, &sorted.len().to_string());
    for (key, value) in sorted {
        push_part(rendered, key);
        push_part(rendered, value);
    }
}

/// Append a `ConfigurationList`. A list is ORDER-SIGNIFICANT, so it renders in request
/// order.
///
/// `Configuration` NESTS ITSELF and this list comes straight from request input, so the
/// walk is driven by an explicit worklist rather than by recursion: nesting depth then
/// costs heap — which the server's request-body cap bounds — instead of one stack frame
/// per level. A Rust stack overflow aborts the process rather than raising a catchable
/// panic, so one deeply nested request must not be able to reach it.
///
/// The rendering stays unambiguous because every list is framed by its own length and
/// every item contributes exactly its classification, its properties and one child list,
/// visited depth-first in list order.
fn push_configurations(rendered: &mut String, list: Option<&[Configuration]>) {
    let mut worklist: Vec<&[Configuration]> = vec![list.unwrap_or_default()];
    while let Some(items) = worklist.pop() {
        push_part(rendered, &items.len().to_string());
        for item in items {
            push_part(rendered, &item.classification);
            push_string_map(rendered, item.properties.as_ref());
        }
        // Pushed in reverse, so the next list popped is the FIRST item's children and the
        // walk stays depth-first in list order.
        for item in items.iter().rev() {
            worklist.push(item.configurations.as_deref().unwrap_or_default());
        }
    }
}

/// Append a `ParametricConfigurationOverrides`, member by member.
fn push_parametric_overrides(
    rendered: &mut String,
    overrides: Option<&ParametricConfigurationOverrides>,
) {
    let Some(overrides) = overrides else {
        push_part(rendered, "0");
        return;
    };
    push_part(rendered, "1");
    push_configurations(rendered, overrides.application_configuration.as_deref());
    match overrides.monitoring_configuration.as_ref() {
        None => push_part(rendered, "0"),
        Some(monitoring) => {
            push_part(rendered, "1");
            push_option(rendered, monitoring.persistent_app_ui.as_deref());
            match monitoring.cloud_watch_monitoring_configuration.as_ref() {
                None => push_part(rendered, "0"),
                Some(cloud_watch) => {
                    push_part(rendered, "1");
                    push_option(rendered, cloud_watch.log_group_name.as_deref());
                    push_option(rendered, cloud_watch.log_stream_name_prefix.as_deref());
                }
            }
            match monitoring.s3_monitoring_configuration.as_ref() {
                None => push_part(rendered, "0"),
                Some(s3) => {
                    push_part(rendered, "1");
                    push_option(rendered, s3.log_uri.as_deref());
                }
            }
        }
    }
}

/// Append a `JobDriver`: both union-shaped members, each present or absent.
fn push_job_driver(rendered: &mut String, driver: &JobDriver) {
    match driver.spark_submit_job_driver.as_ref() {
        None => push_part(rendered, "0"),
        Some(spark_submit) => {
            push_part(rendered, "1");
            push_part(rendered, &spark_submit.entry_point);
            let arguments = spark_submit.entry_point_arguments.as_deref().unwrap_or(&[]);
            push_part(rendered, &arguments.len().to_string());
            for argument in arguments {
                push_part(rendered, argument);
            }
            push_option(rendered, spark_submit.spark_submit_parameters.as_deref());
        }
    }
    match driver.spark_sql_job_driver.as_ref() {
        None => push_part(rendered, "0"),
        Some(spark_sql) => {
            push_part(rendered, "1");
            push_option(rendered, spark_sql.entry_point.as_deref());
            push_option(rendered, spark_sql.spark_sql_parameters.as_deref());
        }
    }
}

/// Append a `TemplateParameterConfigurationMap`, its entries in key order.
fn push_parameter_configuration(
    rendered: &mut String,
    map: Option<&HashMap<String, TemplateParameterConfiguration>>,
) {
    let sorted: BTreeMap<&str, &TemplateParameterConfiguration> = map
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.as_str(), value))
        .collect();
    push_part(rendered, &sorted.len().to_string());
    for (key, value) in sorted {
        push_part(rendered, key);
        push_option(rendered, value.r#type.as_deref());
        push_option(rendered, value.default_value.as_deref());
    }
}

/// A fixed-size fingerprint of EVERY parameter a job template is built from.
///
/// `clientToken` is `@idempotencyToken`, which means "the same token names the same
/// request": a second call carrying a token already spent is a replay only when it
/// really is the same request. This digest is how that is decided, so it has to cover
/// every member that ends up on the stored record — `name`, the whole
/// `jobTemplateData` tree, `tags` and `kmsKeyArn` — and nothing that does not.
///
/// SHA-256, so the size is fixed however large the request was, and a map's entries are
/// rendered in key order so one request never fingerprints two ways.
pub fn job_template_fingerprint(
    name: &str,
    data: &JobTemplateData,
    tags: Option<&HashMap<String, String>>,
    kms_key_arn: Option<&str>,
) -> String {
    let mut rendered = String::new();
    push_part(&mut rendered, name);
    push_part(&mut rendered, &data.execution_role_arn);
    push_part(&mut rendered, &data.release_label);
    push_parametric_overrides(&mut rendered, data.configuration_overrides.as_ref());
    push_job_driver(&mut rendered, &data.job_driver);
    push_parameter_configuration(&mut rendered, data.parameter_configuration.as_ref());
    push_string_map(&mut rendered, data.job_tags.as_ref());
    push_string_map(&mut rendered, tags);
    push_option(&mut rendered, kms_key_arn);

    let mut hasher = Sha256::new();
    hasher.update(rendered.as_bytes());
    hex::encode(hasher.finalize())
}

/// One job template, as `CreateJobTemplate` persisted it.
///
/// A job template has no lifecycle state: EMR on EKS stores the `StartJobRun` values and
/// nothing about it transitions, so there is no `state` member and no pending transition
/// here.
#[derive(Debug, Clone)]
pub struct JobTemplateRecord {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub created_at_millis: i64,
    pub tags: HashMap<String, String>,
    pub job_template_data: JobTemplateData,
    pub kms_key_arn: Option<String>,
    pub client_token: String,
    /// [`job_template_fingerprint`] of the request that minted this template, so a later
    /// call carrying the same `clientToken` is answered as a replay only when it is the
    /// same request.
    pub request_fingerprint: String,
    /// Creation sequence, so templates minted within one millisecond still have a total
    /// order for `ListJobTemplates` to page through.
    pub sequence: u64,
}

impl JobTemplateRecord {
    /// The modelled `JobTemplate` this record represents.
    ///
    /// `createdBy` stays absent: no IAM identity created this template — the emulator
    /// verifies a signature but mints no `RequestIdentityUserArn` to attribute it to.
    /// `decryptionError` stays absent too: a `kmsKeyArn` is persisted as submitted and
    /// nothing is encrypted, so no decryption ever failed.
    pub fn to_job_template(&self) -> JobTemplate {
        JobTemplate {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(date_time(self.created_at_millis)),
            created_by: None,
            tags: Some(self.tags.clone()),
            job_template_data: self.job_template_data.clone(),
            kms_key_arn: self.kms_key_arn.clone(),
            decryption_error: None,
        }
    }
}

/// One security configuration, as `CreateSecurityConfiguration` persisted it.
///
/// It carries no state member: EMR on EKS models a security configuration as a template,
/// with no lifecycle status, so there is no transition for the clock to owe one.
#[derive(Debug, Clone)]
pub struct SecurityConfigurationRecord {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub created_at_millis: i64,
    pub security_configuration_data: SecurityConfigurationData,
    pub container_provider: Option<ContainerProvider>,
    pub tags: HashMap<String, String>,
    pub client_token: String,
}

impl SecurityConfigurationRecord {
    /// The modelled `SecurityConfiguration` this record represents.
    ///
    /// `createdBy` stays absent: no identity created this configuration in the emulator,
    /// so reporting one would be fabricating a principal.
    ///
    /// `containerProvider` is stored (the create accepts it) but is NOT a member of the
    /// `SecurityConfiguration` shape, so nothing reports it: see the shape's members in
    /// `ops::types::shared_types`.
    pub fn to_security_configuration(&self) -> SecurityConfiguration {
        SecurityConfiguration {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(date_time(self.created_at_millis)),
            created_by: None,
            security_configuration_data: Some(self.security_configuration_data.clone()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-emrcontainers-awschicagowebservice. A handler reaches it through [`service_state`].
///
/// # Bounding the store
///
/// Every record set is CAPPED ([`MAX_VIRTUAL_CLUSTERS`], [`MAX_JOB_RUNS`],
/// [`MAX_JOB_TEMPLATES`], [`MAX_SECURITY_CONFIGURATIONS`]) and retired
/// records are reclaimed oldest-first when a cap is reached, so the store cannot grow
/// with the number of requests served: a caller that creates resources in a loop ends up
/// recycling the same bounded set of slots, and once every slot holds a LIVE resource the
/// next create is refused with the modelled `ValidationException` instead of allocating.
/// Each record's own size is bounded by the model's length constraints on the members it
/// stores and, above those, by the server's request-body cap.
#[derive(Debug, Default)]
pub struct ComAmazonawsEmrcontainersAwschicagowebserviceState {
    /// Virtual clusters by `id`. A terminated cluster stays here while it has a slot: EMR
    /// on EKS keeps it describable rather than answering 404 once it is gone.
    pub virtual_clusters: HashMap<String, VirtualClusterRecord>,
    /// Job runs by `id`.
    pub job_runs: HashMap<String, JobRunRecord>,
    /// Security configurations by `id`, capped at [`MAX_SECURITY_CONFIGURATIONS`].
    pub security_configurations: HashMap<String, SecurityConfigurationRecord>,
    /// Job-run ids per virtual cluster, in creation order.
    pub job_runs_by_cluster: HashMap<String, Vec<String>>,
    /// Job templates by `id`. Capped by [`MAX_JOB_TEMPLATES`]; a template leaves only
    /// when `DeleteJobTemplate` removes it.
    pub job_templates: HashMap<String, JobTemplateRecord>,
    /// `(idempotency scope, clientToken) -> minted resource id`, so a replayed
    /// `@idempotencyToken` returns the resource the first call minted instead of a
    /// second one. One entry per stored record, inserted and reclaimed with it, so this
    /// map is bounded by the two caps above.
    pub idempotency: HashMap<(String, String), String>,
    /// The ids of the clusters that owe the clock a transition, so [`Self::settle`] walks
    /// only those rather than the whole store.
    pub pending_clusters: HashSet<String>,
    /// The ids of the job runs that owe the clock a transition.
    pub pending_job_runs: HashSet<String>,
    /// Monotonic counter behind [`Self::mint_id`].
    pub id_counter: u64,
}

impl ComAmazonawsEmrcontainersAwschicagowebserviceState {
    /// Mint a fresh `ResourceIdString`: [`ID_WIDTH`] base-36 characters, so it matches
    /// `^[0-9a-z]+$`.
    ///
    /// The value mixes the wall clock with a monotonic counter, so two ids minted in the
    /// same millisecond still differ and an id is never reused.
    pub fn mint_id(&mut self) -> String {
        self.id_counter = self.id_counter.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let mut value = nanos
            .wrapping_mul(1_000_003)
            .wrapping_add(u128::from(self.id_counter));
        let mut digits = Vec::with_capacity(ID_WIDTH);
        for _ in 0..ID_WIDTH {
            digits.push(char::from(ID_ALPHABET[(value % 36) as usize]));
            value /= 36;
        }
        digits.into_iter().rev().collect()
    }

    /// Materialise every status transition the clock has already made due.
    ///
    /// Called first by every handler — including the reporting ones, which may write
    /// only this: the change was owed at the same instant whether or not anybody called.
    ///
    /// It walks only the records that actually owe a transition, so one call costs the
    /// number of transitions in flight, not the size of the store.
    pub fn settle(&mut self) {
        let now = Instant::now();
        for id in std::mem::take(&mut self.pending_clusters) {
            let still_pending = match self.virtual_clusters.get_mut(&id) {
                Some(record) => {
                    record.settle(now);
                    record.pending.is_some()
                }
                None => false,
            };
            if still_pending {
                self.pending_clusters.insert(id);
            }
        }
        for id in std::mem::take(&mut self.pending_job_runs) {
            let still_pending = match self.job_runs.get_mut(&id) {
                Some(record) => {
                    record.settle(now);
                    record.pending.is_some()
                }
                None => false,
            };
            if still_pending {
                self.pending_job_runs.insert(id);
            }
        }
    }

    /// Make room for one more virtual cluster, reclaiming the oldest RETIRED ones (a
    /// TERMINATED cluster holding no job run that has not finished, together with the
    /// finished runs it still holds) when the cap is reached.
    ///
    /// `false` when every slot holds a live cluster: the caller must then refuse the
    /// create rather than grow the store.
    pub fn reserve_virtual_cluster(&mut self) -> bool {
        while self.virtual_clusters.len() >= MAX_VIRTUAL_CLUSTERS {
            // A retired cluster is one that is TERMINATED and holds no job run that has
            // not finished. Its remaining (finished) runs are reclaimed WITH it, so a
            // cluster slot is never held hostage by a run that is already over.
            let retired = self
                .virtual_clusters
                .values()
                .filter(|record| {
                    record.state == CLUSTER_TERMINATED
                        && self
                            .cluster_job_runs(&record.id)
                            .all(|run| job_state_is_terminal(&run.state))
                })
                .min_by_key(|record| (record.created_at_millis, record.id.clone()))
                .map(|record| (record.id.clone(), record.client_token.clone()));
            let Some((id, client_token)) = retired else {
                return false;
            };
            for run_id in self.job_runs_by_cluster.remove(&id).unwrap_or_default() {
                self.pending_job_runs.remove(&run_id);
                if let Some(run) = self.job_runs.remove(&run_id) {
                    self.idempotency
                        .remove(&(start_job_run_scope(&id), run.client_token));
                }
            }
            self.virtual_clusters.remove(&id);
            self.pending_clusters.remove(&id);
            self.idempotency
                .remove(&(CREATE_VIRTUAL_CLUSTER_SCOPE.to_string(), client_token));
        }
        true
    }

    /// Make room for one more job run, reclaiming the oldest RETIRED ones (a run in a
    /// terminal state) when the cap is reached.
    ///
    /// `false` when every slot holds a run that has not finished.
    pub fn reserve_job_run(&mut self) -> bool {
        while self.job_runs.len() >= MAX_JOB_RUNS {
            let retired = self
                .job_runs
                .values()
                .filter(|record| job_state_is_terminal(&record.state))
                .min_by_key(|record| (record.created_at_millis, record.sequence))
                .map(|record| {
                    (
                        record.id.clone(),
                        record.virtual_cluster_id.clone(),
                        record.client_token.clone(),
                    )
                });
            let Some((id, virtual_cluster_id, client_token)) = retired else {
                return false;
            };
            self.job_runs.remove(&id);
            self.pending_job_runs.remove(&id);
            if let Some(ids) = self.job_runs_by_cluster.get_mut(&virtual_cluster_id) {
                ids.retain(|stored| stored != &id);
            }
            self.idempotency
                .remove(&(start_job_run_scope(&virtual_cluster_id), client_token));
        }
        true
    }

    /// Store a freshly minted virtual cluster, with the idempotency entry its
    /// `clientToken` earns and its pending transition (if any) registered. One function,
    /// so a record can never be stored without them.
    pub fn insert_virtual_cluster(&mut self, record: VirtualClusterRecord) {
        if record.pending.is_some() {
            self.pending_clusters.insert(record.id.clone());
        }
        self.idempotency.insert(
            (
                CREATE_VIRTUAL_CLUSTER_SCOPE.to_string(),
                record.client_token.clone(),
            ),
            record.id.clone(),
        );
        self.virtual_clusters.insert(record.id.clone(), record);
    }

    /// Store a freshly minted job run, with its per-cluster index entry, its idempotency
    /// entry and its pending transition.
    pub fn insert_job_run(&mut self, record: JobRunRecord) {
        if record.pending.is_some() {
            self.pending_job_runs.insert(record.id.clone());
        }
        self.job_runs_by_cluster
            .entry(record.virtual_cluster_id.clone())
            .or_default()
            .push(record.id.clone());
        self.idempotency.insert(
            (
                start_job_run_scope(&record.virtual_cluster_id),
                record.client_token.clone(),
            ),
            record.id.clone(),
        );
        self.job_runs.insert(record.id.clone(), record);
    }

    /// Make room for one more job template.
    ///
    /// `false` once every slot is taken: a job template has no lifecycle state, so none
    /// of them is ever retired and there is nothing to reclaim — the caller must refuse
    /// the create rather than grow the store, and a `DeleteJobTemplate` is what frees a
    /// slot.
    pub fn reserve_job_template(&self) -> bool {
        self.job_templates.len() < MAX_JOB_TEMPLATES
    }

    /// Store a freshly minted job template, with the idempotency entry its `clientToken`
    /// earns. One function, so a record can never be stored without it.
    pub fn insert_job_template(&mut self, record: JobTemplateRecord) {
        self.idempotency.insert(
            (
                CREATE_JOB_TEMPLATE_SCOPE.to_string(),
                record.client_token.clone(),
            ),
            record.id.clone(),
        );
        self.job_templates.insert(record.id.clone(), record);
    }

    /// Remove a stored job template and the idempotency entry that named it. `None` when
    /// the id names no template.
    ///
    /// The entry goes WITH the record, so this emulator's token index holds exactly one
    /// entry per stored template and is bounded by [`MAX_JOB_TEMPLATES`] — the same terms
    /// [`Self::reserve_virtual_cluster`] reclaims a virtual cluster's entry on. The
    /// consequence is that a create retried with the same `clientToken` AFTER its
    /// template was deleted mints a new one rather than replaying: EMR on EKS documents
    /// no retention window for a spent `clientToken`, so how long a token outlives the
    /// resource it named is not a modelled behaviour, and bounding the index is the
    /// emulator's own choice. While the template exists — the whole window the dedup
    /// contract is about — the token never mints a second one.
    pub fn remove_job_template(&mut self, id: &str) -> Option<JobTemplateRecord> {
        let record = self.job_templates.remove(id)?;
        self.idempotency.remove(&(
            CREATE_JOB_TEMPLATE_SCOPE.to_string(),
            record.client_token.clone(),
        ));
        Some(record)
    }

    /// The modelled `JobTemplate` for this id.
    pub fn job_template_view(&self, id: &str) -> Option<JobTemplate> {
        Some(self.job_templates.get(id)?.to_job_template())
    }

    /// Move a stored virtual cluster into `now_state` with `next_state` owed to the
    /// clock.
    pub fn move_virtual_cluster(
        &mut self,
        id: &str,
        now_state: &'static str,
        next_state: &'static str,
    ) {
        if let Some(record) = self.virtual_clusters.get_mut(id) {
            record.state = now_state.to_string();
            record.pending = Some(PendingTransition::due(next_state));
            self.pending_clusters.insert(id.to_string());
        }
    }

    /// Move a stored job run into `now_state` with `next_state` owed to the clock.
    pub fn move_job_run(&mut self, id: &str, now_state: &'static str, next_state: &'static str) {
        if let Some(record) = self.job_runs.get_mut(id) {
            record.state = now_state.to_string();
            record.pending = Some(PendingTransition::due(next_state));
            self.pending_job_runs.insert(id.to_string());
        }
    }

    /// The stored job run with this id, only when it really belongs to this cluster: a
    /// job-run id under the wrong virtual cluster is not found, not somebody else's run.
    pub fn job_run(&self, virtual_cluster_id: &str, id: &str) -> Option<&JobRunRecord> {
        self.job_runs
            .get(id)
            .filter(|record| record.virtual_cluster_id == virtual_cluster_id)
    }

    /// The modelled `VirtualCluster` for this id.
    pub fn virtual_cluster_view(&self, id: &str) -> Option<VirtualCluster> {
        Some(self.virtual_clusters.get(id)?.to_virtual_cluster())
    }

    /// This cluster's stored job runs, in creation order.
    pub fn cluster_job_runs(
        &self,
        virtual_cluster_id: &str,
    ) -> impl Iterator<Item = &JobRunRecord> {
        self.job_runs_by_cluster
            .get(virtual_cluster_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.job_runs.get(id))
    }

    /// Whether there is room for one more security configuration.
    ///
    /// A security configuration has no retired form to reclaim (it has no status, and only
    /// a delete removes one), so at the cap the create is refused instead of growing the
    /// store without bound.
    pub fn can_add_security_configuration(&self) -> bool {
        self.security_configurations.len() < MAX_SECURITY_CONFIGURATIONS
    }

    /// Store a freshly minted security configuration together with the idempotency entry
    /// its `clientToken` earns, so a record can never be stored without it. A delete that
    /// removes the record removes that entry with it.
    pub fn insert_security_configuration(&mut self, record: SecurityConfigurationRecord) {
        self.idempotency.insert(
            (
                CREATE_SECURITY_CONFIGURATION_SCOPE.to_string(),
                record.client_token.clone(),
            ),
            record.id.clone(),
        );
        self.security_configurations
            .insert(record.id.clone(), record);
    }

    /// The modelled `SecurityConfiguration` for this id.
    pub fn security_configuration_view(&self, id: &str) -> Option<SecurityConfiguration> {
        Some(
            self.security_configurations
                .get(id)?
                .to_security_configuration(),
        )
    }

    /// The resource id `(scope, client_token)` already minted, if this token is a replay.
    pub fn replayed_idempotency(&self, scope: &str, client_token: &str) -> Option<&String> {
        self.idempotency
            .get(&(scope.to_string(), client_token.to_string()))
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEmrcontainersAwschicagowebserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEmrcontainersAwschicagowebserviceState>)
}

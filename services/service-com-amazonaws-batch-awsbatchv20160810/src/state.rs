// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-batch-awsbatchv20160810 service.
//!
//! Every implemented operation computes its answer from the records below: the
//! compute environments, job queues, job definition revisions and jobs this
//! service has been asked to create, keyed the way AWS Batch keys them (by name,
//! with an ARN index beside it so anything the model documents as "name or ARN"
//! resolves both ways).
//!
//! # The job lifecycle is driven by the CLOCK, never by a read
//!
//! A job AWS Batch accepts is returned in a non-terminal status (`SUBMITTED`),
//! so the moves out of it are owed by the clock: each one falls due
//! [`JOB_PHASE_MILLIS`] after the call (or the transition) that started it, and
//! [`ComAmazonawsBatchAwsbatchv20160810State::materialize_due`] only materialises
//! what is already due. A reporting operation (`DescribeJobs`, `ListJobs`, the
//! `Describe*` readers) calls it and therefore never changes what a later caller
//! sees: two reads at the same instant answer identically, and the transient
//! status is observable because the span is never zero.
//!
//! # Everything a caller can fill is BOUNDED
//!
//! Every map below is filled by a request anyone can repeat, so each one has a maximum
//! ([`limits`]): a create past the cap answers the modelled `ClientException`, and the
//! jobs store evicts the jobs that have been terminal longest before it refuses, so the
//! store cannot grow for the emulator's lifetime.
//!
//! A job's container is NOT run: this emulator has no container runtime, and the
//! engine contract (`vendor/engine/src/lib.rs`) forbids reporting a status,
//! duration or exit code for work nothing ran. So the run the caller asked for
//! ends in the modelled failure state with [`NO_CONTAINER_RUNTIME_REASON`] as its
//! `statusReason`, and no `startedAt`, attempt or `exitCode` is invented. Everything
//! around it — accepting, resolving, scheduling, listing, terminating — is real.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    CapacityLimit, ClientException, ComputeEnvironmentOrder, ComputeResource,
    ConsumableResourceProperties, ContainerDetail, ContainerOverrides, ContainerProperties,
    EcsProperties, EcsSettings, EksConfiguration, JobDefinition, JobDependency,
    JobStateTimeLimitAction, JobTimeout, KeyValuePair, NodeProperties, ResourceRequirement,
    RetryStrategy, ServerException, ServiceEnvironmentOrder,
};

/// This service's ARN namespace and sigv4 signing name (`OPERATIONS.md`).
pub const ARN_SERVICE: &str = "batch";

/// The region used when a request carries no SigV4 credential scope to read one
/// from. Every emulator defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long one clock-owed transition takes: a compute environment or job queue
/// leaving `CREATING`, and each move a submitted job makes.
///
/// A span chosen by this emulator (AWS documents no duration for it), inside the
/// 250–750 ms band the project fixes for a transition the clock owes. It is never
/// zero, so a caller that reads straight after a create still observes `CREATING`,
/// and a caller that terminates straight after a submit still observes `SUBMITTED`.
pub const JOB_PHASE_MILLIS: i64 = 500;

/// What this emulator will store for one caller, and the answer past it.
///
/// Every map in [`ComAmazonawsBatchAwsbatchv20160810State`] is filled by requests an
/// unauthenticated caller can repeat, so each one carries a maximum: a create past the
/// cap answers the modelled `ClientException` (HTTP 400), the way AWS Batch answers a
/// request past a service quota, and the jobs store additionally evicts the jobs that
/// have been terminal longest (nothing a later call can move again), so a long-running
/// emulator cannot accumulate records without bound. The indexes beside those maps are
/// bounded by them: the due index and the dependency index hold only entries for jobs the
/// store still holds, and an entry goes when either end of it does.
pub mod limits {
    /// AWS Batch service quota "Compute environments per Region": 50 (adjustable).
    pub const MAX_COMPUTE_ENVIRONMENTS: usize = 50;

    /// AWS Batch service quota "Job queues per Region": 50 (adjustable).
    pub const MAX_JOB_QUEUES: usize = 50;

    /// Service environments kept at once. An EMULATOR bound (AWS publishes no quota for
    /// this resource): `CreateServiceEnvironment` fills the store and anyone can repeat
    /// it, so a create past this answers the modelled `ClientException`. Sized like the
    /// other per-Region, name-keyed Batch resources above.
    pub const MAX_SERVICE_ENVIRONMENTS: usize = 50;

    /// Dependencies one submit may name. AWS Batch documents it on `SubmitJob$dependsOn`:
    /// "A job can depend upon a maximum of 20 jobs."
    pub const MAX_JOB_DEPENDENCIES: usize = 20;

    /// Jobs kept at once. An emulator bound, as above: a submit that would exceed it
    /// first evicts the jobs that have been terminal longest, and is refused only when
    /// there is nothing evictable left.
    pub const MAX_JOBS: usize = 500;
}

/// The `ClientException` message a request past one of [`limits`] answers with.
pub fn quota_message(resource: &str, maximum: usize) -> String {
    format!("Too many {resource}: this emulator stores at most {maximum}")
}

/// `statusReason` of a job whose container could not be run here.
pub const NO_CONTAINER_RUNTIME_REASON: &str =
    "This emulator has no container runtime, so the job's container was never started";

/// `statusReason` of a job released by a dependency that failed.
pub const DEPENDENT_JOB_FAILED_REASON: &str = "Dependent Job failed";

/// `statusReason` of a job none of whose dependencies the service can resolve any more.
///
/// `SubmitJob` refuses a `dependsOn` entry naming a job that does not exist, and eviction
/// keeps a dependency a live job still waits on, so nothing reachable through the API
/// produces this. It exists so a job that COULD not be released can still reach a
/// terminal status instead of occupying the store for ever.
pub const DEPENDENCY_UNRESOLVABLE_REASON: &str =
    "A job this job depends on is no longer known to the service";

/// Message of the `ServerException` a handler answers when the engine does not
/// hand back this service's store.
pub const UNREACHABLE_STATE: &str = "the Batch emulator could not reach its service state";

// Status/state literals. The values come from the model's own enums
// (`C_E_STATUS_VALUES`, `J_Q_STATUS_VALUES`, `JOB_STATUS_VALUES`, `CEState`/`JQState`).
/// `CEStatus`/`JQStatus` value a resource carries while its creation is pending.
pub const STATUS_CREATING: &str = "CREATING";
/// `CEStatus`/`JQStatus` value of a created resource.
pub const STATUS_VALID: &str = "VALID";
/// `CEState`/`JQState` default: a resource is created enabled unless asked otherwise.
pub const STATE_ENABLED: &str = "ENABLED";
/// `JobDefinition$status` of a revision `RegisterJobDefinition` created and nothing has
/// deregistered.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// `JobDefinition$status` of a deregistered revision. `DescribeJobDefinitions`'
/// `status` filter is matched against stored status values, and
/// `DeregisterJobDefinition` is what sets this one.
pub const STATUS_INACTIVE: &str = "INACTIVE";

/// `OrchestrationType` of a job definition whose work runs on Amazon ECS — the default,
/// per the `containerOrchestrationType` doc ("The valid values are `ECS` (default) or
/// `EKS`").
pub const ORCHESTRATION_TYPE_ECS: &str = "ECS";
/// `OrchestrationType` of a job definition whose work runs on Amazon EKS.
pub const ORCHESTRATION_TYPE_EKS: &str = "EKS";

/// `PlatformCapability` for jobs on Amazon EC2 — the default when the request names none
/// (`platformCapabilities` doc: "If no value is specified, it defaults to EC2").
pub const PLATFORM_CAPABILITY_EC2: &str = "EC2";
/// `PlatformCapability` for jobs on Fargate.
pub const PLATFORM_CAPABILITY_FARGATE: &str = "FARGATE";
/// `PlatformCapability` for jobs on Amazon ECS Managed Instances.
pub const PLATFORM_CAPABILITY_MANAGED_INSTANCES: &str = "MANAGED_INSTANCES";

/// The longest `jobDefinitionName` the service accepts, from the member's doc: "It can be
/// up to 128 letters long. It can contain uppercase and lowercase letters, numbers,
/// hyphens (-), and underscores (_)." The model types the member as a plain `String`, so
/// the framework cannot refuse a bad one.
pub const MAX_JOB_DEFINITION_NAME_LEN: usize = 128;

/// The longest `serviceEnvironmentName` the service accepts, from the member's doc: "The
/// name for the service environment. It can be up to 128 characters long and can contain
/// letters, numbers, hyphens (-), and underscores (_)." The model types the member as a
/// plain `String`, so the framework cannot refuse a bad one.
pub const MAX_SERVICE_ENVIRONMENT_NAME_LEN: usize = 128;

/// The lowest `schedulingPriority` the service accepts, from the member's doc: "The
/// minimum supported value is 0 and the maximum supported value is 9999."
pub const MIN_SCHEDULING_PRIORITY: i32 = 0;
/// The highest `schedulingPriority` the service accepts; see [`MIN_SCHEDULING_PRIORITY`].
pub const MAX_SCHEDULING_PRIORITY: i32 = 9999;

/// The shortest job timeout the service accepts, in seconds, from the `timeout` member's
/// doc: "The minimum value for the timeout is 60 seconds."
pub const MIN_ATTEMPT_DURATION_SECONDS: i32 = 60;

/// The most job definitions `DescribeJobDefinitions` accepts in one request, from the
/// `jobDefinitions` member's doc: "A list of up to 100 job definitions."
pub const MAX_DESCRIBE_FILTER_ENTRIES: usize = 100;

/// The page-size bounds `DescribeJobDefinitions` accepts, from the `maxResults` member's
/// doc: "This value can be between 1 and 100. If this parameter isn't used, then
/// `DescribeJobDefinitions` returns up to 100 results".
pub const MIN_DESCRIBE_PAGE_SIZE: i32 = 1;
/// See [`MIN_DESCRIBE_PAGE_SIZE`]; also the page size used when the caller names no
/// `maxResults`.
pub const MAX_DESCRIBE_PAGE_SIZE: i32 = 100;

/// The most job-definition revisions this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota: the store is in memory and every
/// `RegisterJobDefinition` adds to it, so the registry answers the modelled
/// `ClientException` past this point rather than growing without bound. AWS enforces a
/// per-account job-definition quota of its own.
pub const MAX_JOB_DEFINITIONS: usize = 5_000;

/// `CEType` value whose compute resources AWS Batch manages.
pub const CE_TYPE_MANAGED: &str = "MANAGED";

/// `JobStatus` of a job the service has accepted but not yet scheduled.
pub const JOB_SUBMITTED: &str = "SUBMITTED";
/// `JobStatus` of a job waiting on the jobs it depends on.
pub const JOB_PENDING: &str = "PENDING";
/// `JobStatus` of a job the scheduler may place.
pub const JOB_RUNNABLE: &str = "RUNNABLE";
/// `JobStatus` of a job whose container is being started.
pub const JOB_STARTING: &str = "STARTING";
/// `JobStatus` of a job whose container is running.
pub const JOB_RUNNING: &str = "RUNNING";
/// Terminal `JobStatus`: the job's container exited successfully.
pub const JOB_SUCCEEDED: &str = "SUCCEEDED";
/// Terminal `JobStatus`: the job did not complete successfully.
pub const JOB_FAILED: &str = "FAILED";

/// A fresh identifier in the shape AWS Batch gives a job id and a compute environment
/// `uuid`: a version-4 UUID, `8-4-4-4-12` lowercase hex with the version and variant bits
/// RFC 4122 §4.4 fixes.
///
/// The 128 bits come from three sources hashed together with SHA-256 (the `sha2` crate)
/// and hex-encoded (the `hex` crate): the OS-seeded randomness a `RandomState` draws, the
/// system clock in nanoseconds, and `sequence` — a counter the store hands out, so two
/// calls in the same nanosecond cannot collide. An id names a resource for correlation; a
/// credential would come from `ctx.credential_store()` instead, never from here.
pub fn new_uuid(sequence: u64) -> String {
    use sha2::{Digest, Sha256};
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(RandomState::new().hash_one(sequence).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(sequence.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version nibble is 4, and the variant's two top bits are `10`.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Epoch milliseconds, the unit every `createdAt`/`startedAt`/`stoppedAt` member
/// of this service carries.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

/// The region this request was signed for, which is the region of every ARN the
/// request mints. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// `arn:aws:batch:<region>:<account>:compute-environment/<name>`.
pub fn compute_environment_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("compute-environment/{name}"))
}

/// `arn:aws:batch:<region>:<account>:job-queue/<name>`.
pub fn job_queue_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("job-queue/{name}"))
}

/// `arn:aws:batch:<region>:<account>:job-definition/<name>:<revision>`.
pub fn job_definition_arn(region: &str, name: &str, revision: i32) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("job-definition/{name}:{revision}"),
    )
}

/// `arn:aws:batch:<region>:<account>:job/<job id>`.
pub fn job_arn(region: &str, job_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("job/{job_id}"))
}

/// `arn:aws:batch:<region>:<account>:service-environment/<name>`.
pub fn service_environment_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("service-environment/{name}"),
    )
}

/// The resource part of an ARN (everything after the account field), or `None`
/// when `value` is not an ARN at all.
fn arn_resource(value: &str) -> Option<&str> {
    if !value.starts_with("arn:") {
        return None;
    }
    value.splitn(6, ':').nth(5)
}

/// The resource id an ARN of this `kind` names (`compute-environment`,
/// `job-queue`, `job-definition`, `job`, `service-environment`), or `None` when `value`
/// is not an ARN of that kind. A plain name is passed through by the callers below,
/// which is how every "name or ARN" member resolves both ways.
fn arn_resource_id<'a>(value: &'a str, kind: &str) -> Option<&'a str> {
    arn_resource(value)?.strip_prefix(kind)?.strip_prefix('/')
}

/// A compute environment, exactly as `CreateComputeEnvironment` stored it.
#[derive(Debug, Clone)]
pub struct ComputeEnvironment {
    pub name: String,
    pub arn: String,
    pub ce_type: String,
    pub state: String,
    pub status: String,
    pub status_reason: Option<String>,
    /// Set while the `CREATING` -> `VALID` move is still owed by the clock.
    pub status_due_at: Option<i64>,
    /// The environment's unique identifier, minted once at creation.
    pub uuid: String,
    pub unmanagedv_cpus: Option<i32>,
    pub compute_resources: Option<ComputeResource>,
    pub service_role: Option<String>,
    pub eks_configuration: Option<EksConfiguration>,
    pub ecs_settings: Option<EcsSettings>,
    pub context: Option<String>,
    pub tags: Option<HashMap<String, String>>,
}

/// A service environment, exactly as `CreateServiceEnvironment` stored it.
///
/// The name is the environment's identity and the ARN is minted once beside it, so every
/// later operation reports the same one. AWS Batch returns a fresh service environment in
/// the non-terminal `CREATING` status, so the move to `VALID` is owed by the clock
/// ([`ServiceEnvironment::status_due_at`]).
#[derive(Debug, Clone)]
pub struct ServiceEnvironment {
    pub name: String,
    pub arn: String,
    /// `ServiceEnvironmentType` (the model's only value today is `SAGEMAKER_TRAINING`).
    pub environment_type: String,
    /// `ServiceEnvironmentState`: `ENABLED` unless the create asked for `DISABLED`.
    pub state: String,
    /// `ServiceEnvironmentStatus`.
    pub status: String,
    /// Set while the `CREATING` -> `VALID` move is still owed by the clock.
    pub status_due_at: Option<i64>,
    pub capacity_limits: Vec<CapacityLimit>,
    pub tags: Option<HashMap<String, String>>,
}

/// A job queue, exactly as `CreateJobQueue` stored it. Its
/// `compute_environment_order` holds the ARN each entry resolved to, so a queue
/// created by compute environment NAME reports the environment's ARN the way AWS
/// Batch does.
#[derive(Debug, Clone)]
pub struct JobQueue {
    pub name: String,
    pub arn: String,
    pub state: String,
    pub status: String,
    pub status_reason: Option<String>,
    /// Set while the `CREATING` -> `VALID` move is still owed by the clock.
    pub status_due_at: Option<i64>,
    pub priority: i32,
    pub scheduling_policy_arn: Option<String>,
    pub compute_environment_order: Vec<ComputeEnvironmentOrder>,
    pub service_environment_order: Option<Vec<ServiceEnvironmentOrder>>,
    pub job_queue_type: Option<String>,
    pub tags: Option<HashMap<String, String>>,
    pub job_state_time_limit_actions: Option<Vec<JobStateTimeLimitAction>>,
}

/// A submitted job: the request's own members, the definition it resolved to, and
/// the effective container the definition and the overrides add up to.
#[derive(Debug, Clone)]
pub struct Job {
    pub job_id: String,
    pub job_arn: String,
    pub job_name: String,
    pub job_queue_arn: String,
    pub job_definition_arn: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub stopped_at: Option<i64>,
    /// When the next clock-owed transition falls due; `None` while the job waits
    /// on a dependency, and once it is terminal.
    pub next_phase_due_at: Option<i64>,
    pub is_cancelled: bool,
    pub is_terminated: bool,
    /// The `reason` of a `CancelJob` that arrived while this job was `PENDING`, i.e.
    /// still waiting on the jobs it depends on.
    ///
    /// `CancelJob`'s own doc defers that one case — "A PENDING job is cancelled after all
    /// dependency jobs are completed. Therefore, it might take longer than expected to
    /// cancel a job in PENDING status." — so the request is RECORDED here and
    /// materialised by the clock, when
    /// [`ComAmazonawsBatchAwsbatchv20160810State::advance_job`] next finds the
    /// dependencies complete. `None` for every other job.
    pub cancel_requested: Option<String>,
    pub parameters: Option<HashMap<String, String>>,
    pub container: Option<ContainerDetail>,
    pub node_properties: Option<NodeProperties>,
    pub retry_strategy: Option<RetryStrategy>,
    pub timeout: Option<JobTimeout>,
    pub depends_on: Option<Vec<JobDependency>>,
    pub share_identifier: Option<String>,
    pub scheduling_priority: Option<i32>,
    pub propagate_tags: Option<bool>,
    pub platform_capabilities: Option<Vec<String>>,
    pub consumable_resource_properties: Option<ConsumableResourceProperties>,
    pub tags: Option<HashMap<String, String>>,
}

/// Whether a `JobStatus` is one a job never leaves.
pub fn is_terminal_status(status: &str) -> bool {
    matches!(status, JOB_SUCCEEDED | JOB_FAILED)
}

/// What a job's dependencies allow right now.
enum DependencyOutcome {
    /// At least one dependency has not finished.
    Waiting,
    /// A dependency names a job the store does not hold, so the job can never be released.
    Unresolvable,
    /// Every dependency succeeded; the value is the instant the last one stopped.
    Satisfied(i64),
    /// A dependency failed; the value is the instant it stopped.
    Failed(i64),
}

/// What a `CancelJob` call did to the job it named.
///
/// Every variant is a SUCCESS for the operation — AWS Batch answers `CancelJob` 200
/// "even if no job is cancelled" — so this reports what happened to the job rather than
/// whether the call is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The store holds no job with that id, so there was nothing to cancel.
    Unknown,
    /// The job had not progressed past `RUNNABLE`: it is now `FAILED` and `isCancelled`.
    Cancelled,
    /// The job is `PENDING` on the jobs it depends on, so the cancel is recorded and
    /// falls due when they complete (see [`Job::cancel_requested`]).
    Deferred,
    /// The job reached `STARTING` or `RUNNING`, which `CancelJob` does not cancel: it is
    /// left running, for `TerminateJob` to end.
    NotCancelled,
    /// The job is already in a terminal status, and is left exactly as it is.
    AlreadyTerminal,
}

/// What materialising one owed transition did to a job.
enum Advanced {
    /// The job moved and is not terminal: it may owe another transition at this instant.
    Moved,
    /// The job reached a terminal status: the jobs waiting on it may now move.
    Terminal,
}

/// The request id a refusal names when the response carries none.
///
/// Batch's frontend wraps a client-side refusal together with the id of the
/// request it refused (see [`client_exception_message`]). This emulator's
/// restJson1 error path returns no request id to the caller — the framework sets
/// no request-id header on a JSON error — so there is no id to name and the
/// segment renders this placeholder, which is what a client that reads the
/// response's request id reports for an absent one.
const ABSENT_REQUEST_ID: &str = "None";

/// How Batch words a `ClientException` its frontend raises: the detail of what
/// was wrong, wrapped in `Error executing request, Exception : <detail>,
/// RequestId: <id>`.
///
/// Every refusal this service raises for a request it understood but cannot
/// accept goes through here, so one wording rule covers all of them.
pub fn client_exception_message(detail: &str) -> String {
    format!("Error executing request, Exception : {detail}, RequestId: {ABSENT_REQUEST_ID}")
}

/// Whether `value` is a name AWS Batch accepts for a resource it keys by name: a
/// non-empty string of at most `max_len` letters, numbers, hyphens and underscores.
///
/// Every Batch resource whose name member documents that charset shares this rule
/// (`jobDefinitionName`, `serviceEnvironmentName`, …), and the model types each of them
/// as a plain `String`, so the framework admits anything and the rule is applied in the
/// handler.
pub fn is_valid_resource_name(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.chars().count() <= max_len
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `value` is a `jobDefinitionName` the service accepts: up to
/// [`MAX_JOB_DEFINITION_NAME_LEN`] letters, numbers, hyphens and underscores
/// (the member's own doc). See [`is_valid_resource_name`].
pub fn is_valid_job_definition_name(value: &str) -> bool {
    is_valid_resource_name(value, MAX_JOB_DEFINITION_NAME_LEN)
}

/// Whether `value` is a `serviceEnvironmentName` the service accepts: up to
/// [`MAX_SERVICE_ENVIRONMENT_NAME_LEN`] letters, numbers, hyphens and underscores
/// (the member's own doc). See [`is_valid_resource_name`].
pub fn is_valid_service_environment_name(value: &str) -> bool {
    is_valid_resource_name(value, MAX_SERVICE_ENVIRONMENT_NAME_LEN)
}

/// Encode a `DescribeJobDefinitions` cursor: the `(name, revision)` key of the
/// first item of the NEXT page.
///
/// Hex-encoded so the token is opaque to the caller, as the member's doc asks
/// ("Treat this token as an opaque identifier").
pub fn encode_page_token(name: &str, revision: i32) -> String {
    hex::encode(format!("{name}:{revision}"))
}

/// Decode a cursor produced by [`encode_page_token`]; `None` when the token was
/// not produced by this service.
///
/// A job-definition name holds no `:` ([`is_valid_job_definition_name`]), so the
/// LAST `:` separates the name from the revision unambiguously.
pub fn decode_page_token(token: &str) -> Option<(String, i32)> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (name, revision) = decoded.rsplit_once(':')?;
    Some((name.to_string(), revision.parse().ok()?))
}

/// The execution role of EVERY container-properties object a job definition
/// carries, one entry per object, in declaration order: the top-level
/// `containerProperties`, the `container` and the `ecsProperties` tasks of each
/// node range of a multi-node definition, and each `ecsProperties` task (which is
/// where an ECS task carries the role for its containers).
///
/// One traversal, so a rule about the execution role — "For jobs that run on
/// Fargate resources, you must provide an execution role" — is applied to every
/// place a job definition can carry container properties rather than only to the
/// top-level member.
pub fn execution_roles_of<'a>(
    container_properties: Option<&'a ContainerProperties>,
    node_properties: Option<&'a NodeProperties>,
    ecs_properties: Option<&'a EcsProperties>,
) -> Vec<Option<&'a str>> {
    let mut roles = Vec::new();
    if let Some(container) = container_properties {
        roles.push(container.execution_role_arn.as_deref());
    }
    if let Some(node) = node_properties {
        for range in &node.node_range_properties {
            if let Some(container) = range.container.as_ref() {
                roles.push(container.execution_role_arn.as_deref());
            }
            for task in range
                .ecs_properties
                .as_ref()
                .map(|ecs| ecs.task_properties.as_slice())
                .unwrap_or_default()
            {
                roles.push(task.execution_role_arn.as_deref());
            }
        }
    }
    if let Some(ecs) = ecs_properties {
        for task in &ecs.task_properties {
            roles.push(task.execution_role_arn.as_deref());
        }
    }
    roles
}

/// Per-service state for com-amazonaws-batch-awsbatchv20160810. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBatchAwsbatchv20160810State {
    /// Compute environments by name.
    pub compute_environments: HashMap<String, ComputeEnvironment>,
    /// Compute environment ARN -> name, so `name or ARN` members resolve both ways.
    pub compute_environments_by_arn: HashMap<String, String>,
    /// Job queues by name.
    pub job_queues: HashMap<String, JobQueue>,
    /// Job queue ARN -> name.
    pub job_queues_by_arn: HashMap<String, String>,
    /// Service environments by name. Bounded by [`limits::MAX_SERVICE_ENVIRONMENTS`].
    pub service_environments: HashMap<String, ServiceEnvironment>,
    /// Service environment ARN -> name, so `name or ARN` members resolve both ways.
    pub service_environments_by_arn: HashMap<String, String>,
    /// Registered job-definition revisions, keyed by `(jobDefinitionName, revision)` —
    /// the describe and pagination order. Bounded by [`MAX_JOB_DEFINITIONS`].
    pub job_definitions: BTreeMap<(String, i32), JobDefinition>,
    /// Jobs by job id.
    pub jobs: HashMap<String, Job>,
    /// Job ids per queue ARN, in submission order, backing `ListJobs`.
    pub jobs_by_queue: HashMap<String, Vec<String>>,
    /// The jobs with a clock-owed transition still pending, ordered by the instant it
    /// falls due, so [`ComAmazonawsBatchAwsbatchv20160810State::materialize_due`] visits
    /// the jobs that are actually due instead of re-scanning the store.
    jobs_due: BTreeSet<(i64, String)>,
    /// Job ids waiting on a job, keyed by the job id they wait on: when a job reaches a
    /// terminal status, only the jobs that depend on it are re-examined.
    dependents: HashMap<String, Vec<String>>,
    /// How many identifiers this store has minted, so two ids are distinct even when the
    /// clock reports the same nanosecond (see [`new_uuid`]).
    minted_ids: u64,
}

impl ComAmazonawsBatchAwsbatchv20160810State {
    /// A fresh identifier for a resource this store is about to hold: the UUID shape AWS
    /// Batch uses, over this store's own counter (see [`new_uuid`]).
    pub fn new_uuid(&mut self) -> String {
        self.minted_ids = self.minted_ids.wrapping_add(1);
        new_uuid(self.minted_ids)
    }
    /// The compute environment `name_or_arn` names, resolved by name or by ARN.
    pub fn compute_environment(&self, name_or_arn: &str) -> Option<&ComputeEnvironment> {
        let name = self.compute_environment_name(name_or_arn)?;
        self.compute_environments.get(&name)
    }

    /// The NAME of the compute environment `name_or_arn` names.
    pub fn compute_environment_name(&self, name_or_arn: &str) -> Option<String> {
        if let Some(name) = self.compute_environments_by_arn.get(name_or_arn) {
            return Some(name.clone());
        }
        if self.compute_environments.contains_key(name_or_arn) {
            return Some(name_or_arn.to_string());
        }
        let id = arn_resource_id(name_or_arn, "compute-environment")?;
        self.compute_environments
            .contains_key(id)
            .then(|| id.to_string())
    }

    /// The job queue `name_or_arn` names, resolved by name or by ARN.
    pub fn job_queue(&self, name_or_arn: &str) -> Option<&JobQueue> {
        let name = self.job_queue_name(name_or_arn)?;
        self.job_queues.get(&name)
    }

    /// The NAME of the job queue `name_or_arn` names.
    pub fn job_queue_name(&self, name_or_arn: &str) -> Option<String> {
        if let Some(name) = self.job_queues_by_arn.get(name_or_arn) {
            return Some(name.clone());
        }
        if self.job_queues.contains_key(name_or_arn) {
            return Some(name_or_arn.to_string());
        }
        let id = arn_resource_id(name_or_arn, "job-queue")?;
        self.job_queues.contains_key(id).then(|| id.to_string())
    }

    /// The service environment `name_or_arn` names, resolved by name or by ARN.
    pub fn service_environment(&self, name_or_arn: &str) -> Option<&ServiceEnvironment> {
        let name = self.service_environment_name(name_or_arn)?;
        self.service_environments.get(&name)
    }

    /// The NAME of the service environment `name_or_arn` names.
    pub fn service_environment_name(&self, name_or_arn: &str) -> Option<String> {
        if let Some(name) = self.service_environments_by_arn.get(name_or_arn) {
            return Some(name.clone());
        }
        if self.service_environments.contains_key(name_or_arn) {
            return Some(name_or_arn.to_string());
        }
        let id = arn_resource_id(name_or_arn, "service-environment")?;
        self.service_environments
            .contains_key(id)
            .then(|| id.to_string())
    }

    /// The revision `RegisterJobDefinition` gives the next registration of `name`: one
    /// past the highest revision stored for it, and 1 for a name nothing is stored for. A
    /// revision is never reused, so deregistering the highest one does not hand its
    /// number out again.
    pub fn next_revision(&self, name: &str) -> i32 {
        self.revisions_of(name)
            .max()
            .map_or(1, |highest| highest.saturating_add(1))
    }

    /// Every stored revision number of `name`, ascending.
    pub fn revisions_of<'a>(&'a self, name: &'a str) -> impl Iterator<Item = i32> + 'a {
        self.job_definitions
            .keys()
            .filter(move |(stored, _)| stored == name)
            .map(|(_, revision)| *revision)
    }

    /// The job-definition revision a `name`, `name:revision` or ARN reference names (with
    /// or without the revision, the forms `SubmitJob$jobDefinition` documents): the exact
    /// revision when one is given, else "the latest active revision".
    pub fn job_definition(&self, reference: &str) -> Option<&JobDefinition> {
        let (name, revision) = parse_job_definition_reference(reference)?;
        match revision {
            Some(revision) => self.job_definitions.get(&(name.to_string(), revision)),
            None => self
                .revisions_of(name)
                .filter_map(|revision| self.job_definitions.get(&(name.to_string(), revision)))
                .filter(|definition| {
                    definition.status.as_deref() == Some(STATUS_ACTIVE)
                })
                .max_by_key(|definition| definition.revision),
        }
    }

    /// The `(name, revision)` key of the STORED revision a reference names, whatever
    /// its status.
    ///
    /// The forms are the ones a "name and revision (`name:revision`) or full ARN"
    /// member documents; a reference carrying no revision names the definition's
    /// LATEST revision, which is the highest revision stored for the name. Unlike
    /// [`Self::job_definition`] — which resolves a reference a job is to RUN under, so
    /// a bare name there means the latest ACTIVE revision — this resolves a reference
    /// to a revision as a record, so an already-`INACTIVE` revision still answers to
    /// it and `DeregisterJobDefinition` stays idempotent.
    ///
    /// `None` when no stored revision answers to the reference.
    pub fn job_definition_key(&self, reference: &str) -> Option<(String, i32)> {
        let (name, revision) = parse_job_definition_reference(reference)?;
        match revision {
            Some(revision) => {
                let key = (name.to_string(), revision);
                self.job_definitions.contains_key(&key).then_some(key)
            }
            None => self
                .revisions_of(name)
                .max()
                .map(|revision| (name.to_string(), revision)),
        }
    }

    /// The STORED revision a reference names, mutably: [`Self::job_definition_key`]
    /// followed by the lookup it proved, so a caller that must WRITE the record (the
    /// status `DeregisterJobDefinition` sets) resolves and mutates in one step
    /// instead of resolving a key and then performing a second lookup that could
    /// miss.
    pub fn job_definition_mut(&mut self, reference: &str) -> Option<&mut JobDefinition> {
        let key = self.job_definition_key(reference)?;
        self.job_definitions.get_mut(&key)
    }

    /// Materialise every transition the clock already owes, and nothing else.
    ///
    /// Safe for a reporting operation to call: it writes only what was due at
    /// `now` whether or not anybody called, so a later caller sees exactly what
    /// this caller saw.
    ///
    /// The work one call does is bounded. The compute environment, job queue and service
    /// environment stores are capped by [`limits::MAX_COMPUTE_ENVIRONMENTS`],
    /// [`limits::MAX_JOB_QUEUES`] and [`limits::MAX_SERVICE_ENVIRONMENTS`],
    /// and the jobs are reached through the due index rather than by scanning: a call
    /// materialises the transitions that have actually fallen due — each of which happens
    /// once in a job's life — plus the jobs waiting on a job that just finished.
    pub fn materialize_due(&mut self, now: i64) {
        for environment in self.compute_environments.values_mut() {
            if environment.status_due_at.is_some_and(|due| due <= now) {
                environment.status = STATUS_VALID.to_string();
                environment.status_due_at = None;
            }
        }
        for queue in self.job_queues.values_mut() {
            if queue.status_due_at.is_some_and(|due| due <= now) {
                queue.status = STATUS_VALID.to_string();
                queue.status_due_at = None;
            }
        }
        for environment in self.service_environments.values_mut() {
            if environment.status_due_at.is_some_and(|due| due <= now) {
                environment.status = STATUS_VALID.to_string();
                environment.status_due_at = None;
            }
        }
        self.advance_jobs(now);
    }

    /// Advance the jobs the clock owes a transition at `now`.
    ///
    /// Seeded from the due index, so no job that is not due is even looked at. Each pop
    /// materialises at most ONE transition; a job is re-queued only when it really moved,
    /// and a job waiting on a dependency is queued only when that dependency reaches a
    /// terminal status. The total work is therefore proportional to the transitions that
    /// fell due, not to the size of the store.
    fn advance_jobs(&mut self, now: i64) {
        let mut work: VecDeque<String> = self
            .jobs_due
            .iter()
            .take_while(|(due, _)| *due <= now)
            .map(|(_, id)| id.clone())
            .collect();
        while let Some(id) = work.pop_front() {
            match self.advance_job(&id, now) {
                None => {}
                Some(Advanced::Terminal) => {
                    if let Some(waiting) = self.dependents.get(&id) {
                        work.extend(waiting.iter().cloned());
                    }
                }
                Some(Advanced::Moved) => work.push_back(id),
            }
        }
    }

    /// Materialise the ONE transition `id` owes at `now`: `None` when nothing moved,
    /// else whether the job is now in a terminal status.
    fn advance_job(&mut self, id: &str, now: i64) -> Option<Advanced> {
        let job = self.jobs.get(id)?;
        if is_terminal_status(&job.status) {
            return None;
        }
        // What a PENDING job owes is decided by the jobs it depends on, recomputed from
        // their stop instants on every look, so the answer never depends on WHEN it was
        // looked at: released once they have all succeeded, failed when one failed, and
        // out of the due index again while any of them is still running. Its due instant
        // is only a wake-up hint, set by `finish_job` when a dependency finishes.
        if job.status == JOB_PENDING {
            // A `CancelJob` that arrived while the job was PENDING falls due with the
            // jobs it waits on, not with the call: whatever they completed WITH, the
            // job then ends FAILED carrying the caller's reason (`CancelJob`'s doc).
            if job.cancel_requested.is_some() {
                return match self.dependency_outcome(id, now) {
                    DependencyOutcome::Waiting => {
                        self.set_job_due(id, None);
                        None
                    }
                    // Nothing the API exposes reaches this (see
                    // `DEPENDENCY_UNRESOLVABLE_REASON`); the cancel the caller asked for
                    // is what ends the job, so it is what the job records.
                    DependencyOutcome::Unresolvable => {
                        self.finish_cancelled(id, now);
                        Some(Advanced::Terminal)
                    }
                    DependencyOutcome::Failed(at) | DependencyOutcome::Satisfied(at) => {
                        self.finish_cancelled(id, at);
                        Some(Advanced::Terminal)
                    }
                };
            }
            return match self.dependency_outcome(id, now) {
                DependencyOutcome::Waiting => {
                    self.set_job_due(id, None);
                    None
                }
                DependencyOutcome::Unresolvable => {
                    self.finish_job(id, JOB_FAILED, DEPENDENCY_UNRESOLVABLE_REASON, now);
                    Some(Advanced::Terminal)
                }
                DependencyOutcome::Failed(at) => {
                    self.finish_job(id, JOB_FAILED, DEPENDENT_JOB_FAILED_REASON, at);
                    Some(Advanced::Terminal)
                }
                DependencyOutcome::Satisfied(at) => {
                    // The job may be placed one span after the last dependency stopped.
                    let released = at + JOB_PHASE_MILLIS;
                    if released <= now {
                        self.set_job_status(id, JOB_RUNNABLE);
                        self.set_job_due(id, Some(released + JOB_PHASE_MILLIS));
                        Some(Advanced::Moved)
                    } else {
                        self.set_job_due(id, Some(released));
                        None
                    }
                }
            };
        }
        match job.next_phase_due_at {
            Some(due) if due <= now => {
                let waits = job.depends_on.as_ref().is_some_and(|on| !on.is_empty());
                match job.status.clone().as_str() {
                    JOB_SUBMITTED if waits => {
                        self.set_job_status(id, JOB_PENDING);
                        self.set_job_due(id, None);
                        Some(Advanced::Moved)
                    }
                    JOB_SUBMITTED => {
                        self.set_job_status(id, JOB_RUNNABLE);
                        self.set_job_due(id, Some(due + JOB_PHASE_MILLIS));
                        Some(Advanced::Moved)
                    }
                    // The container cannot be run here, so the run the caller
                    // asked for ends in the modelled failure state, saying so.
                    // No `startedAt`, attempt or `exitCode` is invented for it.
                    JOB_RUNNABLE => {
                        self.finish_job(id, JOB_FAILED, NO_CONTAINER_RUNTIME_REASON, due);
                        Some(Advanced::Terminal)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Persist a submitted job, bounded by [`limits::MAX_JOBS`].
    ///
    /// Makes room first by evicting the jobs that have been terminal longest — a terminal
    /// job is one no later call can move again — and answers `Err(message)`, for the
    /// modelled `ClientException`, when every job in a full store is still live.
    pub fn insert_job(&mut self, job: Job) -> Result<(), String> {
        if self.jobs.len() >= limits::MAX_JOBS {
            self.evict_terminal_jobs(self.jobs.len() + 1 - limits::MAX_JOBS);
        }
        if self.jobs.len() >= limits::MAX_JOBS {
            return Err(quota_message("jobs", limits::MAX_JOBS));
        }
        let id = job.job_id.clone();
        let queue_arn = job.job_queue_arn.clone();
        let due = job.next_phase_due_at;
        // An edge only for a dependency the store actually holds: a `dependsOn` id is
        // request-supplied, and `SubmitJob` refuses one that names no job, so an entry for
        // an absent id would be an index key nothing could ever remove.
        let dependencies: Vec<String> = job
            .depends_on
            .iter()
            .flatten()
            .filter_map(|dependency| dependency.job_id.clone())
            .filter(|dependency_id| self.jobs.contains_key(dependency_id))
            .collect();
        for dependency_id in dependencies {
            self.dependents.entry(dependency_id).or_default().push(id.clone());
        }
        self.jobs.insert(id.clone(), job);
        self.jobs_by_queue
            .entry(queue_arn)
            .or_default()
            .push(id.clone());
        if let Some(due) = due {
            self.jobs_due.insert((due, id));
        }
        Ok(())
    }

    /// Terminate or cancel `id`, and answer whether the job exists.
    ///
    /// AWS Batch documents for `TerminateJob`: jobs in `SUBMITTED`, `PENDING` or
    /// `RUNNABLE` are cancelled, jobs that have progressed to `STARTING` or `RUNNING` are
    /// terminated; either way the job ends `FAILED` carrying `reason`. `STARTING` and
    /// `RUNNING` are modelled but unreachable here (nothing runs a container, so a job
    /// never leaves `RUNNABLE` except to fail), and the rule is kept for both so the
    /// distinction holds the day a runtime exists. A job already in a terminal status is
    /// left exactly as it is.
    pub fn terminate_job(&mut self, id: &str, reason: &str, now: i64) -> bool {
        let Some(job) = self.jobs.get(id) else {
            return false;
        };
        if is_terminal_status(&job.status) {
            return true;
        }
        let running = matches!(job.status.as_str(), JOB_STARTING | JOB_RUNNING);
        if let Some(job) = self.jobs.get_mut(id) {
            job.is_terminated = running;
            job.is_cancelled = !running;
        }
        self.finish_job(id, JOB_FAILED, reason, now);
        true
    }

    /// Cancel `id` with `reason`, and answer what that did to the job.
    ///
    /// AWS Batch documents for `CancelJob`: a job in `SUBMITTED`, `PENDING` or `RUNNABLE`
    /// is cancelled and ends `FAILED`; a job that progressed to `STARTING` or `RUNNING`
    /// "aren't cancelled" and must be terminated instead, "However, the API operation
    /// still succeeds, even if no job is cancelled". A job already in a terminal status is
    /// left exactly as it is, and a `PENDING` job's cancel is deferred to the clock (see
    /// [`Job::cancel_requested`]). Which of those happened is the answer, so the caller
    /// never has to re-derive it; `CancelJob` itself succeeds for all of them.
    pub fn cancel_job(&mut self, id: &str, reason: &str, now: i64) -> CancelOutcome {
        let Some(job) = self.jobs.get(id) else {
            return CancelOutcome::Unknown;
        };
        if is_terminal_status(&job.status) {
            return CancelOutcome::AlreadyTerminal;
        }
        match job.status.as_str() {
            JOB_PENDING => {
                if let Some(job) = self.jobs.get_mut(id) {
                    job.cancel_requested = Some(reason.to_string());
                }
                CancelOutcome::Deferred
            }
            JOB_SUBMITTED | JOB_RUNNABLE => {
                if let Some(job) = self.jobs.get_mut(id) {
                    job.is_cancelled = true;
                }
                self.finish_job(id, JOB_FAILED, reason, now);
                CancelOutcome::Cancelled
            }
            // STARTING and RUNNING: cancelling does not touch them.
            _ => CancelOutcome::NotCancelled,
        }
    }

    /// Materialise the cancel [`Job::cancel_requested`] recorded for `id`: the job ends
    /// `FAILED` at `stopped_at` carrying the reason the cancel named.
    fn finish_cancelled(&mut self, id: &str, stopped_at: i64) {
        let reason = self
            .jobs
            .get(id)
            .and_then(|job| job.cancel_requested.clone())
            .unwrap_or_default();
        if let Some(job) = self.jobs.get_mut(id) {
            job.is_cancelled = true;
        }
        self.finish_job(id, JOB_FAILED, &reason, stopped_at);
    }

    /// Set `id`'s status, leaving what the clock owes it alone.
    fn set_job_status(&mut self, id: &str, status: &str) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.status = status.to_string();
        }
    }

    /// Point `id`'s clock-owed transition at `due`, or clear it, keeping the due index in
    /// step so [`Self::advance_jobs`] stays proportional to what is due.
    fn set_job_due(&mut self, id: &str, due: Option<i64>) {
        let Some(job) = self.jobs.get_mut(id) else {
            return;
        };
        let previous = job.next_phase_due_at;
        job.next_phase_due_at = due;
        if let Some(previous) = previous {
            self.jobs_due.remove(&(previous, id.to_string()));
        }
        if let Some(due) = due {
            self.jobs_due.insert((due, id.to_string()));
        }
    }

    /// Put `id` in a terminal `status` at `stopped_at` with `reason` as its
    /// `statusReason` (and its container's `reason`), and clear what the clock owed it.
    ///
    /// Every route into a terminal status goes through here — the clock and `TerminateJob`
    /// alike — so this is where the jobs WAITING on `id` are woken: each one that is still
    /// waiting gets this instant as its due instant, which puts it back in the due index,
    /// and the next `materialize_due` recomputes what its dependencies now allow. One
    /// release path, both callers.
    fn finish_job(&mut self, id: &str, status: &str, reason: &str, stopped_at: i64) {
        self.set_job_due(id, None);
        if let Some(job) = self.jobs.get_mut(id) {
            job.status = status.to_string();
            job.status_reason = Some(reason.to_string());
            job.stopped_at = Some(stopped_at);
            if let Some(container) = job.container.as_mut() {
                container.reason = Some(reason.to_string());
            }
        }
        let waiting = self.dependents.get(id).cloned().unwrap_or_default();
        for dependent in waiting {
            let still_waiting = self.jobs.get(&dependent).is_some_and(|job| {
                job.status == JOB_PENDING && job.next_phase_due_at.is_none()
            });
            if still_waiting {
                self.set_job_due(&dependent, Some(stopped_at));
            }
        }
    }

    /// Drop up to `wanted` of the jobs that have been terminal longest, with every index
    /// entry that named them. A job a live job still depends on is kept, so a dependency
    /// never vanishes from under a dependent that is still waiting for it.
    fn evict_terminal_jobs(&mut self, wanted: usize) {
        let mut evictable: Vec<(i64, String)> = self
            .jobs
            .values()
            .filter(|job| {
                is_terminal_status(&job.status) && !self.has_live_dependent(&job.job_id)
            })
            .map(|job| (job.stopped_at.unwrap_or(job.created_at), job.job_id.clone()))
            .collect();
        evictable.sort();
        for (_, id) in evictable.into_iter().take(wanted) {
            self.remove_job(&id);
        }
    }

    /// Whether a job that is not terminal still waits on `id`.
    fn has_live_dependent(&self, id: &str) -> bool {
        self.dependents.get(id).is_some_and(|waiting| {
            waiting.iter().any(|dependent| {
                self.jobs
                    .get(dependent)
                    .is_some_and(|job| !is_terminal_status(&job.status))
            })
        })
    }

    /// Forget `id` and every index entry that named it.
    fn remove_job(&mut self, id: &str) {
        let Some(job) = self.jobs.remove(id) else {
            return;
        };
        if let Some(due) = job.next_phase_due_at {
            self.jobs_due.remove(&(due, id.to_string()));
        }
        if let Some(ids) = self.jobs_by_queue.get_mut(&job.job_queue_arn) {
            ids.retain(|candidate| candidate != id);
        }
        self.dependents.remove(id);
        for dependency in job.depends_on.iter().flatten() {
            if let Some(dependency_id) = dependency.job_id.as_deref() {
                if let Some(waiting) = self.dependents.get_mut(dependency_id) {
                    waiting.retain(|candidate| candidate != id);
                    // The key goes with its last edge, so the index holds nothing for a
                    // job that is gone.
                    if waiting.is_empty() {
                        self.dependents.remove(dependency_id);
                    }
                }
            }
        }
    }

    /// What the jobs `id` depends on allow right now.
    fn dependency_outcome(&self, id: &str, now: i64) -> DependencyOutcome {
        let Some(job) = self.jobs.get(id) else {
            return DependencyOutcome::Waiting;
        };
        let mut latest = job.created_at;
        for dependency in job.depends_on.iter().flatten() {
            let Some(dependency_id) = dependency.job_id.as_deref() else {
                continue;
            };
            let Some(dependency) = self.jobs.get(dependency_id) else {
                return DependencyOutcome::Unresolvable;
            };
            let stopped = dependency.stopped_at.unwrap_or(now);
            match dependency.status.as_str() {
                JOB_SUCCEEDED => latest = latest.max(stopped),
                JOB_FAILED => return DependencyOutcome::Failed(stopped),
                _ => return DependencyOutcome::Waiting,
            }
        }
        DependencyOutcome::Satisfied(latest)
    }
}

/// The job definition NAME a reference carries: the name part of an ARN
/// (`.../job-definition/<name>:<revision>`) or of a `<name>:<revision>` short form, and
/// the reference itself when it is a bare name.
pub fn job_definition_reference_name(reference: &str) -> &str {
    let short = arn_resource_id(reference, "job-definition").unwrap_or(reference);
    short.rsplit_once(':').map_or(short, |(name, _)| name)
}

/// The two forms a job-definition reference takes, parsed once: the NAME it carries
/// and the revision when it names one.
///
/// The syntax is what every `jobDefinition` member documents — a bare name, a
/// `<name>:<revision>` short form, or a full ARN of either shape — so the
/// resolvers above differ only in what they SELECT (the latest active revision, or
/// the latest stored one) rather than each re-parsing the reference. `None` when a
/// revision is present but is not a number, which no stored revision answers to.
fn parse_job_definition_reference(reference: &str) -> Option<(&str, Option<i32>)> {
    let short = arn_resource_id(reference, "job-definition").unwrap_or(reference);
    match short.rsplit_once(':') {
        Some((name, revision)) => Some((name, Some(revision.parse().ok()?))),
        None => Some((short, None)),
    }
}

/// A `ClientException` for `detail` — the 400 every implemented operation answers an
/// unknown, duplicate, invalid or over-quota request with, worded the way Batch's
/// frontend words one ([`client_exception_message`]), so one wording rule covers every
/// refusal this service raises.
pub fn client_exception(detail: impl AsRef<str>) -> ClientException {
    ClientException {
        message: Some(client_exception_message(detail.as_ref())),
    }
}

/// The `ServerException` for an engine that did not hand back this service's store.
pub fn unreachable_state() -> ServerException {
    ServerException {
        message: Some(UNREACHABLE_STATE.to_string()),
    }
}

/// The container a job actually gets: the definition's `containerProperties` with
/// the submit request's `containerOverrides` applied over it.
///
/// `command`, `vcpus`, `memory` and `instanceType` are replaced by the override
/// when it carries one; `environment` and `resourceRequirements` are merged
/// entry-wise (an override entry replaces the definition's entry of the same
/// name/type and is otherwise added), which is what AWS Batch documents for them.
pub fn effective_container(
    properties: Option<&ContainerProperties>,
    overrides: Option<&ContainerOverrides>,
) -> Option<ContainerDetail> {
    let properties = properties?;
    let mut detail = ContainerDetail {
        image: properties.image.clone(),
        vcpus: properties.vcpus,
        memory: properties.memory,
        command: properties.command.clone(),
        job_role_arn: properties.job_role_arn.clone(),
        execution_role_arn: properties.execution_role_arn.clone(),
        volumes: properties.volumes.clone(),
        environment: properties.environment.clone(),
        mount_points: properties.mount_points.clone(),
        readonly_root_filesystem: properties.readonly_root_filesystem,
        ulimits: properties.ulimits.clone(),
        privileged: properties.privileged,
        user: properties.user.clone(),
        instance_type: properties.instance_type.clone(),
        resource_requirements: properties.resource_requirements.clone(),
        linux_parameters: properties.linux_parameters.clone(),
        log_configuration: properties.log_configuration.clone(),
        secrets: properties.secrets.clone(),
        network_configuration: properties.network_configuration.clone(),
        fargate_platform_configuration: properties.fargate_platform_configuration.clone(),
        ephemeral_storage: properties.ephemeral_storage.clone(),
        runtime_platform: properties.runtime_platform.clone(),
        repository_credentials: properties.repository_credentials.clone(),
        enable_execute_command: properties.enable_execute_command,
        ..ContainerDetail::default()
    };
    if let Some(overrides) = overrides {
        if let Some(command) = overrides.command.as_ref().filter(|it| !it.is_empty()) {
            detail.command = Some(command.clone());
        }
        if let Some(vcpus) = overrides.vcpus {
            detail.vcpus = Some(vcpus);
        }
        if let Some(memory) = overrides.memory {
            detail.memory = Some(memory);
        }
        if let Some(instance_type) = overrides.instance_type.as_ref() {
            detail.instance_type = Some(instance_type.clone());
        }
        if let Some(environment) = overrides.environment.as_deref() {
            detail.environment = Some(merged_environment(
                detail.environment.as_deref(),
                environment,
            ));
        }
        if let Some(requirements) = overrides.resource_requirements.as_deref() {
            detail.resource_requirements = Some(merged_requirements(
                detail.resource_requirements.as_deref(),
                requirements,
            ));
        }
    }
    Some(detail)
}

/// `base` with every `overrides` entry replacing the entry of the same name, or
/// appended when there is none.
fn merged_environment(base: Option<&[KeyValuePair]>, overrides: &[KeyValuePair]) -> Vec<KeyValuePair> {
    let mut merged = base.unwrap_or_default().to_vec();
    for pair in overrides {
        match merged.iter_mut().find(|entry| entry.name == pair.name) {
            Some(entry) => entry.value = pair.value.clone(),
            None => merged.push(pair.clone()),
        }
    }
    merged
}

/// `base` with every `overrides` requirement replacing the requirement of the
/// same type, or appended when there is none.
fn merged_requirements(
    base: Option<&[ResourceRequirement]>,
    overrides: &[ResourceRequirement],
) -> Vec<ResourceRequirement> {
    let mut merged = base.unwrap_or_default().to_vec();
    for requirement in overrides {
        match merged
            .iter_mut()
            .find(|entry| entry.r#type == requirement.r#type)
        {
            Some(entry) => entry.value = requirement.value.clone(),
            None => merged.push(requirement.clone()),
        }
    }
    merged
}

/// The job definition's parameter defaults with the submit request's parameters
/// applied over them, as AWS Batch documents for `SubmitJob$parameters`.
pub fn merged_parameters(
    defaults: Option<&HashMap<String, String>>,
    overrides: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    if defaults.is_none() && overrides.is_none() {
        return None;
    }
    let mut merged = defaults.cloned().unwrap_or_default();
    if let Some(overrides) = overrides {
        for (key, value) in overrides {
            merged.insert(key.clone(), value.clone());
        }
    }
    Some(merged)
}

/// One page of `items`, plus the `nextToken` that resumes after it.
///
/// The token is the index the next page starts at, so a caller that pages to the
/// end sees every item exactly once. An unparseable or out-of-range token is an
/// error the caller gets as a `ClientException`.
pub fn paginate<T>(
    items: Vec<T>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<(Vec<T>, Option<String>), String> {
    let total = items.len();
    let start = match next_token {
        Some(token) => token
            .parse::<usize>()
            .ok()
            .filter(|start| *start <= total)
            .ok_or_else(|| format!("The nextToken {token} is not valid."))?,
        None => 0,
    };
    let limit = max_results
        .filter(|requested| *requested > 0)
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(total);
    let end = start.saturating_add(limit).min(total);
    let page: Vec<T> = items.into_iter().skip(start).take(end - start).collect();
    let token = (end < total).then(|| end.to_string());
    Ok((page, token))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBatchAwsbatchv20160810State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBatchAwsbatchv20160810State>)
}

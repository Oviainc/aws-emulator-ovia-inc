// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-batch-awsbatchv20160810 service.
//!
//! Every implemented operation computes its answer from the records below: the
//! compute environments, job queues, job definition revisions, consumable
//! resources and jobs this service has been asked to create, keyed the way AWS
//! Batch keys them (by name, with an ARN index beside it so anything the model
//! documents as "name or ARN" resolves both ways).
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
//!
//! A SERVICE job's work is not run either, for the same reason: nothing here runs
//! SageMaker AI, so a service job `SubmitServiceJob` accepted walks the same
//! clock-owed chain (`SUBMITTED` -> `RUNNABLE`) and then ends in the modelled failure
//! state with [`NO_SERVICE_JOB_RUNTIME_REASON`], with no `scheduledAt`, `startedAt`,
//! attempt or capacity usage invented for it.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    CapacityLimit, ClientException, ComputeEnvironmentOrder, ComputeResource,
    ConsumableResourceProperties, ContainerDetail, ContainerOverrides, ContainerProperties,
    EcsProperties, EcsSettings, EksConfiguration, FairsharePolicy, JobDefinition, JobDependency,
    JobStateTimeLimitAction, JobTimeout, KeyValuePair, NodeProperties,
    QuotaShareCapacityLimit, QuotaSharePolicy, QuotaSharePreemptionConfiguration,
    QuotaShareResourceSharingConfiguration, ResourceRequirement, RetryStrategy, ServerException,
    ServiceEnvironmentOrder, ServiceJobPreemptionConfiguration, ServiceJobRetryStrategy,
    ServiceJobTimeout, ShareAttributes, UpdatePolicy,
};

/// This service's ARN namespace and sigv4 signing name (`OPERATIONS.md`).
pub const ARN_SERVICE: &str = "batch";

/// The region used when a request carries no SigV4 credential scope to read one
/// from. Every emulator defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long one clock-owed transition takes: a compute environment or job queue
/// leaving `CREATING`, a compute environment or job queue leaving `UPDATING`, and
/// each move a submitted job makes.
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

    /// Consumable resources kept at once. An EMULATOR-CHOSEN ceiling, not an AWS quota:
    /// `CreateConsumableResource` is a request anyone can repeat, so the store answers the
    /// modelled `ClientException` past this point rather than growing without bound.
    pub const MAX_CONSUMABLE_RESOURCES: usize = 500;

    /// Quota shares kept at once. An EMULATOR bound (AWS publishes no quota for this
    /// resource): `CreateQuotaShare` fills the store and anyone can repeat it, so a create
    /// past this answers the modelled `ClientException`. Sized like the other per-Region
    /// Batch resources above.
    pub const MAX_QUOTA_SHARES: usize = 50;

    /// Service jobs kept at once, the same emulator bound as [`MAX_JOBS`] and with the
    /// same eviction-before-refusal rule
    /// ([`super::ComAmazonawsBatchAwsbatchv20160810State::insert_service_job`]).
    pub const MAX_SERVICE_JOBS: usize = 500;

    /// Scheduling policies kept at once. An EMULATOR bound (AWS publishes no quota for
    /// this resource): `CreateSchedulingPolicy` fills the store and anyone can repeat it,
    /// so a create past this answers the modelled `ClientException`. Sized like the other
    /// per-Region, name-keyed Batch resources above.
    pub const MAX_SCHEDULING_POLICIES: usize = 50;

    /// Job ids ONE `DescribeJobs` call may name. AWS documents it on the member:
    /// `DescribeJobsRequest$jobs` is "A list of up to 100 job IDs"
    /// (`smithy_query(query="doc", shape="DescribeJobsRequest", member="jobs")`); the
    /// model types it as a plain `StringList`, so the request validator admits any length
    /// and the handler applies the bound.
    pub const MAX_DESCRIBE_JOBS: usize = 100;
}

/// What AWS Batch answers a `DescribeJobs` naming more than
/// [`limits::MAX_DESCRIBE_JOBS`] job ids with, inside its frontend envelope.
///
/// The spacing around the maximum is AWS's own (the recorded detail reads
/// `Maximum number of jobs supported is  100 .`): the number is interpolated into a
/// sentence that already carries the separators.
pub fn describe_jobs_bound_message() -> String {
    format!(
        "Maximum number of jobs supported is  {} .",
        limits::MAX_DESCRIBE_JOBS
    )
}

/// The `ClientException` message a request past one of [`limits`] answers with.
pub fn quota_message(resource: &str, maximum: usize) -> String {
    format!("Too many {resource}: this emulator stores at most {maximum}")
}

/// What `DescribeQuotaShare` answers for a `quotaShareArn` that is not a quota-share ARN
/// at all — the recorded detail, which states the pattern requirement without echoing the
/// value or spelling the pattern out.
pub const QUOTA_SHARE_ARN_NOT_VALID: &str =
    "Quota Share arn should match a valid Quota Share arn pattern";

/// What AWS Batch answers when a `Create*` names an object it already holds: the bare
/// detail `CreateComputeEnvironment` is recorded to state for a duplicate name, with no
/// resource name interpolated into it.
pub const OBJECT_ALREADY_EXISTS: &str = "Object already exists";

/// What `CreateComputeEnvironment` answers when an `UNMANAGED` environment is created
/// with Fargate compute resources — the recorded detail, stated as the rule rather than
/// naming the environment.
///
/// `computeResources` is "details of the compute resources MANAGED by the compute
/// environment", and Fargate capacity exists only where Batch manages it, so the pair
/// `UNMANAGED` + a Fargate `CRType` describes nothing this service can create.
pub const UNMANAGED_FARGATE_NOT_CREATABLE: &str =
    "Cannot create an UNMANAGED Fargate Compute Environment.";

/// The `CRType` values whose compute resources run on Fargate, from the model's own
/// `com.amazonaws.batch#CRType` enum (`EC2 | SPOT | FARGATE | FARGATE_SPOT |
/// ECS_MANAGED_INSTANCES`), of which these two are the Fargate pair.
pub const FARGATE_COMPUTE_TYPES: &[&str] = &["FARGATE", "FARGATE_SPOT"];

/// Whether a compute resource of this `CRType` runs on Fargate
/// ([`FARGATE_COMPUTE_TYPES`]).
pub fn is_fargate_compute_type(compute_type: &str) -> bool {
    FARGATE_COMPUTE_TYPES.contains(&compute_type)
}

/// What `CreateJobQueue` answers when its `computeEnvironmentOrder` names a compute
/// environment it cannot attach — the bare detail AWS is recorded to state, which names
/// the requirement rather than the environment.
pub const COMPUTE_ENVIRONMENTS_NOT_ATTACHABLE: &str =
    "Compute Environments must be created and valid before attaching them to a job queue";

/// What `CreateJobQueue`/`UpdateJobQueue` answer when a `serviceEnvironmentOrder` entry
/// names a service environment they cannot attach — the recorded detail, which (like
/// [`COMPUTE_ENVIRONMENTS_NOT_ATTACHABLE`], its compute-environment twin) states the
/// precondition instead of naming the entry, so the one refusal covers an environment
/// that was never created and one that is not valid.
pub const SERVICE_ENVIRONMENTS_NOT_ATTACHABLE: &str =
    "Service environments must be created and valid before attaching them to a job queue.";

/// What `DescribeJobDefinitions` answers for a `nextToken` it did not issue — the bare
/// detail AWS is recorded to state.
pub const NEXT_TOKEN_NOT_PROCESSABLE: &str = "Error processing next token";

/// `statusReason` of a job whose container could not be run here.
pub const NO_CONTAINER_RUNTIME_REASON: &str =
    "This emulator has no container runtime, so the job's container was never started";

/// `statusReason` of a job the deletion of its job queue ended.
///
/// `DeleteJobQueue`'s own doc: "All jobs in the queue are eventually terminated when you
/// delete a job queue."
pub const JOB_QUEUE_DELETED_REASON: &str = "The job queue this job was submitted to was deleted";

/// `statusReason` of a service job whose work nothing here dispatched.
///
/// A service job's work runs on another service (`ServiceJobType` is
/// `SAGEMAKER_TRAINING`), which this emulator does not run and the engine contract
/// (`vendor/engine/src/lib.rs`) forbids pretending to: so the run the caller asked for
/// ends in the modelled failure state saying exactly that, and no `scheduledAt`,
/// `startedAt`, attempt or capacity usage is invented for it.
pub const NO_SERVICE_JOB_RUNTIME_REASON: &str =
    "This emulator does not run SageMaker AI, so the service job was never dispatched";

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
/// `CEStatus`/`JQStatus` value a resource carries while an update to it is pending
/// (`UpdateComputeEnvironment`, `UpdateJobQueue`); the move back to [`STATUS_VALID`]
/// is owed by the clock, like the one out of [`STATUS_CREATING`].
pub const STATUS_UPDATING: &str = "UPDATING";
/// `CEStatus`/`JQStatus` value of a created resource.
pub const STATUS_VALID: &str = "VALID";
/// `CEStatus`/`JQStatus` value a resource carries while the deletion a `Delete*` call
/// started is still owed by the clock.
pub const STATUS_DELETING: &str = "DELETING";
/// `CEState`/`JQState` default: a resource is created enabled unless asked otherwise.
pub const STATE_ENABLED: &str = "ENABLED";
/// `CEState`/`JQState` of a resource that accepts no new work, and the state a resource
/// must carry before it can be deleted (`DeleteComputeEnvironment`: "you must set its
/// state to DISABLED"; `DeleteJobQueue`: submissions must first be disabled with
/// `UpdateJobQueue`). `JQState`'s own doc: "If the job queue state is `DISABLED`, new
/// jobs can't be added to the queue, but jobs already in the queue can finish."
pub const STATE_DISABLED: &str = "DISABLED";
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

/// The longest `quotaShareName` the service accepts, from the member's doc: "The name of
/// the quota share. It can be up to 128 characters long. It can contain uppercase and
/// lowercase letters, numbers, hyphens (-), and underscores (_)." The model types the
/// member as a plain `String`, so the framework cannot refuse a bad one.
pub const MAX_QUOTA_SHARE_NAME_LEN: usize = 128;

/// The longest `jobName` the service accepts, from the member's doc (`SubmitJob$jobName`,
/// `SubmitServiceJob$jobName`): "It can be up to 128 characters long. It can contain
/// uppercase and lowercase letters, numbers, hyphens (-), and underscores (_)." The model
/// types the member as a plain `String`, so the framework cannot refuse a bad one.
pub const MAX_JOB_NAME_LEN: usize = 128;

/// The fewest attempts `ServiceJobRetryStrategy$attempts` accepts, from the member's doc:
/// "You can specify between 1 and 10 attempts." The model types it as a plain `Integer`,
/// so the bound is applied in the handler.
pub const MIN_SERVICE_JOB_RETRY_ATTEMPTS: i32 = 1;
/// The most attempts `ServiceJobRetryStrategy$attempts` accepts; see
/// [`MIN_SERVICE_JOB_RETRY_ATTEMPTS`].
pub const MAX_SERVICE_JOB_RETRY_ATTEMPTS: i32 = 10;

/// The longest scheduling policy `name` the service accepts, from the member's doc: "The
/// name of the fair-share scheduling policy. It can be up to 128 letters long. It can
/// contain uppercase and lowercase letters, numbers, hyphens (-), and underscores (_)."
/// The model types the member as a plain `String`, so the framework cannot refuse a bad
/// one.
pub const MAX_SCHEDULING_POLICY_NAME_LEN: usize = 128;

/// The bounds of `FairsharePolicy$shareDecaySeconds`, from the member's own doc: "A value
/// of zero (0) indicates the default minimum time window (600 seconds). The maximum
/// supported value is 604800 (1 week)." The model types the member as a plain `Integer`,
/// so the rule is applied by every operation that accepts the policy
/// ([`validate_fairshare_policy`]).
pub const MIN_SHARE_DECAY_SECONDS: i32 = 0;
/// See [`MIN_SHARE_DECAY_SECONDS`].
pub const MAX_SHARE_DECAY_SECONDS: i32 = 604_800;

/// The bounds of `FairsharePolicy$computeReservation`, from the member's own doc: "The
/// minimum value is 0 and the maximum value is 99."
pub const MIN_COMPUTE_RESERVATION: i32 = 0;
/// See [`MIN_COMPUTE_RESERVATION`].
pub const MAX_COMPUTE_RESERVATION: i32 = 99;

/// The bounds of `ShareAttributes$weightFactor`, from the member's own doc: "The smallest
/// supported value is 0.0001, and the largest supported value is 999.9999."
pub const MIN_WEIGHT_FACTOR: f32 = 0.0001;
/// See [`MIN_WEIGHT_FACTOR`].
pub const MAX_WEIGHT_FACTOR: f32 = 999.9999;

/// The longest `ShareAttributes$shareIdentifier` the service accepts, from the member's
/// own doc: "The string is limited to 255 alphanumeric characters, and can be followed by
/// an asterisk (*)."
pub const MAX_SHARE_IDENTIFIER_LEN: usize = 255;

/// The most share identifiers one fair-share policy may carry, from the
/// `ShareAttributes$shareIdentifier` doc ("There can be no more than 500 share identifiers
/// active in a job queue"), which `CreateJobQueue$schedulingPolicyArn` repeats ("Jobs
/// queues with a fair-share scheduling policy can have a maximum of 500 active share
/// identifiers"). The model constrains the list's length nowhere, so a longer
/// `shareDistribution` is refused here — before anything walks it
/// ([`validate_fairshare_policy`]), which is also what bounds the payload one stored
/// policy retains.
pub const MAX_SHARE_DISTRIBUTION_ENTRIES: usize = 500;

/// The lowest `schedulingPriority` the service accepts, from the member's doc: "The
/// minimum supported value is 0 and the maximum supported value is 9999."
pub const MIN_SCHEDULING_PRIORITY: i32 = 0;
/// The highest `schedulingPriority` the service accepts; see [`MIN_SCHEDULING_PRIORITY`].
pub const MAX_SCHEDULING_PRIORITY: i32 = 9999;

/// The shortest job timeout the service accepts, in seconds, from the `timeout` member's
/// doc: "The minimum value for the timeout is 60 seconds."
pub const MIN_ATTEMPT_DURATION_SECONDS: i32 = 60;

/// The bounds of `JobStateTimeLimitAction$maxTimeSeconds`, from the member's own doc:
/// "The minimum value is 600 (10 minutes) and the maximum value is 86,400 (24 hours)."
/// The model types the member as a plain `Integer`, so the framework admits anything and
/// the rule is applied by every operation that accepts the list
/// ([`validate_job_state_time_limit_actions`]).
pub const MIN_JOB_STATE_TIME_LIMIT_SECONDS: i32 = 600;
/// See [`MIN_JOB_STATE_TIME_LIMIT_SECONDS`].
pub const MAX_JOB_STATE_TIME_LIMIT_SECONDS: i32 = 86_400;

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

/// `ConsumableResource$resourceType` of a resource that is handed back when a job that
/// held it completes — the default, from the `resourceType` member's own doc:
/// "`REPLENISHABLE` (default)".
pub const RESOURCE_TYPE_REPLENISHABLE: &str = "REPLENISHABLE";

/// `ConsumableResource$resourceType` of a resource a completed job does NOT hand back,
/// from the same doc's list.
pub const RESOURCE_TYPE_NON_REPLENISHABLE: &str = "NON_REPLENISHABLE";

/// Every `resourceType` a consumable resource may carry, from the `resourceType` member's
/// doc ("Can be one of: `REPLENISHABLE` (default), `NON_REPLENISHABLE`"). The model types
/// the member as a plain `String`, so the framework admits anything and the handler
/// applies this list.
pub const CONSUMABLE_RESOURCE_TYPES: [&str; 2] =
    [RESOURCE_TYPE_REPLENISHABLE, RESOURCE_TYPE_NON_REPLENISHABLE];

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

// `ServiceJobStatus` values. A separate enum from `JobStatus` in the model
// (`smithy_query(query="shape", shape="ServiceJobStatus")`: SUBMITTED, PENDING,
// RUNNABLE, SCHEDULED, STARTING, RUNNING, SUCCEEDED, FAILED), so a service job's
// statuses are named — and tested for terminality — on their own.
/// `ServiceJobStatus` of a service job the service has accepted but not yet placed —
/// the status `SubmitServiceJob` hands a fresh service job back in.
pub const SERVICE_JOB_SUBMITTED: &str = "SUBMITTED";
/// `ServiceJobStatus` of a service job the scheduler may place.
pub const SERVICE_JOB_RUNNABLE: &str = "RUNNABLE";
/// Terminal `ServiceJobStatus`: the service job completed successfully.
pub const SERVICE_JOB_SUCCEEDED: &str = "SUCCEEDED";
/// Terminal `ServiceJobStatus`: the service job did not complete successfully.
pub const SERVICE_JOB_FAILED: &str = "FAILED";

/// `JobQueueType` of a queue that runs service jobs, which is the type
/// `SubmitServiceJob$jobQueue` documents ("The job queue must have the type
/// `SAGEMAKER_TRAINING`"). One of the model's `JobQueueType` values
/// (`smithy_query(query="shape", shape="JobQueueType")`).
pub const JOB_QUEUE_TYPE_SAGEMAKER_TRAINING: &str = "SAGEMAKER_TRAINING";

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

/// `arn:aws:batch:<region>:<account>:consumable-resource/<name>`.
pub fn consumable_resource_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("consumable-resource/{name}"))
}

/// `arn:aws:batch:<region>:<account>:service-environment/<name>`.
pub fn service_environment_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("service-environment/{name}"),
    )
}

/// `arn:aws:batch:<region>:<account>:job-queue/<queue>/quota-share/<name>` — a quota
/// share is named under the job queue it is associated with, which is the ARN pattern
/// `DescribeQuotaShare` matches a `quotaShareArn` against.
pub fn quota_share_arn(region: &str, job_queue_name: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("job-queue/{job_queue_name}/quota-share/{name}"),
    )
}

/// The `(job queue name, quota share name)` a well-formed `quotaShareArn` names, or
/// `None` when the value carries no `job-queue/<queue>/quota-share/<share>` path — which
/// is a DIFFERENT refusal from an ARN of the right shape naming nothing stored.
pub fn parse_quota_share_arn(value: &str) -> Option<(&str, &str)> {
    let resource = arn_resource_id(value, "job-queue")?;
    let (queue, share) = resource.split_once("/quota-share/")?;
    (!queue.is_empty() && !share.is_empty() && !share.contains('/')).then_some((queue, share))
}

/// `arn:aws:batch:<region>:<account>:scheduling-policy/<name>` — the format
/// `CreateSchedulingPolicy$arn` documents ("The format is
/// aws:Partition:batch:Region:Account:scheduling-policy/Name").
pub fn scheduling_policy_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("scheduling-policy/{name}"))
}

/// `arn:aws:batch:<region>:<account>:service-job/<job id>` — the ARN a service job
/// carries, the resource kind AWS Batch names a service job with.
pub fn service_job_arn(region: &str, job_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("service-job/{job_id}"))
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
    /// Set while the `CREATING` -> `VALID` or `UPDATING` -> `VALID` move is still
    /// owed by the clock.
    pub status_due_at: Option<i64>,
    /// The environment's unique identifier, minted once at creation.
    pub uuid: String,
    pub unmanagedv_cpus: Option<i32>,
    pub compute_resources: Option<ComputeResource>,
    /// The infrastructure update policy the last `UpdateComputeEnvironment` set, which
    /// `DescribeComputeEnvironments` reports back; `None` until one sets it (nothing
    /// else carries this member — `CreateComputeEnvironment` has no `updatePolicy`).
    pub update_policy: Option<UpdatePolicy>,
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

/// A scheduling policy, exactly as `CreateSchedulingPolicy` stored it.
///
/// The name is the policy's identity and the ARN is minted once beside it
/// ([`scheduling_policy_arn`]), so every later operation reports the same one. AWS Batch
/// returns a scheduling policy ready — it carries no status member at all — so nothing
/// about it is owed by the clock.
///
/// A policy carries a fair-share policy OR a quota-share policy, never both: "Only one of
/// fairsharePolicy or quotaSharePolicy can be set. Once set, this policy type cannot be
/// removed or changed" (`CreateSchedulingPolicy$fairsharePolicy`).
#[derive(Debug, Clone)]
pub struct SchedulingPolicy {
    pub name: String,
    pub arn: String,
    pub fairshare_policy: Option<FairsharePolicy>,
    pub quota_share_policy: Option<QuotaSharePolicy>,
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
    /// Set while the `CREATING` -> `VALID` or `UPDATING` -> `VALID` move is still owed by
    /// the clock.
    pub status_due_at: Option<i64>,
    pub priority: i32,
    pub scheduling_policy_arn: Option<String>,
    pub compute_environment_order: Vec<ComputeEnvironmentOrder>,
    pub service_environment_order: Option<Vec<ServiceEnvironmentOrder>>,
    pub job_queue_type: Option<String>,
    pub tags: Option<HashMap<String, String>>,
    pub job_state_time_limit_actions: Option<Vec<JobStateTimeLimitAction>>,
}

/// A quota share, exactly as `CreateQuotaShare` stored it.
///
/// Keyed by its ARN, which is the only identifier `DescribeQuotaShare` takes
/// (`quotaShareArn`), and nested under the job queue the share is associated with — what
/// that ARN names ([`quota_share_arn`]). AWS Batch returns a fresh quota share in the
/// non-terminal `CREATING` status, so the move to `VALID` is owed by the clock
/// ([`QuotaShare::status_due_at`]).
#[derive(Debug, Clone)]
pub struct QuotaShare {
    pub name: String,
    pub arn: String,
    /// The ARN of the job queue the create's `jobQueue` resolved to (by name or by ARN).
    pub job_queue_arn: String,
    pub capacity_limits: Vec<QuotaShareCapacityLimit>,
    pub resource_sharing_configuration: QuotaShareResourceSharingConfiguration,
    pub preemption_configuration: QuotaSharePreemptionConfiguration,
    /// `QuotaShareState`: `ENABLED` unless the create asked for `DISABLED` ("The default
    /// state is `ENABLED`").
    pub state: String,
    /// `QuotaShareStatus`.
    pub status: String,
    /// Set while the `CREATING` -> `VALID` move is still owed by the clock.
    pub status_due_at: Option<i64>,
    pub tags: Option<HashMap<String, String>>,
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

/// A consumable resource, exactly as `CreateConsumableResource` stored it.
///
/// AWS Batch hands a created consumable resource back ready — the create response carries
/// no status and `DescribeConsumableResource` models none — so this record owes the clock
/// no transition.
#[derive(Debug, Clone)]
pub struct ConsumableResource {
    pub name: String,
    pub arn: String,
    /// `totalQuantity`: how much of the resource exists. Non-negative (the member's doc:
    /// "Must be non-negative"), and 0 for a create that named none.
    pub total_quantity: i64,
    /// `inUseQuantity`: how much of the resource the jobs holding it have taken. 0 for a
    /// resource nothing has taken yet; the operations that reserve and release quantity
    /// maintain it.
    pub in_use_quantity: i64,
    /// One of [`CONSUMABLE_RESOURCE_TYPES`].
    pub resource_type: String,
    /// `createdAt`: the instant this record was stored, in epoch milliseconds.
    pub created_at: i64,
    pub tags: Option<HashMap<String, String>>,
}

impl ConsumableResource {
    /// `availableQuantity` — "The amount of the consumable resource that is currently
    /// available to use": what is left of [`Self::total_quantity`] once the quantity in
    /// use is taken off. Derived on every read, never stored, so it cannot drift from the
    /// two quantities it is computed from.
    pub fn available_quantity(&self) -> i64 {
        self.total_quantity.saturating_sub(self.in_use_quantity).max(0)
    }
}

/// Whether a `JobStatus` is one a job never leaves.
pub fn is_terminal_status(status: &str) -> bool {
    matches!(status, JOB_SUCCEEDED | JOB_FAILED)
}

/// A service job, as `SubmitServiceJob` accepted it: the record `DescribeServiceJob`
/// reports and `TerminateServiceJob` ends.
///
/// A service job's work is dispatched to another service (`ServiceJobType` is
/// `SAGEMAKER_TRAINING`), which this emulator does not run, so no attempt, capacity
/// usage or preemption is ever invented for one: those members stay absent.
#[derive(Debug, Clone, Default)]
pub struct ServiceJob {
    pub job_id: String,
    pub job_arn: String,
    pub job_name: String,
    /// The ARN of the queue the job was submitted to, which is what
    /// `DescribeServiceJob$jobQueue` reports ("The ARN of the job queue").
    pub job_queue_arn: String,
    pub service_job_type: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: i64,
    pub scheduled_at: Option<i64>,
    pub started_at: Option<i64>,
    pub stopped_at: Option<i64>,
    /// When the next clock-owed transition falls due, and `None` once the job owes none
    /// (it reached a terminal status, or it was seeded in one of the modelled statuses
    /// this emulator never moves a job into).
    pub next_phase_due_at: Option<i64>,
    /// Whether a `TerminateServiceJob` is what ended this job.
    pub is_terminated: bool,
    pub share_identifier: Option<String>,
    pub quota_share_name: Option<String>,
    pub scheduling_priority: Option<i32>,
    pub service_request_payload: Option<String>,
    pub retry_strategy: Option<ServiceJobRetryStrategy>,
    pub timeout_config: Option<ServiceJobTimeout>,
    pub preemption_configuration: Option<ServiceJobPreemptionConfiguration>,
    pub tags: Option<HashMap<String, String>>,
    /// The `clientToken` the `SubmitServiceJob` that minted this job carried, if any:
    /// what makes a repeat of that submit resolve to THIS record instead of minting a
    /// second one ("This token is used to ensure idempotency of requests").
    pub client_token: Option<String>,
    /// A digest of the submit request that minted this job, so a `clientToken` repeated
    /// with a DIFFERENT payload can be told apart from the identical replay the member's
    /// doc describes ("two submit requests with identical payloads and `clientToken`s").
    /// `None` for a record no submit request minted.
    pub request_digest: Option<String>,
}

/// Whether a `ServiceJobStatus` is one a service job never leaves.
pub fn is_terminal_service_job_status(status: &str) -> bool {
    matches!(status, SERVICE_JOB_SUCCEEDED | SERVICE_JOB_FAILED)
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

/// How many request ids this process has minted, so two refusals answered in the same
/// nanosecond still name two different requests (see [`new_uuid`]).
static MINTED_REQUEST_IDS: AtomicU64 = AtomicU64::new(0);

/// The id of the request a refusal is answering, which Batch's frontend names in the
/// envelope it wraps a `ClientException` in ([`client_exception_message`]).
///
/// A request id is minted by the SERVICE, per call — nothing in the request carries one —
/// so this emulator mints it the way it mints every other identifier ([`new_uuid`]): a
/// fresh version-4 UUID, distinct for every refusal for the emulator's lifetime. It is a
/// per-run value and is never derived from the request.
pub fn new_request_id() -> String {
    new_uuid(MINTED_REQUEST_IDS.fetch_add(1, Ordering::Relaxed))
}

/// How Batch words the `ClientException`s its frontend WRAPS: the detail of what was
/// wrong, inside `Error executing request, Exception : <detail>, RequestId: <id>`, where
/// the id is the one the service gave the call being refused.
///
/// Only the refusals AWS is recorded to wrap are worded this way; see
/// [`client_exception`] for the bare form and [`client_exception_enveloped`] for the
/// wrapped one.
pub fn client_exception_message(detail: &str, request_id: &str) -> String {
    format!("Error executing request, Exception : {detail}, RequestId: {request_id}")
}

/// The sentence AWS Batch's resource resolver answers when an identifier names no stored
/// resource: the ARN the identifier resolved to, then ` does not exist`.
///
/// The ARN comes from the REQUEST ([`resolved_arn`]) — the resource is not stored, so it
/// has no ARN of its own — and nothing else is stated: the identifier the caller gave is
/// what fills the sentence. Recorded in exactly this form for `UpdateComputeEnvironment`,
/// `DescribeConsumableResource` and `DescribeServiceJob`; the job-queue resolver
/// (`UpdateJobQueue`, `DeleteJobQueue`) and the quota-share one terminate the same
/// sentence with a period, so those call sites add it.
pub fn does_not_exist_message(arn: &str) -> String {
    format!("{arn} does not exist")
}

/// A [`does_not_exist_message`] refusal: the modelled `ClientException` (HTTP 400) for an
/// identifier that resolves to no stored resource.
pub fn does_not_exist_exception(arn: &str) -> ClientException {
    client_exception(does_not_exist_message(arn))
}

/// The ARN a "name or ARN" identifier names, whether or not anything is stored under it:
/// the value itself when it already is an ARN, and otherwise the ARN `mint` gives that
/// name in this request's region and account (`compute_environment_arn`, `job_queue_arn`,
/// … — the same builders a create uses).
///
/// This is what a not-found refusal reports ([`does_not_exist_message`]): AWS names the
/// ARN it resolved the identifier to, and for an identifier naming nothing that ARN can
/// only be computed from the request.
pub fn resolved_arn(name_or_arn: &str, mint: impl FnOnce(&str) -> String) -> String {
    if name_or_arn.starts_with("arn:") {
        return name_or_arn.to_string();
    }
    mint(name_or_arn)
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

/// Whether `value` is a `jobName` the service accepts: up to [`MAX_JOB_NAME_LEN`]
/// letters, numbers, hyphens and underscores (the member's own doc). See
/// [`is_valid_resource_name`].
pub fn is_valid_job_name(value: &str) -> bool {
    is_valid_resource_name(value, MAX_JOB_NAME_LEN)
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
    /// Scheduling policies by name — the name `CreateSchedulingPolicy` keys them by.
    /// Bounded by [`limits::MAX_SCHEDULING_POLICIES`].
    pub scheduling_policies: HashMap<String, SchedulingPolicy>,
    /// Scheduling policy ARN -> name, so a policy named by ARN (the only form
    /// `DescribeSchedulingPolicies`, `UpdateSchedulingPolicy` and `DeleteSchedulingPolicy`
    /// document) resolves to its record.
    pub scheduling_policies_by_arn: HashMap<String, String>,
    /// Registered job-definition revisions, keyed by `(jobDefinitionName, revision)` —
    /// the describe and pagination order. Bounded by [`MAX_JOB_DEFINITIONS`].
    pub job_definitions: BTreeMap<(String, i32), JobDefinition>,
    /// Jobs by job id.
    pub jobs: HashMap<String, Job>,
    /// Consumable resources by name — the name `CreateConsumableResource` must keep
    /// unique ("The name of the consumable resource. Must be unique."). Bounded by
    /// [`limits::MAX_CONSUMABLE_RESOURCES`].
    pub consumable_resources: HashMap<String, ConsumableResource>,
    /// Consumable resource ARN -> name, so `name or ARN` members resolve both ways.
    pub consumable_resources_by_arn: HashMap<String, String>,
    /// Quota shares by ARN — the only identifier `DescribeQuotaShare` takes. Bounded by
    /// [`limits::MAX_QUOTA_SHARES`], and a share goes when the job queue it hangs off
    /// does.
    pub quota_shares: BTreeMap<String, QuotaShare>,
    /// Job ids per queue ARN, in submission order, backing `ListJobs`.
    pub jobs_by_queue: HashMap<String, Vec<String>>,
    /// Service jobs by service job id, the records `DescribeServiceJob` reports and
    /// `TerminateServiceJob` ends. Bounded by [`limits::MAX_SERVICE_JOBS`].
    pub service_jobs: HashMap<String, ServiceJob>,
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

    /// The names of the job queues whose `computeEnvironmentOrder` still names the
    /// compute environment `arn`.
    ///
    /// `DeleteComputeEnvironment`'s own doc requires the environment to be disassociated
    /// "from any job queues with the UpdateJobQueue API operation" before it can be
    /// deleted, so the delete answers the modelled refusal while this is non-empty.
    pub fn queues_using_compute_environment(&self, arn: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .job_queues
            .values()
            .filter(|queue| {
                queue
                    .compute_environment_order
                    .iter()
                    .any(|entry| entry.compute_environment == arn)
            })
            .map(|queue| queue.name.clone())
            .collect();
        names.sort();
        names
    }

    /// The consumable resource `name_or_arn` names, resolved by name or by ARN — the two
    /// forms every `consumableResource` member documents ("The name or ARN of the
    /// consumable resource").
    pub fn consumable_resource(&self, name_or_arn: &str) -> Option<&ConsumableResource> {
        let name = self.consumable_resource_name(name_or_arn)?;
        self.consumable_resources.get(&name)
    }

    /// The NAME of the consumable resource `name_or_arn` names.
    pub fn consumable_resource_name(&self, name_or_arn: &str) -> Option<String> {
        if let Some(name) = self.consumable_resources_by_arn.get(name_or_arn) {
            return Some(name.clone());
        }
        if self.consumable_resources.contains_key(name_or_arn) {
            return Some(name_or_arn.to_string());
        }
        let id = arn_resource_id(name_or_arn, "consumable-resource")?;
        self.consumable_resources
            .contains_key(id)
            .then(|| id.to_string())
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

    /// The scheduling policy `name_or_arn` names, resolved by ARN (the form every
    /// scheduling-policy member documents) or by plain name.
    pub fn scheduling_policy(&self, name_or_arn: &str) -> Option<&SchedulingPolicy> {
        let name = self.scheduling_policy_name(name_or_arn)?;
        self.scheduling_policies.get(&name)
    }

    /// The NAME of the scheduling policy `name_or_arn` names.
    pub fn scheduling_policy_name(&self, name_or_arn: &str) -> Option<String> {
        if let Some(name) = self.scheduling_policies_by_arn.get(name_or_arn) {
            return Some(name.clone());
        }
        if self.scheduling_policies.contains_key(name_or_arn) {
            return Some(name_or_arn.to_string());
        }
        let id = arn_resource_id(name_or_arn, "scheduling-policy")?;
        self.scheduling_policies
            .contains_key(id)
            .then(|| id.to_string())
    }

    /// `entries` with every `computeEnvironment` resolved against the stored compute
    /// environments and rewritten as that environment's ARN; the refusal detail of the
    /// first entry that resolves to nothing otherwise.
    ///
    /// The normalisation rule is shared by every operation that accepts the list
    /// (`CreateJobQueue`, `UpdateJobQueue`), so a queue reports the ARN whichever form
    /// named the environment.
    ///
    /// An environment this service holds nothing for, and one whose deletion the clock
    /// already owes (so it can never be valid again), are the SAME refusal, and AWS states
    /// it as the requirement rather than as the entry it was given:
    /// [`COMPUTE_ENVIRONMENTS_NOT_ATTACHABLE`].
    pub fn resolve_compute_environment_order(
        &self,
        entries: &[ComputeEnvironmentOrder],
    ) -> Result<Vec<ComputeEnvironmentOrder>, String> {
        entries
            .iter()
            .map(|entry| {
                let environment = self
                    .compute_environment(&entry.compute_environment)
                    .filter(|environment| environment.status != STATUS_DELETING)
                    .ok_or_else(|| COMPUTE_ENVIRONMENTS_NOT_ATTACHABLE.to_string())?;
                Ok(ComputeEnvironmentOrder {
                    order: entry.order,
                    compute_environment: environment.arn.clone(),
                })
            })
            .collect()
    }

    /// `entries` with every `serviceEnvironment` resolved against the stored service
    /// environments and rewritten as that environment's ARN; see
    /// [`Self::resolve_compute_environment_order`].
    ///
    /// An entry naming an environment this service holds nothing for, and one naming an
    /// environment whose deletion the clock already owes (so it can never be valid
    /// again), are the SAME refusal, and AWS states it as the precondition rather than as
    /// the entry it was given: [`SERVICE_ENVIRONMENTS_NOT_ATTACHABLE`].
    pub fn resolve_service_environment_order(
        &self,
        entries: &[ServiceEnvironmentOrder],
    ) -> Result<Vec<ServiceEnvironmentOrder>, String> {
        entries
            .iter()
            .map(|entry| {
                let environment = self
                    .service_environment(&entry.service_environment)
                    .filter(|environment| environment.status != STATUS_DELETING)
                    .ok_or_else(|| SERVICE_ENVIRONMENTS_NOT_ATTACHABLE.to_string())?;
                Ok(ServiceEnvironmentOrder {
                    order: entry.order,
                    service_environment: environment.arn.clone(),
                })
            })
            .collect()
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
    /// this caller saw. That includes the LAST step of a deletion a `Delete*` call
    /// already started: a resource it moved to `DELETING` stops existing when that move
    /// falls due, which was owed at the same instant even if nobody had called.
    ///
    /// The work one call does is bounded. The compute environment, job queue, service
    /// environment and quota share stores are capped by [`limits::MAX_COMPUTE_ENVIRONMENTS`],
    /// [`limits::MAX_JOB_QUEUES`], [`limits::MAX_SERVICE_ENVIRONMENTS`] and
    /// [`limits::MAX_QUOTA_SHARES`],
    /// and the jobs are reached through the due index rather than by scanning: a call
    /// materialises the transitions that have actually fallen due — each of which happens
    /// once in a job's life — plus the jobs waiting on a job that just finished.
    pub fn materialize_due(&mut self, now: i64) {
        // A resource the clock owes a move either settles into `VALID` — whether it is
        // leaving `CREATING` (a create) or `UPDATING` (an update) — or, when a `Delete*`
        // call put it into `DELETING`, stops existing: the same clock-owed transition,
        // so they are materialised in the same pass.
        let mut deleted_environments: Vec<String> = Vec::new();
        for environment in self.compute_environments.values_mut() {
            if environment.status_due_at.is_some_and(|due| due <= now) {
                environment.status_due_at = None;
                if environment.status == STATUS_DELETING {
                    deleted_environments.push(environment.name.clone());
                } else {
                    environment.status = STATUS_VALID.to_string();
                }
            }
        }
        for name in deleted_environments {
            if let Some(removed) = self.compute_environments.remove(&name) {
                self.compute_environments_by_arn.remove(&removed.arn);
            }
        }
        // A queue leaves its transient status (`CREATING` after a create, `UPDATING`
        // after an update) for `VALID` once the span the clock owes has elapsed, and
        // stops existing when the deletion a `DeleteJobQueue` started falls due.
        let mut deleted_queues: Vec<String> = Vec::new();
        for queue in self.job_queues.values_mut() {
            if queue.status_due_at.is_some_and(|due| due <= now) {
                queue.status_due_at = None;
                if queue.status == STATUS_DELETING {
                    deleted_queues.push(queue.name.clone());
                } else {
                    queue.status = STATUS_VALID.to_string();
                }
            }
        }
        for name in deleted_queues {
            if let Some(removed) = self.job_queues.remove(&name) {
                self.job_queues_by_arn.remove(&removed.arn);
                // "All jobs in the queue are eventually terminated when you delete a job
                // queue" (`DeleteJobQueue`): the queue's removal is when that falls due.
                for id in self.jobs_by_queue.remove(&removed.arn).unwrap_or_default() {
                    let live = self
                        .jobs
                        .get(&id)
                        .is_some_and(|job| !is_terminal_status(&job.status));
                    if live {
                        if let Some(job) = self.jobs.get_mut(&id) {
                            job.is_terminated = true;
                        }
                        self.finish_job(&id, JOB_FAILED, JOB_QUEUE_DELETED_REASON, now);
                    }
                }
                // The quota shares that hang off the queue go with it: their ARNs are
                // nested under one nothing resolves any more.
                self.quota_shares
                    .retain(|_, share| share.job_queue_arn != removed.arn);
            }
        }
        for environment in self.service_environments.values_mut() {
            if environment.status_due_at.is_some_and(|due| due <= now) {
                environment.status = STATUS_VALID.to_string();
                environment.status_due_at = None;
            }
        }
        for share in self.quota_shares.values_mut() {
            if share.status_due_at.is_some_and(|due| due <= now) {
                share.status = STATUS_VALID.to_string();
                share.status_due_at = None;
            }
        }
        self.advance_jobs(now);
        self.advance_service_jobs(now);
    }

    /// Advance the service jobs the clock owes a transition at `now`.
    ///
    /// A service job's work is dispatched to another service, which nothing here runs, so
    /// the chain is the same shape as a job's and just as short: `SUBMITTED` ->
    /// `RUNNABLE` -> `FAILED` with [`NO_SERVICE_JOB_RUNTIME_REASON`], each step
    /// [`JOB_PHASE_MILLIS`] after the one before it. A record owes at most those two
    /// moves, and the store is capped by [`limits::MAX_SERVICE_JOBS`], so the work one
    /// call does is bounded. Nothing is invented on the way: a job that was never
    /// dispatched gets no `scheduledAt`, `startedAt` or attempt.
    fn advance_service_jobs(&mut self, now: i64) {
        for job in self.service_jobs.values_mut() {
            while let Some(due) = job.next_phase_due_at.filter(|due| *due <= now) {
                match job.status.as_str() {
                    SERVICE_JOB_SUBMITTED => {
                        job.status = SERVICE_JOB_RUNNABLE.to_string();
                        job.next_phase_due_at = Some(due + JOB_PHASE_MILLIS);
                    }
                    SERVICE_JOB_RUNNABLE => {
                        job.status = SERVICE_JOB_FAILED.to_string();
                        job.status_reason = Some(NO_SERVICE_JOB_RUNTIME_REASON.to_string());
                        job.stopped_at = Some(due);
                        job.next_phase_due_at = None;
                    }
                    // Every other modelled status is one this emulator never moves a
                    // record into, so nothing is owed from it.
                    _ => job.next_phase_due_at = None,
                }
            }
        }
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

    /// Persist a service job, bounded by [`limits::MAX_SERVICE_JOBS`].
    ///
    /// Makes room first by evicting the service jobs that have been terminal longest — a
    /// terminal service job is one no later call can move again — and answers
    /// `Err(message)`, for the modelled `ClientException`, when every service job in a
    /// full store is still live.
    pub fn insert_service_job(&mut self, job: ServiceJob) -> Result<(), String> {
        if self.service_jobs.len() >= limits::MAX_SERVICE_JOBS {
            self.evict_terminal_service_jobs(
                self.service_jobs.len() + 1 - limits::MAX_SERVICE_JOBS,
            );
        }
        if self.service_jobs.len() >= limits::MAX_SERVICE_JOBS {
            return Err(quota_message("service jobs", limits::MAX_SERVICE_JOBS));
        }
        self.service_jobs.insert(job.job_id.clone(), job);
        Ok(())
    }

    /// The service job a `clientToken` already minted, if this store holds one.
    ///
    /// `SubmitServiceJob`'s `clientToken` "is used to ensure idempotency of requests", so
    /// a submit resolves its token here before minting anything. Scans the service jobs,
    /// which [`limits::MAX_SERVICE_JOBS`] bounds, so no second index can drift out of
    /// step with the records it would point at.
    pub fn service_job_by_client_token(&self, token: &str) -> Option<&ServiceJob> {
        self.service_jobs
            .values()
            .find(|job| job.client_token.as_deref() == Some(token))
    }

    /// Drop up to `wanted` of the service jobs that have been terminal longest.
    fn evict_terminal_service_jobs(&mut self, wanted: usize) {
        let mut evictable: Vec<(i64, String)> = self
            .service_jobs
            .values()
            .filter(|job| is_terminal_service_job_status(&job.status))
            .map(|job| (job.stopped_at.unwrap_or(job.created_at), job.job_id.clone()))
            .collect();
        evictable.sort();
        for (_, id) in evictable.into_iter().take(wanted) {
            self.service_jobs.remove(&id);
        }
    }

    /// Terminate the service job `id`, if the store holds one that can still be ended.
    ///
    /// A service job that has not reached a terminal `ServiceJobStatus` ends `FAILED` at
    /// `now`, carrying `reason` as its `statusReason` and `isTerminated` — the two members
    /// `DescribeServiceJob` documents for exactly this (`reason`: "A message to attach to
    /// the service job that explains the reason for terminating it. This message is
    /// returned by DescribeServiceJob operations on the service job."; `isTerminated`:
    /// "Indicates whether the service job has been terminated."). A service job already in
    /// a terminal status is left exactly as it is, so the first reason to end it is the one
    /// it keeps, and an `id` the store holds no service job for changes nothing at all.
    pub fn terminate_service_job(&mut self, id: &str, reason: &str, now: i64) {
        let Some(job) = self.service_jobs.get_mut(id) else {
            return;
        };
        if is_terminal_service_job_status(&job.status) {
            return;
        }
        job.status = SERVICE_JOB_FAILED.to_string();
        job.status_reason = Some(reason.to_string());
        job.stopped_at = Some(now);
        job.is_terminated = true;
        // Terminal: the clock owes the job nothing more.
        job.next_phase_due_at = None;
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

/// Refuse a `jobStateTimeLimitActions` list that names a `maxTimeSeconds` outside the
/// documented bounds; answers the refusal detail of the first offending entry.
///
/// Shared by every operation that accepts the list (`CreateJobQueue`, `UpdateJobQueue`),
/// so one rule covers all of them.
pub fn validate_job_state_time_limit_actions(
    actions: &[JobStateTimeLimitAction],
) -> Result<(), String> {
    for action in actions {
        if !(MIN_JOB_STATE_TIME_LIMIT_SECONDS..=MAX_JOB_STATE_TIME_LIMIT_SECONDS)
            .contains(&action.max_time_seconds)
        {
            return Err(format!(
                "jobStateTimeLimitActions maxTimeSeconds must be between {MIN_JOB_STATE_TIME_LIMIT_SECONDS} and {MAX_JOB_STATE_TIME_LIMIT_SECONDS}, got {}",
                action.max_time_seconds
            ));
        }
    }
    Ok(())
}

/// Whether `value` is a scheduling policy `name` the service accepts: up to
/// [`MAX_SCHEDULING_POLICY_NAME_LEN`] letters, numbers, hyphens and underscores
/// (the member's own doc). See [`is_valid_resource_name`].
pub fn is_valid_scheduling_policy_name(value: &str) -> bool {
    is_valid_resource_name(value, MAX_SCHEDULING_POLICY_NAME_LEN)
}

/// Every `QuotaShareIdleResourceAssignmentStrategy` the model declares
/// (`smithy_query(query="shape", shape="QuotaShareIdleResourceAssignmentStrategy")`).
/// The generated input types carry the member as a plain `String`, so the value is
/// checked here by every operation that accepts a quota-share policy.
pub const QUOTA_SHARE_IDLE_RESOURCE_ASSIGNMENT_STRATEGIES: [&str; 1] = ["FIFO"];

/// The refusal detail when a request carries a policy of neither kind or of both.
///
/// "Only one of fairsharePolicy or quotaSharePolicy can be set" — the doc of
/// `CreateSchedulingPolicy$fairsharePolicy` and `$quotaSharePolicy`. Shared by every
/// operation that accepts the pair (`CreateSchedulingPolicy`, `UpdateSchedulingPolicy`).
pub fn validate_policy_kinds(
    fairshare: Option<&FairsharePolicy>,
    quota_share: Option<&QuotaSharePolicy>,
) -> Result<(), String> {
    if fairshare.is_some() && quota_share.is_some() {
        return Err(
            "Only one of fairsharePolicy or quotaSharePolicy can be set on a scheduling policy"
                .to_string(),
        );
    }
    if let Some(quota_share) = quota_share {
        validate_quota_share_policy(quota_share)?;
    }
    if let Some(fairshare) = fairshare {
        validate_fairshare_policy(fairshare)?;
    }
    Ok(())
}

/// Refuse a `quotaSharePolicy` whose `idleResourceAssignmentStrategy` is not one the
/// model declares ([`QUOTA_SHARE_IDLE_RESOURCE_ASSIGNMENT_STRATEGIES`]).
pub fn validate_quota_share_policy(policy: &QuotaSharePolicy) -> Result<(), String> {
    if !QUOTA_SHARE_IDLE_RESOURCE_ASSIGNMENT_STRATEGIES
        .contains(&policy.idle_resource_assignment_strategy.as_str())
    {
        return Err(format!(
            "idleResourceAssignmentStrategy {} is not one of {}",
            policy.idle_resource_assignment_strategy,
            QUOTA_SHARE_IDLE_RESOURCE_ASSIGNMENT_STRATEGIES.join(", ")
        ));
    }
    Ok(())
}

/// Refuse a `fairsharePolicy` outside the bounds its members document:
/// `shareDecaySeconds` ([`MAX_SHARE_DECAY_SECONDS`]), `computeReservation`
/// ([`MAX_COMPUTE_RESERVATION`]), each `shareIdentifier`'s charset and length
/// ([`MAX_SHARE_IDENTIFIER_LEN`]), each `weightFactor` ([`MIN_WEIGHT_FACTOR`]), and the
/// no-overlap rule between share identifiers.
///
/// The model types every one of these members as a plain `Integer`/`Float`/`String`, so
/// the framework admits anything and the rule is applied by every operation that accepts
/// the policy (`CreateSchedulingPolicy`, `UpdateSchedulingPolicy`).
pub fn validate_fairshare_policy(policy: &FairsharePolicy) -> Result<(), String> {
    if let Some(decay) = policy.share_decay_seconds {
        if !(MIN_SHARE_DECAY_SECONDS..=MAX_SHARE_DECAY_SECONDS).contains(&decay) {
            return Err(format!(
                "fairsharePolicy shareDecaySeconds must be between {MIN_SHARE_DECAY_SECONDS} and {MAX_SHARE_DECAY_SECONDS}, got {decay}"
            ));
        }
    }
    if let Some(reservation) = policy.compute_reservation {
        if !(MIN_COMPUTE_RESERVATION..=MAX_COMPUTE_RESERVATION).contains(&reservation) {
            return Err(format!(
                "fairsharePolicy computeReservation must be between {MIN_COMPUTE_RESERVATION} and {MAX_COMPUTE_RESERVATION}, got {reservation}"
            ));
        }
    }
    let distribution = policy.share_distribution.as_deref().unwrap_or_default();
    // The ENTRY COUNT is bounded before any work proportional to it: the list comes
    // straight from the request, so a longer one is refused rather than walked.
    if distribution.len() > MAX_SHARE_DISTRIBUTION_ENTRIES {
        return Err(format!(
            "fairsharePolicy shareDistribution accepts at most {MAX_SHARE_DISTRIBUTION_ENTRIES} share identifiers, not {}",
            distribution.len()
        ));
    }
    for share in distribution {
        if !is_valid_share_identifier(&share.share_identifier) {
            return Err(format!(
                "shareIdentifier {} must be 1 to {MAX_SHARE_IDENTIFIER_LEN} alphanumeric characters, optionally followed by an asterisk (*)",
                share.share_identifier
            ));
        }
        if let Some(weight) = share.weight_factor {
            if !(MIN_WEIGHT_FACTOR..=MAX_WEIGHT_FACTOR).contains(&weight) {
                return Err(format!(
                    "weightFactor of shareIdentifier {} must be between {MIN_WEIGHT_FACTOR} and {MAX_WEIGHT_FACTOR}, got {weight}",
                    share.share_identifier
                ));
            }
        }
    }
    if let Some((left, right)) = overlapping_share_identifiers(distribution) {
        return Err(format!(
            "The list of share identifiers in a fair-share policy can't overlap: {left} and {right}"
        ));
    }
    Ok(())
}

/// Whether `value` is a `shareIdentifier` the service accepts: at most
/// [`MAX_SHARE_IDENTIFIER_LEN`] alphanumeric characters, optionally followed by a single
/// asterisk which makes the entry a PREFIX (the member's own doc).
pub fn is_valid_share_identifier(value: &str) -> bool {
    let prefix = value.strip_suffix('*').unwrap_or(value);
    !prefix.is_empty()
        && prefix.chars().count() <= MAX_SHARE_IDENTIFIER_LEN
        && prefix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A pair of overlapping share identifiers in `distribution`, or `None`.
///
/// Two entries overlap when one selects everything the other does: the same identifier
/// twice, or a prefix entry (`<prefix>*`) and any entry that starts with that prefix —
/// the example the `shareIdentifier` doc gives ("you can't have one that specifies a
/// shareIdentifier of UserA* and another that specifies a shareIdentifier of UserA1").
///
/// SORTED rather than pairwise, so the cost is `n log n` comparisons and not `n²`: in
/// identifier order every entry that starts with a given prefix follows it contiguously,
/// so an overlapping pair always leaves an ADJACENT overlapping pair behind — if `a` is a
/// prefix of `c` then every `b` between them has `a` as a prefix too, and an overlap
/// between two different identifiers is only ever an earlier PREFIX entry against a later
/// one. Checking neighbours therefore finds an overlap exactly when one exists.
fn overlapping_share_identifiers(distribution: &[ShareAttributes]) -> Option<(String, String)> {
    // The identifier an entry selects on: the string without the `*` that makes it a
    // prefix. The full string breaks ties so the answer does not depend on input order.
    let sort_key = |value: &str| {
        (
            value.strip_suffix('*').unwrap_or(value).to_string(),
            value.to_string(),
        )
    };
    let mut sorted: Vec<&str> = distribution
        .iter()
        .map(|share| share.share_identifier.as_str())
        .collect();
    sorted.sort_unstable_by_key(|value| sort_key(value));
    sorted.windows(2).find_map(|pair| {
        share_identifiers_overlap(pair[0], pair[1])
            .then(|| (pair[0].to_string(), pair[1].to_string()))
    })
}

/// Whether two `shareIdentifier` entries select a common share identifier.
fn share_identifiers_overlap(left: &str, right: &str) -> bool {
    match (left.strip_suffix('*'), right.strip_suffix('*')) {
        // Two prefixes overlap when either contains the other.
        (Some(left), Some(right)) => left.starts_with(right) || right.starts_with(left),
        // A prefix and an exact identifier overlap when the identifier matches it.
        (Some(prefix), None) => right.starts_with(prefix),
        (None, Some(prefix)) => left.starts_with(prefix),
        // Two exact identifiers overlap only when they are the same.
        (None, None) => left == right,
    }
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

/// A `ClientException` whose message is `detail` exactly as the code raising it states
/// it — the 400 an unknown, duplicate, invalid or over-quota request answers with.
///
/// AWS Batch does NOT wrap every refusal in its frontend envelope: the recorded
/// refusals of `CreateJobQueue` ([`COMPUTE_ENVIRONMENTS_NOT_ATTACHABLE`]),
/// `CreateComputeEnvironment` ([`OBJECT_ALREADY_EXISTS`]) and `DescribeJobDefinitions`
/// ([`NEXT_TOKEN_NOT_PROCESSABLE`]) carry the detail alone. So the envelope is chosen
/// per refusal, at the site that raises it ([`client_exception_enveloped`]), rather than
/// applied to all of them here.
pub fn client_exception(detail: impl AsRef<str>) -> ClientException {
    ClientException {
        message: Some(detail.as_ref().to_string()),
    }
}

/// A `ClientException` whose message is `detail` inside Batch's frontend envelope
/// ([`client_exception_message`]) — for the refusals AWS is recorded to wrap, such as
/// `RegisterJobDefinition`'s missing properties object and the quota-share refusals.
pub fn client_exception_enveloped(detail: impl AsRef<str>) -> ClientException {
    ClientException {
        message: Some(client_exception_message(detail.as_ref(), &new_request_id())),
    }
}

/// The `ServerException` for an engine that did not hand back this service's store.
pub fn unreachable_state() -> ServerException {
    ServerException {
        message: Some(UNREACHABLE_STATE.to_string()),
    }
}

/// A `ClientException` for a reference that resolves to no resource, worded the way the
/// Batch service layer words a not-found: `<Kind> <reference> not found.`
///
/// This is the OTHER wording a Batch `ClientException` carries. A refusal the frontend
/// raises is wrapped by [`client_exception_message`]; a lookup the service itself performs
/// and misses answers this bare, request-derived sentence naming the kind of resource and
/// the reference the caller gave — the observed wording of, for instance,
/// `SubmitServiceJob` against a `jobQueue` nothing answers to. The rule is the sentence,
/// not any one reference: the caller's own text is what fills it.
pub fn not_found_exception(kind: &str, reference: &str) -> ClientException {
    ClientException {
        message: Some(format!("{kind} {reference} not found.")),
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

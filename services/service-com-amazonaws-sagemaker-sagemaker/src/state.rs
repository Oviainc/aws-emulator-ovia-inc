// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemaker-sagemaker service.
//!
//! The inference-deployment lifecycle (model -> endpoint config -> endpoint), the
//! hyperparameter tuning jobs, the pipeline lifecycle, the model-card governance
//! resources, batch-transform jobs and the AI benchmarking control plane's workload
//! configurations and benchmark jobs are kept here, in
//! `BTreeMap`s keyed by the resource name AWS keys them by, so `ListModels` has one
//! stable order to sort and paginate over and every `Describe*` replays exactly what
//! the matching `Create*` stored. The Studio AppImageConfig registry, the Git
//! repositories `CreateCodeRepository` registers and the processing jobs
//! `CreateProcessingJob` accepts are kept the same way, keyed by `AppImageConfigName`,
//! `CodeRepositoryName` and `ProcessingJobName`, and so is the model-package-group
//! registry.
//!
//! The lineage entities (context, artifact) are keyed by their ARN instead: SageMaker
//! documents their names as unique to an account IN A REGION (and an artifact need not
//! have a name at all), so the ARN — which carries the Region and the account beside the
//! name — is what identifies one of them.
//!
//! The lineage groups the account's lineage entities belong to are kept the same way,
//! keyed by `(Region, LineageGroupName)` because a lineage group is a REGIONAL resource —
//! with no `Create*` of their own, because SageMaker models none: see
//! [`ComAmazonawsSagemakerSagemakerState::ensure_lineage_group`]. `DescribeLineageGroup`
//! reports one of them and `ListLineageGroups` is a pure read over the Region's.
//!
//! Studio apps live here too, keyed by the [`AppKey`] their five input members name
//! rather than by a bare name, because that tuple — domain, owning user profile or
//! space, app type, app name — is what identifies one app. The ML Lineage entities
//! (artifacts, actions, trial components) are keyed by the ARN or name their own slices
//! document, and the associations BETWEEN them by the `(SourceArn, DestinationArn)` pair
//! that is one edge's whole identity.
//!
//! # A job's status is moved by the CLOCK, never by a read
//!
//! A resource AWS returns in a non-terminal status (an endpoint in `Creating`, a
//! processing job in `InProgress`) owes its next move to the clock: the instant it falls
//! due is stored at create time and the reporting operations only materialise what is
//! already due ([`Endpoint::settle_status`], [`ProcessingJob::settle_status`]), so a read
//! never changes what a later caller sees and the transient status is observable because
//! the span is never zero.
//!
//! # An entity's own collections are ALWAYS reported, and empty when it holds nothing
//!
//! The ML Lineage and experiment-tracking readers report every collection an entity owns
//! — a lineage entity's properties, a trial component's parameters and artifacts, the
//! source types of an artifact's source, the metrics and sources nothing has produced for
//! a component — whatever the creating request carried: an entity created with none of
//! them reports the EMPTY map or list, not a missing member, because the member is how the
//! entity reports what it holds rather than what the request mentioned. So the records
//! below hold those collections directly rather than as an `Option` of one, and the write
//! path resolves an absent request member to the empty collection ONCE, where the record
//! is written, instead of leaving each reader to decide again.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

/// The AutoML job V2 slice: the record, its clock and its constants. Re-exported below,
/// so a handler names them as `crate::state::…` like every other slice.
pub mod auto_ml_job_v2;
pub use auto_ml_job_v2::{
    auto_ml_problem_type_config_name, AutoMLJobV2, AUTO_ML_JOB_KIND_LABEL, AUTO_ML_JOB_QUOTA,
    AUTO_ML_JOB_TERMINAL_AFTER, AUTO_ML_NOT_TRAINED_FAILURE_REASON,
    AUTO_ML_SECONDARY_STATUS_FAILED, AUTO_ML_SECONDARY_STATUS_STARTING, AUTO_ML_STATUS_FAILED,
    AUTO_ML_STATUS_IN_PROGRESS,
};

use crate::ops::types::{
    AdditionalInferenceSpecificationDefinition, AppSpecification, ArtifactSource,
    AsyncInferenceConfig, AutoMLComputeConfig, AutoMLDataSplitConfig, AutoMLJobChannel,
    AutoMLJobObjective, AutoMLOutputDataConfig, AutoMLProblemTypeConfig, AutoMLSecurityConfig,
    Autotune, BatchDataCaptureConfig, CodeEditorAppImageConfig, ContainerDefinition,
    DataCaptureConfig, DataProcessing, DriftCheckBaselines, ExperimentConfig, ExplainerConfig,
    GitConfig, HyperParameterTrainingJobDefinition, HyperParameterTuningJobConfig,
    HyperParameterTuningJobWarmStartConfig, IamIdentity, InferenceExecutionConfig,
    InferenceSpecification, JupyterLabAppImageConfig, KernelGatewayImageConfig,
    ManagedConfiguration, MetadataProperties, MetricsConfig, ModelCardExportOutputConfig,
    ModelCardSecurityConfig, ModelClientConfig, ModelDeployConfig, ModelLifeCycle, ModelMetrics,
    ModelPackageModelCard, ModelPackageSecurityConfig, ModelPackageContainerDefinition,
    ModelPackageStatusDetails,
    ModelPackageStatusItem, ModelPackageValidationSpecification, NetworkConfig,
    ParallelismConfiguration, Parameter, ProcessingInput, ProcessingOutputConfig,
    ProcessingResources, ProcessingStoppingCondition, ProductionVariant, ProductionVariantSummary,
    RecommendationJobInputConfig, RecommendationJobOutputConfig,
    RecommendationJobStoppingConditions, ResourceSpec, SecondaryStatusTransition,
    SelectiveExecutionConfig,
    SourceAlgorithmSpecification, Tag, TrainingSpecification, TransformInput, TransformOutput,
    TransformResources,
    UserContext, VpcConfig,
};

/// The AI recommendation job slice — the record `CreateAIRecommendationJob` writes,
/// `DescribeAIRecommendationJob` replays and `DeleteAIRecommendationJob` releases, with
/// the ARN kind, quota, statuses, clock span and failure reason that belong to it. Kept
/// in its own file (as the CloudFormation service keeps `state::template`) so this one
/// resource kind reads as one unit instead of another block at the end of this module.
pub mod ai_recommendation_job;

pub use ai_recommendation_job::{
    ai_recommendation_job_not_found, ai_recommendation_job_not_running, AIRecommendationJob,
    AI_RECOMMENDATION_JOB_ENDS_AFTER, AI_RECOMMENDATION_JOB_ENTITY, AI_RECOMMENDATION_JOB_KIND,
    AI_RECOMMENDATION_JOB_QUOTA, AI_RECOMMENDATION_JOB_STOPS_AFTER,
    AI_RECOMMENDATION_OPTIMIZE_MODEL_DEFAULT, AI_RECOMMENDATION_STATUS_FAILED,
    AI_RECOMMENDATION_STATUS_IN_PROGRESS, AI_RECOMMENDATION_STATUS_STOPPED,
    AI_RECOMMENDATION_STATUS_STOPPING, NO_RECOMMENDATION_RUNTIME_REASON,
};

/// The model-customization job slice: the record `CreateJob` persists, the lookup every
/// job operation performs and the constants that bound and word it. Re-exported below,
/// so a handler names them as `crate::state::…` like every other slice.
pub mod job;
pub use job::{
    Job, JOB_ARN_KIND, JOB_KIND_LABEL, JOB_QUOTA, JOB_SECONDARY_STATUS_STARTING,
    JOB_STATUS_IN_PROGRESS,
};

/// The cluster-policy slice of this state: HyperPod clusters and the cluster policies
/// on them, with their own records, quota, status span and id mint. Reached through the
/// single [`ComAmazonawsSagemakerSagemakerState::cluster_policies`] field.
pub mod cluster_policy;

/// The reader half of the HyperPod cluster slice: the one lookup that resolves the
/// `ClusterNameOrArn` every cluster operation takes, and the `*Details` / `*Output`
/// projections a reader builds from the [`Cluster`] record this module stores.
pub mod cluster;

/// The ARN service namespace (`arnNamespace` in the model's `aws.api#service` trait,
/// also SageMaker's sigv4 signing name).
pub const ARN_SERVICE: &str = "sagemaker";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. It matches the region the emulator's generated shared config
/// hands the AWS CLI.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after `CreateEndpoint` the clock owes the `Creating` -> `InService`
/// transition. A caller that describes the endpoint straight after creating it still
/// observes `Creating`; one that waits (as `aws sagemaker wait endpoint-in-service`
/// does) observes `InService`, so a waiter terminates. Real SageMaker takes minutes;
/// the span is compressed, never zero.
pub const ENDPOINT_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// How long after `CreateMlflowApp` the clock owes the `Creating` -> `Created`
/// transition of `MlflowAppStatus`.
///
/// A caller that describes the app straight after creating it still observes
/// `Creating`; one that waits observes `Created`. Real SageMaker takes minutes to stand
/// an MLflow app up; the span is compressed, never zero, and the due instant is stored
/// rather than slept on.
pub const MLFLOW_APP_CREATED_AFTER: Duration = Duration::from_millis(300);

/// How long after `CreateApp` the clock owes the `Pending` -> `InService` transition.
///
/// Same contract as [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the app
/// straight after creating it still observes `Pending`, one that waits observes
/// `InService`. Real Studio app launches take minutes; the span is compressed, never
/// zero.
pub const APP_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// The most records each of this service's stores holds, after which a create is
/// refused with the modelled `ResourceLimitExceeded`.
///
/// SageMaker publishes adjustable per-account quotas on the resources this service
/// owns, and this emulator models neither Service Quotas nor an
/// account's adjustments — so this is the emulator's OWN bound on each store rather than
/// a claim about any account's limit. It sits far above any workflow an emulator serves,
/// and it is what stops the state growing without end: every record it holds is released
/// by the matching `Delete*`.
pub const MODEL_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const ENDPOINT_CONFIG_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const ENDPOINT_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. A transform job is terminal and has no delete, so this is the
/// most transform jobs one emulator run holds.
pub const TRANSFORM_JOB_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. `CreateMlflowApp` models `ResourceLimitExceeded` for exactly
/// this refusal.
pub const MLFLOW_APP_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Counts model packages of both kinds (unversioned packages and
/// the versions registered into model package groups), which is what `CreateModelPackage`
/// mints and `DeleteModelPackage` releases.
pub const MODEL_PACKAGE_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const TRIAL_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds the compute-allocation store, and is what
/// `CreateComputeQuota` answers `ResourceLimitExceeded` past.
pub const COMPUTE_QUOTA_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const PIPELINE_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const APP_IMAGE_CONFIG_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const CODE_REPOSITORY_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const APP_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const MODEL_PACKAGE_GROUP_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`] — this one is per cluster, the resource that holds the nodes.
pub const CLUSTER_NODE_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds the lineage-artifact store the same way, and is what
/// `CreateArtifact` answers `ResourceLimitExceeded` past.
pub const ARTIFACT_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds the model-card store, and is what `CreateModelCard`
/// answers `ResourceLimitExceeded` past.
pub const MODEL_CARD_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds how many versions ONE model card can hold, and is what
/// `UpdateModelCard` answers `ResourceLimitExceeded` past: every content update appends
/// a version, so without a bound one card could grow the store without end.
pub const MODEL_CARD_VERSION_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds the model-card export-job store, and is what
/// `CreateModelCardExportJob` answers `ResourceLimitExceeded` past: an export job is
/// terminal and has no delete, so this is the most one emulator run can start.
pub const MODEL_CARD_EXPORT_JOB_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const PROCESSING_JOB_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. `DeleteTrainingJob` is still a stub, so no call releases a
/// training-job record yet; the bound is what keeps the store finite in the meantime.
pub const TRAINING_JOB_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds the lineage-group store as a whole, across Regions: a
/// group is materialised per Region, so this is the total the store holds rather than a
/// per-Region count. Declared last in this block, with the lineage-group record and its
/// two accessors at the END of this file, so a service growing another store does not
/// collide with the lineage-group one.
pub const LINEAGE_GROUP_QUOTA: usize = 1_000;

/// The most `BatchAddClusterNodes` client tokens one cluster remembers. See
/// [`Cluster::remember_node_addition`].
pub const NODE_ADDITION_HISTORY_CAP: usize = 64;

/// The most presigned-URL grants one partner app keeps, so a caller asking for URL after
/// URL cannot grow the store without end; a grant past the cap is forgotten, which a
/// caller cannot tell from the expiry it was minted with. Beside the other store bounds,
/// next to the [`PartnerApp`] it bounds.
pub const GRANT_HISTORY_CAP: usize = 32;

/// How long a presigned partner-app URL stays valid when the request names no
/// `ExpiresInSeconds`.
///
/// The emulator's OWN choice, not a published AWS default: the model documents none. The
/// widest value `ExpiresInSeconds` allows (`@range 5..300`), so a caller that names no
/// value gets a link a human has time to follow. Beside [`GRANT_HISTORY_CAP`], with the
/// other knobs of the [`PartnerApp`] record they govern.
pub const DEFAULT_PRESIGNED_URL_EXPIRES_IN_SECONDS: i32 = 300;

/// How long the session a presigned partner-app URL opens lasts when the request names
/// no `SessionExpirationDurationInSeconds`.
///
/// The emulator's OWN choice, not a published AWS default: the narrowest value the model
/// allows (`@range 1800..43200`), so an unnamed session is the shortest one rather than
/// the longest.
pub const DEFAULT_PRESIGNED_URL_SESSION_SECONDS: i32 = 1800;

/// The lineage group every lineage entity this account creates belongs to.
///
/// SageMaker ML Lineage Tracking gives an account one lineage group per Region,
/// `sagemaker-default-lineage-group`, and reports its ARN as the `LineageGroupArn` of
/// each entity in it; this emulator models no other lineage group, so every artifact
/// reports this one. It is also the group the lineage-entity creates materialise through
/// [`ComAmazonawsSagemakerSagemakerState::ensure_lineage_group`], so that
/// `DescribeLineageGroup` reports the group those entities are in and
/// `ListLineageGroups` lists it.
pub const DEFAULT_LINEAGE_GROUP: &str = "sagemaker-default-lineage-group";

/// See [`MODEL_QUOTA`]. `DeleteHyperParameterTuningJob` is still a stub, so no call
/// releases a tuning-job record yet; the bound is what keeps the store finite in the
/// meantime.
pub const HYPER_PARAMETER_TUNING_JOB_QUOTA: usize = 1_000;

/// The most training jobs one hyperparameter tuning job may be asked to launch
/// (`ResourceLimits$MaxNumberOfTrainingJobs`).
///
/// SageMaker publishes this as an adjustable account quota — named by
/// [`TRAINING_JOBS_PER_TUNING_JOB_QUOTA_NAME`] — and refuses a larger request with the
/// modelled `ResourceLimitExceeded`. This emulator models neither Service Quotas nor an
/// account's adjustments, so every account is held to the published default.
pub const MAX_TRAINING_JOBS_PER_TUNING_JOB: i32 = 750;

/// The most training jobs one hyperparameter tuning job may run at a time
/// (`ResourceLimits$MaxParallelTrainingJobs`), from the published adjustable quota
/// [`PARALLEL_TRAINING_JOBS_PER_TUNING_JOB_QUOTA_NAME`]. See
/// [`MAX_TRAINING_JOBS_PER_TUNING_JOB`] for why the default is what every account gets.
pub const MAX_PARALLEL_TRAINING_JOBS_PER_TUNING_JOB: i32 = 10;

/// The published name of the quota [`MAX_TRAINING_JOBS_PER_TUNING_JOB`] is the default
/// of, as a refusal states it. INFERRED from the sibling
/// [`PARALLEL_TRAINING_JOBS_PER_TUNING_JOB_QUOTA_NAME`], which is the one of the pair a
/// recorded refusal spells out: the two dimensions are told apart by name, so this one is
/// worded in that family's shape. A later recording of the total-jobs refusal is what
/// would settle it.
pub const TRAINING_JOBS_PER_TUNING_JOB_QUOTA_NAME: &str =
    "Maximum number of training jobs each hyper parameter tuning job can run";
/// The published name of the quota [`MAX_PARALLEL_TRAINING_JOBS_PER_TUNING_JOB`] is the
/// default of, as a refusal states it.
pub const PARALLEL_TRAINING_JOBS_PER_TUNING_JOB_QUOTA_NAME: &str =
    "Maximum number of training jobs each hyper parameter tuning job can run in parallel \
     at once";

/// The unit [`account_quota_exceeded`] counts a training-job quota in: what the limit,
/// the utilisation and the delta of such a quota are numbers OF.
pub const TRAINING_JOBS_UNIT: &str = "TrainingJobs";

/// How SageMaker words a refusal of a request that is past an adjustable account quota
/// AWS PUBLISHES: the quota by its published name, its limit, what the account is already
/// using of it and what this request asks for on top — each count followed by the UNIT
/// the quota is counted in — and then how to ask for more.
///
/// The four numbers are the point of the template: a client reads the limit against the
/// utilisation to decide whether to ask for less or to request an increase, so none of
/// them is left out and none is replaced by prose about one member of the request.
/// Stated once here so every operation that evaluates a published account quota words it
/// the same way.
///
/// This is NOT the wording for the emulator's own bound on a store (see [`MODEL_QUOTA`]):
/// that bound is not a published quota, so a refusal past it states the bound in its own
/// words rather than claiming Service Quotas can raise it.
pub fn account_quota_exceeded(
    quota: &str,
    unit: &str,
    limit: i64,
    in_use: i64,
    delta: i64,
) -> String {
    format!(
        "The account-level service limit '{quota}' is {limit} {unit}, with current \
         utilization of {in_use} {unit} and a request delta of {delta} {unit}. Please use \
         AWS Service Quotas to request an increase for this quota. If AWS Service Quotas \
         is not available, contact AWS support to request an increase for this quota."
    )
}

/// The most hyperparameters one tuning job may search over, across
/// `ParameterRanges`' integer, continuous and categorical lists together.
///
/// The model bounds each of those lists at 30 entries and `ParameterRanges`' own doc
/// says that figure "refers to the maximum number of hyperparameters for each range and
/// also the maximum for the hyperparameter tuning job itself" — the sum is held to the
/// same bound the model states per list.
pub const MAX_SEARCHED_HYPERPARAMETERS: usize = 30;

/// How long after `CreateHyperParameterTuningJob` the clock owes the tuning job's
/// `InProgress` -> terminal transition.
///
/// A caller that describes the job straight after creating it still observes
/// `InProgress`; one that waits observes the terminal status, so a waiter terminates.
/// Real tuning jobs take hours; the span is compressed, never zero, and sits at the top
/// of the band this emulator uses for a clock-owed transition so a caller (or a test)
/// reading the transient status right after the create is not racing the clock.
pub const TUNING_JOB_SETTLES_AFTER: Duration = Duration::from_millis(700);

/// `HyperParameterTuningJobStatus`'s members this emulator moves a tuning job through.
pub const TUNING_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`TUNING_STATUS_IN_PROGRESS`].
pub const TUNING_STATUS_FAILED: &str = "Failed";
/// See [`TUNING_STATUS_IN_PROGRESS`]: the transient status
/// `StopHyperParameterTuningJob` moves a running job into.
pub const TUNING_STATUS_STOPPING: &str = "Stopping";
/// See [`TUNING_STATUS_IN_PROGRESS`]: the terminal status the clock owes a job that was
/// asked to stop, after which "it releases all reserved resources for the tuning job"
/// (`StopHyperParameterTuningJob`'s own doc).
pub const TUNING_STATUS_STOPPED: &str = "Stopped";

/// How long after `StopHyperParameterTuningJob` the clock owes the job's `Stopping` ->
/// `Stopped` transition.
///
/// Same contract as [`TUNING_JOB_SETTLES_AFTER`] and chosen on the same terms (AWS
/// documents no duration for the stop): never zero, so a caller that describes the job
/// straight after asking it to stop still observes `Stopping`, and one that waits
/// observes `Stopped`.
pub const TUNING_JOB_STOPS_AFTER: Duration = Duration::from_millis(300);

/// Why a hyperparameter tuning job this emulator accepted ends in `Failed`.
///
/// A tuning job's work is the training jobs it launches, and this emulator runs no
/// training container: nothing evaluates an objective metric and nothing writes model
/// artifacts. Reporting `Completed`, a best training job or a metric value would claim
/// results of work that never ran, so the job ends in the modelled failure status
/// carrying this reason instead.
pub const NO_TRAINING_RUNNER_FAILURE_REASON: &str =
    "This emulator does not run training jobs: no training job was launched for this \
hyperparameter tuning job, so no objective metric was evaluated and no model artifacts were \
written to the output S3 location.";

/// How long after `CreateProcessingJob` the clock owes the job's move out of
/// `InProgress` into its terminal status.
///
/// A span chosen by this emulator (AWS documents no duration for a processing run),
/// inside the 250–750 ms band the project fixes for a transition the clock owes. It is
/// never zero, so a caller that describes the job straight after creating it still
/// observes `InProgress`, and one that waits (as `aws sagemaker wait
/// processing-job-completed` does) observes the terminal status, so a waiter terminates.
pub const PROCESSING_JOB_ENDS_AFTER: Duration = Duration::from_millis(400);

/// `ProcessingJobStatus`'s members this emulator moves a processing job through (the
/// modelled enum also carries `Completed`, which this slice does not reach because
/// nothing runs the job's container image). Named once here so no handler compares a
/// bare string literal.
pub const PROCESSING_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`PROCESSING_STATUS_IN_PROGRESS`].
pub const PROCESSING_STATUS_FAILED: &str = "Failed";
/// See [`PROCESSING_STATUS_IN_PROGRESS`]: the transient status a job carries from the
/// `StopProcessingJob` call until the clock owes it [`PROCESSING_STATUS_STOPPED`].
pub const PROCESSING_STATUS_STOPPING: &str = "Stopping";
/// See [`PROCESSING_STATUS_STOPPING`]: the terminal status of a job the caller stopped.
pub const PROCESSING_STATUS_STOPPED: &str = "Stopped";

/// How long after `StopProcessingJob` the clock owes the job's `Stopping` -> `Stopped`
/// move.
///
/// Chosen on the same terms as [`PROCESSING_JOB_ENDS_AFTER`] (AWS documents no duration
/// for the stop either) and inside the same band: never zero, so a caller that describes
/// the job straight after asking it to stop still observes the transient `Stopping`, and
/// one that waits observes `Stopped`, so a waiter terminates.
pub const PROCESSING_JOB_STOPS_AFTER: Duration = Duration::from_millis(300);

/// How the processing-job API words a `ProcessingJobName` that resolves to no job for a
/// call that only REPORTS the job: fixed text, then the name the request presented.
///
/// Recorded wire behaviour: the service answers this one with the error code
/// `ValidationException` and HTTP 400, not with the `ResourceNotFound` the model lists, so
/// the handler returns it through the validation channel. Named once here so the reader
/// cannot word the fact differently from the template below.
pub fn processing_job_not_found_for_read(name: &str) -> String {
    format!("Could not find requested job with name: {name}")
}

/// How the processing-job API words a `ProcessingJobName` that resolves to no job for a
/// call that would CHANGE the job — the stop: fixed text, then the name the request
/// presented.
///
/// A separate template from [`processing_job_not_found_for_read`] because the service
/// words the two differently, and carried on the same `ValidationException` / HTTP 400
/// envelope for the same reason.
pub fn processing_job_not_found_for_update(name: &str) -> String {
    format!("Could not find job to update with name: {name}")
}

/// Why stopping a job whose run is already over is refused, naming the job and the status
/// it had already reached.
///
/// A job in a status nothing leaves on its own has no run left to stop.
/// `StopProcessingJob` models only `ResourceNotFound` and the job IS found, so the
/// refusal is a 400 `ValidationException` carrying this sentence.
pub fn processing_job_not_running(name: &str, status: &str) -> String {
    format!(
        "The processing job \"{name}\" is {status} and is not running, so there is \
         nothing to stop."
    )
}

/// `FailureReason` of a processing job whose container image could not be run here.
///
/// A processing job runs a customer Docker image (`AppSpecification$ImageUri`). This
/// emulator has no container runtime, and the engine contract
/// (`vendor/engine/src/lib.rs`) forbids reporting a status, duration or exit code for
/// work nothing ran — so the run the caller asked for ends in the modelled `Failed`
/// state with this reason, and no `ProcessingStartTime` or `ExitMessage` is invented.
/// Everything around the run — accepting the job, validating it, minting its ARN,
/// replaying it, reporting its terminal status — is real.
pub const NO_CONTAINER_RUNTIME_REASON: &str =
    "This emulator has no container runtime, so the processing job's container image was \
     never started";

/// The instance count a processing cluster runs when the request names none, per
/// `ProcessingClusterConfig$InstanceCount`'s doc: "The default value is 1."
pub const DEFAULT_PROCESSING_INSTANCE_COUNT: i32 = 1;

/// `PipelineStatus`'s members (`smithy_query(query="shape",shape="PipelineStatus")`
/// answers exactly `Active` and `Deleting`). Named once here so no handler compares a
/// bare string literal.
///
/// Only `Active` is ever reported: recorded AWS makes a pipeline unreadable from the
/// `DeletePipeline` call on, so no reader observes a pipeline being deleted and there is
/// no transient status for `DeletePipeline` to leave behind.
pub const PIPELINE_STATUS_ACTIVE: &str = "Active";

/// The one pipeline-definition schema version SageMaker Pipelines publishes, and the
/// value a definition's required `Version` key carries.
pub const PIPELINE_DEFINITION_VERSION: &str = "2020-12-01";

/// The `Type` of the one step kind that holds steps of its own, in the branches the
/// schema gives it (`IfSteps` / `ElseSteps`). Every other step's `Arguments` are its own
/// payload, so nothing inside them is walked as a pipeline step.
const PIPELINE_CONDITION_STEP_TYPE: &str = "Condition";

/// The `Type` of the step kind that ENDS a run: it stops the pipeline with its own error
/// message, so nothing follows it.
const PIPELINE_FAIL_STEP_TYPE: &str = "Fail";

/// The step `Type`s no `DependsOn` entry may name as its target.
///
/// A `DependsOn` edge waits for the target step to COMPLETE, and neither of these two
/// kinds completes into something a successor can run after: a fail step ends the run,
/// and a condition step hands control to one of its own branches rather than to the
/// steps declared beside it. Each is refused by name, so a caller is told which kind it
/// tried to wait on.
const PIPELINE_UNDEPENDABLE_STEP_TYPES: [&str; 2] =
    [PIPELINE_FAIL_STEP_TYPE, PIPELINE_CONDITION_STEP_TYPE];

/// The key a pipeline definition writes a property reference under: the schema's
/// `{"Get": "<reference>"}` form, which is how a step's arguments name a value resolved
/// when the pipeline runs.
const PIPELINE_PROPERTY_REFERENCE_KEY: &str = "Get";

/// The property-reference prefix that addresses one of the definition's own parameters,
/// i.e. a reference of the form `Parameters.<name>`, where `<name>` is a name the
/// definition's top-level `Parameters` list declares.
const PIPELINE_PARAMETER_REFERENCE_PREFIX: &str = "Parameters.";

/// The version id a pipeline carries from the moment it is created. `UpdatePipeline`
/// mints later ones; nothing here does yet, so a created pipeline has exactly this one.
pub const PIPELINE_INITIAL_VERSION: i64 = 1;

/// `PipelineExecutionStatus`'s members (`smithy_query(query="shape",
/// shape="PipelineExecutionStatus")` answers `Executing`, `Stopping`, `Stopped`,
/// `Failed`, `Succeeded` and `CompletedWithFailures`). Named here so no handler compares
/// a bare string literal; the two an execution this emulator starts ever reports are
/// the non-terminal one it begins in and the terminal one [`PIPELINE_EXECUTION_NOT_RUN`]
/// explains.
pub const PIPELINE_EXECUTION_STATUS_EXECUTING: &str = "Executing";
/// See [`PIPELINE_EXECUTION_STATUS_EXECUTING`].
pub const PIPELINE_EXECUTION_STATUS_FAILED: &str = "Failed";

/// The most steps one selective run may name, and the least.
///
/// `SelectedStepList` is modelled `len 1..50`
/// (`smithy_query(query="shape",shape="SelectiveExecutionConfig")`), so a longer or an
/// empty list is outside the model. The bound is restated here because it is what keeps
/// the step-membership check, and the size of the record the run persists, proportional
/// to the model rather than to the request's own length.
pub const MIN_SELECTED_STEPS: usize = 1;
/// See [`MIN_SELECTED_STEPS`].
pub const MAX_SELECTED_STEPS: usize = 50;

/// How long after `StartPipelineExecution` the clock owes the execution's terminal
/// status.
///
/// Same contract as [`PROCESSING_JOB_ENDS_AFTER`]: a caller that describes the execution
/// straight after starting it still observes `Executing`, one that waits observes the
/// terminal status. The span is the emulator's own, compressed and never zero, and the
/// due instant is stored rather than slept on.
pub const PIPELINE_EXECUTION_ENDS_AFTER: Duration = Duration::from_millis(400);

/// Why an execution this emulator started ends [`PIPELINE_EXECUTION_STATUS_FAILED`].
///
/// A pipeline's steps are training, processing, transform, Lambda and callback work run
/// outside SageMaker's control plane, and this emulator has no runtime that performs any
/// of it. So an accepted execution is never reported `Succeeded`: nothing ran its steps,
/// and a success status, a job ARN, a step output or a runtime for work nothing performed
/// would be fabricated. The execution instead ends in the modelled failure state with
/// this reason, which says exactly that, and the steps the run was to perform report the
/// same failure state and the same reason — see [`step_status_of_execution`] and
/// [`pipeline_definition_step_declarations`], which is what
/// `ListPipelineExecutionSteps` reports them from. Within the modelled
/// `PipelineExecutionFailureReason` length (0..1300), and within the shorter
/// `FailureReason` (0..1024) a step execution reports it under.
pub const PIPELINE_EXECUTION_NOT_RUN: &str =
    "The pipeline execution was accepted but no step of it ran: this emulator has no \
     runtime that performs a pipeline step's work (training, processing, transform, \
     Lambda or callback), so the execution ended Failed rather than reporting work that \
     did not happen.";

/// The `StepStatus` the steps of an execution in `execution_status` report, or `None`
/// while that execution has reached no step at all.
///
/// A step execution belongs to the run that performs it, so its status follows the run's:
/// `PipelineExecutionStatus` and `StepStatus` share every member
/// (`smithy_query(query="shape",shape="StepStatus")` answers `Starting`, `Executing`,
/// `Stopping`, `Stopped`, `Failed`, `Succeeded`) but `CompletedWithFailures`, which names
/// no step's own outcome. An execution still [`PIPELINE_EXECUTION_STATUS_EXECUTING`] has
/// reached no step yet — the clock has not settled it, so nothing about any step of it is
/// known — and a status with no `StepStatus` member of its own reports no step rather
/// than one in a status the model does not name.
pub fn step_status_of_execution(execution_status: &str) -> Option<&'static str> {
    if execution_status == PIPELINE_EXECUTION_STATUS_EXECUTING {
        return None;
    }
    crate::ops::types::STEP_STATUS_VALUES
        .iter()
        .copied()
        .find(|member| *member == execution_status)
}

/// See [`MODEL_QUOTA`]. Bounds the pipeline-execution store, so a caller starting
/// executions cannot grow it without end; `StartPipelineExecution` refuses past it with
/// the modelled `ResourceLimitExceeded`.
pub const PIPELINE_EXECUTION_QUOTA: usize = 1_000;

/// How deeply a pipeline definition's steps may nest (a condition step's branches hold
/// steps, which may themselves be condition steps). A bound, so a definition cannot
/// drive the step walk below into unbounded recursion.
const PIPELINE_MAX_STEP_DEPTH: usize = 32;

/// The most steps one pipeline definition may declare, counting the steps nested in a
/// condition step's branches, and the most `DependsOn` edges those steps may declare
/// between them.
///
/// `PipelineDefinition` is modelled `len 1..1048576`, and a megabyte of JSON can declare
/// tens of thousands of steps and edges — the two factors the dependency walk's work is
/// the product of. These are the emulator's OWN bounds on one definition rather than a
/// claim about any account's published quota: each sits far above any pipeline an
/// emulator serves, and together they are what keeps validating a definition short and
/// bounded however large the document is. A definition past either bound is refused
/// before any graph work is done.
const PIPELINE_MAX_STEPS: usize = 512;
/// See [`PIPELINE_MAX_STEPS`].
const PIPELINE_MAX_STEP_DEPENDENCIES: usize = 4_096;

/// [`PIPELINE_MAX_STEPS`] and [`PIPELINE_MAX_STEP_DEPENDENCIES`], so a test can pin the
/// bound at the value this module enforces rather than repeating a literal.
pub const fn pipeline_definition_bounds() -> (usize, usize) {
    (PIPELINE_MAX_STEPS, PIPELINE_MAX_STEP_DEPENDENCIES)
}

/// The most records each of this service's paginated listings returns in one page,
/// including when `MaxResults` is absent or larger.
///
/// `MaxResults` is modelled `range 1..100` on these operations — `ListModels` documents
/// no default and `ListLineageGroups` documents 10 — so this is the service's own
/// server-side ceiling on top of whichever default applies: one call can never serialise
/// the whole store, and a caller that wants the rest follows `NextToken`. Matches the
/// ceiling the other paginated services in this workspace apply.
pub const MAX_PAGE_SIZE: i32 = 50;

/// `SortBy`'s documented default on this service's listings ("Sorts the list of results.
/// The default is `CreationTime`." — the member doc on both `ListModels` and
/// `ListEndpoints`).
pub const DEFAULT_SORT_BY: &str = "CreationTime";

/// `SortOrder`'s documented default ("The sort order for results. The default is
/// `Descending`.").
pub const DEFAULT_SORT_ORDER: &str = ORDER_DESCENDING;

/// `OrderKey`'s descending member, named once here so no handler decides a sort
/// direction by comparing a bare string literal, nor by assuming the documented default
/// is the descending one.
pub const ORDER_DESCENDING: &str = "Descending";

/// `OrderKey`'s ascending member — the documented default of the listings whose own
/// member doc says so (`ListClusters`, `ListCodeRepositories`), as opposed to
/// [`DEFAULT_SORT_ORDER`].
pub const ORDER_ASCENDING: &str = "Ascending";

/// A page boundary as an opaque `NextToken`: the hex of the resource name the page
/// stopped at.
///
/// The token names the item the page stopped at rather than an offset, so resuming lands
/// exactly after it even though the token is opaque to the caller. Stated once here so
/// every paginated listing in this service mints and reads one token format.
pub fn encode_page_token(resource_name: &str) -> String {
    hex::encode(resource_name.as_bytes())
}

/// The resource name a `NextToken` names, or `None` when the token was not minted by
/// [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// What a listing refuses a pagination token it cannot resolve against the current
/// result set with. Stated once, beside the token vocabulary above, so every paginated
/// listing of this service refuses alike.
pub const STALE_PAGE_TOKEN: &str =
    "NextToken does not name a page boundary of this result set; restart the listing \
     without it.";

/// `MlflowAppStatus`'s members this emulator moves an MLflow app through (the modelled
/// enum also carries the update, delete and failure states, which creating one does not
/// reach). Named once here so no handler compares a bare string literal.
pub const MLFLOW_APP_CREATING: &str = "Creating";
/// See [`MLFLOW_APP_CREATING`].
pub const MLFLOW_APP_CREATED: &str = "Created";

/// `ModelRegistrationMode`'s member `CreateMlflowApp` stores when the request names
/// none: the operation's model doc states "If not specified, AutomaticModelRegistration
/// defaults to AutoModelRegistrationDisabled".
pub const AUTO_MODEL_REGISTRATION_DISABLED: &str = "AutoModelRegistrationDisabled";

/// `EndpointStatus`'s and `ClusterStatus`'s members this emulator moves a resource
/// through (both modelled enums also carry the update, rollback and delete states,
/// which this slice does not reach). `InferenceComponentStatus` spells the same two
/// members, so an inference component moves through these too. Named once here so no
/// handler compares a bare
/// string literal.
pub const STATUS_CREATING: &str = "Creating";
/// See [`STATUS_CREATING`].
pub const STATUS_IN_SERVICE: &str = "InService";
/// `ClusterStatus`'s member a cluster carries between the call that asked for its
/// deletion and the instant the deletion completes. See [`Cluster::begin_deletion`].
pub const STATUS_DELETING: &str = "Deleting";

/// How long after the call that starts it the clock owes a transient -> ready
/// transition for the resources in this slice: a scheduler resource's `Creating` ->
/// `Created`, a cluster node's `Pending` -> `Running`, a volume attachment's
/// `attaching` -> `attached`, and an export job's `InProgress` -> terminal.
///
/// Real SageMaker takes minutes; the span is compressed and never zero, so a caller
/// acting straight after the call still observes the transient state. The due instant is
/// stored and never slept on.
///
/// The span itself is stated ONCE for this service, by [`ENDPOINT_IN_SERVICE_AFTER`];
/// this is that one value under the name the resources above are settled by, so the two
/// cannot drift apart.
pub const TRANSIENT_STATE_SETTLES_AFTER: Duration = ENDPOINT_IN_SERVICE_AFTER;

/// `ClusterInstanceStatus`'s two members this emulator moves an added node through.
pub const NODE_STATUS_PENDING: &str = "Pending";
/// See [`NODE_STATUS_PENDING`].
pub const NODE_STATUS_RUNNING: &str = "Running";
/// `ClusterInstanceStatus`'s member a node carries while the instance it runs on is
/// being terminated, which is what a node replacement starts with. See
/// [`ClusterNode::begin_replacement`].
pub const NODE_STATUS_SHUTTING_DOWN: &str = "ShuttingDown";

/// `VolumeAttachmentStatus`'s two members this emulator moves an attachment through.
pub const VOLUME_STATUS_ATTACHING: &str = "attaching";
/// See [`VOLUME_STATUS_ATTACHING`].
pub const VOLUME_STATUS_ATTACHED: &str = "attached";

/// The device names this service hands successive volumes attached to one node.
///
/// AWS's EBS device-naming guidance recommends `/dev/sdf` through `/dev/sdp` for data
/// volumes on a Linux instance; every entry also satisfies the modelled
/// `VolumeDeviceName` pattern, whose `sd[b-z]` branch covers them.
pub const VOLUME_DEVICE_NAMES: [&str; 11] = [
    "/dev/sdf", "/dev/sdg", "/dev/sdh", "/dev/sdi", "/dev/sdj", "/dev/sdk", "/dev/sdl",
    "/dev/sdm", "/dev/sdn", "/dev/sdo", "/dev/sdp",
];

/// What every ARN this service mints, and every ARN-shaped member it accepts, begins
/// with — so a name-or-ARN member is told apart from a bare name once, here.
pub const ARN_PREFIX: &str = "arn:";

/// `GetScalingConfigurationRecommendation`'s documented default for
/// `TargetCpuUtilizationPerCore`: "The percentage of how much utilization you want an
/// instance to use before autoscaling. The default value is 50%."
pub const DEFAULT_TARGET_CPU_UTILIZATION_PER_CORE: i32 = 50;

/// The cooldowns the recommended dynamic scaling configuration carries, in seconds.
///
/// The emulator's OWN value rather than a claim about any published AWS default: the
/// model documents none, and a recommendation has to carry some cooldown for a caller to
/// apply. Equal for scale-in and scale-out, so neither direction is favoured.
pub const RECOMMENDED_SCALING_COOLDOWN_SECONDS: i32 = 300;

/// The target-tracking metric a SageMaker endpoint variant scales on, as Application
/// Auto Scaling names its predefined metric for SageMaker.
pub const PREDEFINED_SCALING_METRIC: &str = "SageMakerVariantInvocationsPerInstance";

/// A fresh 12-character lowercase-hex resource id: the form SageMaker's
/// `compute-quota/` and `cluster-scheduler-config/` ARNs carry (`^[a-z0-9]{12}$`), and
/// the one [`mint_cluster_id`] gives a cluster.
///
/// The digits come from this module's ONE id scheme, [`mint_opaque_resource_id`], so
/// every server-generated id in this crate is minted the same way and a new one needs no
/// second scheme: take as many of its digits as the model's pattern admits.
pub fn mint_short_id() -> String {
    mint_opaque_resource_id().chars().take(12).collect()
}

/// A fresh EC2 instance id for a node that has reached `Running`: `i-` and 17 hex
/// characters, the long form of the modelled `ClusterNodeId` pattern. Same scheme as
/// [`mint_short_id`], cut to the length this pattern admits.
pub fn mint_instance_id() -> String {
    let digits: String = mint_opaque_resource_id().chars().take(17).collect();
    format!("i-{digits}")
}

/// A fresh `NodeLogicalId`: the id `BatchAddClusterNodes` answers for a node it added,
/// before the node has an instance id. The 32 hex digits [`mint_short_id`]'s scheme
/// mints, which the modelled `ClusterNodeLogicalId` pattern
/// (`^[a-zA-Z0-9][a-zA-Z0-9\-]*[a-zA-Z0-9]$`, len 1..128) accepts.
pub fn mint_node_logical_id() -> String {
    mint_opaque_resource_id()
}

/// A fresh opaque token for a presigned-URL grant, from the same scheme as every other
/// id here. It is an identifier inside this emulator's own store, never a credential:
/// nothing signs with it and nothing presents it back to the emulator.
///
/// NOT for a presigned URL whose token is handed to the caller as a SECRET: such a token
/// is minted through the emulator's credential store, because a service may not construct
/// a secret itself (`CreatePresignedMlflowAppUrl` does that). This one mints the id of a
/// grant this emulator stores and looks up again.
pub fn mint_grant_token() -> String {
    mint_opaque_resource_id()
}

/// `TransformJobStatus`'s members this emulator moves a transform job through (the
/// modelled enum also carries `Completed`, `Stopping` and `Stopped`, which this slice
/// does not reach). Named once here so no handler compares a bare string literal.
pub const TRANSFORM_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`TRANSFORM_STATUS_IN_PROGRESS`].
pub const TRANSFORM_STATUS_FAILED: &str = "Failed";

/// How long after `CreateTransformJob` the clock owes the job's terminal status.
///
/// A caller that describes the job straight after creating it still observes
/// `InProgress`; one that waits (as `aws sagemaker wait transform-job-completed-or-stopped`
/// does) observes the terminal status. Real SageMaker takes minutes to provision the ML
/// instances; the span is compressed, never zero.
pub const TRANSFORM_JOB_SETTLES_AFTER: Duration = Duration::from_millis(300);

/// Why a batch transform job this emulator accepted ends in the modelled `Failed`
/// status.
///
/// A batch transform runs the model's inference container against every record of the
/// input dataset; this emulator runs no container, so no inference is performed and no
/// transformed object is written. Reporting `Completed` — with an `S3OutputPath` nothing
/// wrote — would claim work that never ran, so the job ends in the status the model
/// carries for a job that did not produce its output, with this reason saying why.
pub const TRANSFORM_NOT_RUN_FAILURE: &str =
    "This emulator runs no model inference container, so no records were transformed and \
     no output object was written to the job's S3OutputPath.";

/// `MaxPayloadInMB`'s documented ceiling: "The value of MaxPayloadInMB cannot be greater
/// than 100 MB" and "the value of (MaxConcurrentTransforms * MaxPayloadInMB) also cannot
/// exceed 100 MB" (`CreateTransformJob$MaxPayloadInMB`'s model doc). The model constrains
/// the member only from below (`range 0..`), so the ceiling is enforced in the handler.
pub const MAX_PAYLOAD_IN_MB_CEILING: i32 = 100;

/// `MaxPayloadInMB` when the request leaves it unset: "The default value is 6 MB"
/// (`CreateTransformJob$MaxPayloadInMB`'s model doc).
pub const DEFAULT_MAX_PAYLOAD_IN_MB: i32 = 6;

/// `MaxConcurrentTransforms` when the request leaves it unset or zero: "If
/// MaxConcurrentTransforms is set to 0 or left unset, Amazon SageMaker checks the optional
/// execution-parameters to determine the settings for your chosen algorithm. If the
/// execution-parameters endpoint is not enabled, the default value is 1"
/// (`CreateTransformJob$MaxConcurrentTransforms`'s model doc). This emulator serves no
/// execution-parameters endpoint, so the documented fallback of 1 is what it resolves.
pub const DEFAULT_MAX_CONCURRENT_TRANSFORMS: i32 = 1;

/// `CompressionType` and `SplitType` when the request leaves them unset: "The default
/// value is `None`" (`TransformInput$CompressionType`) and "The default value for
/// SplitType is `None`" (`TransformInput$SplitType`).
pub const DEFAULT_NONE: &str = "None";

/// How long after `CreateModelPackage` the clock owes the `Pending` -> `Completed`
/// transition of `ModelPackageStatus`.
///
/// `CreateModelPackage` answers with an ARN while registration (and any validation the
/// request asked for) is still outstanding, so a caller that describes the package
/// straight away still observes `Pending`, and one that waits observes `Completed` —
/// which is what lets a waiter terminate. Real SageMaker takes anywhere from seconds to
/// minutes depending on the validation profiles; the span is compressed, never zero.
pub const MODEL_PACKAGE_COMPLETED_AFTER: Duration = Duration::from_millis(300);

/// The `ModelPackageStatus` members this emulator moves a model package through (the
/// modelled enum also carries `InProgress`, `Failed` and `Deleting`, which this slice
/// does not reach). Named once here so no handler compares a bare string literal, and
/// separately from [`STATUS_COMPLETED`], which is a `ModelPackageGroupStatus` member.
pub const MODEL_PACKAGE_STATUS_PENDING: &str = "Pending";
/// See [`MODEL_PACKAGE_STATUS_PENDING`].
pub const MODEL_PACKAGE_STATUS_COMPLETED: &str = "Completed";

/// The `DetailedModelPackageStatus` members a per-profile validation item reports while
/// the package is `Pending` and once it is `Completed`.
pub const DETAILED_MODEL_PACKAGE_STATUS_NOT_STARTED: &str = "NotStarted";
/// See [`DETAILED_MODEL_PACKAGE_STATUS_NOT_STARTED`].
pub const DETAILED_MODEL_PACKAGE_STATUS_COMPLETED: &str = "Completed";

/// The `ModelApprovalStatus` a registered model version carries when the request named
/// none: SageMaker's model registry documents a new version as awaiting manual approval,
/// and `CreateModelPackage` documents the member as optional for versioned models.
pub const MODEL_APPROVAL_PENDING_MANUAL: &str = "PendingManualApproval";

/// The `ModelPackageRegistrationType` a package `CreateModelPackage` put in the registry
/// carries: it was REGISTERED. The enum's other member, `Logged`, is what a run logs.
pub const MODEL_PACKAGE_REGISTRATION_TYPE_REGISTERED: &str = "Registered";

/// How many `UpdateModelPackage` idempotency tokens one model package remembers.
///
/// `ClientToken` "guarantees that the call to this API is idempotent", so a retry must
/// replay rather than append a second copy of the additional inference specifications it
/// carried. Keeping the tokens on the record bounds them by the record's own lifetime,
/// and this cap bounds how many one record holds, so a caller updating a package in a
/// loop cannot grow the store without end. It is the emulator's own bound, not an AWS
/// one: real SageMaker's deduplication window is not published.
pub const MODEL_PACKAGE_UPDATE_TOKEN_HISTORY: usize = 64;

/// The largest number of additional inference specifications one model package may hold:
/// "Total number of additional Inference Specifications can not exceed 15"
/// (`UpdateModelPackageInput$AdditionalInferenceSpecificationsToAdd`'s model doc). The
/// member's own modelled length bounds ONE request's list, so the running total is
/// enforced in the handler.
pub const ADDITIONAL_INFERENCE_SPECIFICATIONS_MAX: usize = 15;

/// The largest number of customer metadata properties one model package may hold: the
/// modelled `length` of `CustomerMetadataMap` (`min 1, max 50`), which bounds the map a
/// request carries and therefore the merged map a later read replays.
pub const CUSTOMER_METADATA_PROPERTIES_MAX: usize = 50;

/// `AppStatus`'s members this emulator moves an app through. The modelled enum
/// (`smithy_query(query="shape", shape="AppStatus")`) also carries `Deleting`,
/// `Deleted` and `Failed`, which this slice does not reach: `DeleteApp` releases the
/// record, and nothing here fails a launch.
pub const APP_STATUS_PENDING: &str = "Pending";
/// See [`APP_STATUS_PENDING`].
pub const APP_STATUS_IN_SERVICE: &str = "InService";

/// The `Completed` status this service's resources report, shared by the two modelled
/// enums that name it.
///
/// It is the `ModelPackageGroupStatus` a model package group carries once
/// `CreateModelPackageGroup` has returned: a model group is a registry container, not a
/// provisioned resource — the call that creates it is synchronous, and the group is
/// immediately usable (a model package can be registered into it on the very next call).
/// So it is born in this TERMINAL status and the clock owes it no later transition — the
/// modelled enum's `Pending`/`InProgress` describe a group whose creation is still in
/// flight, which never happens here.
pub const STATUS_COMPLETED: &str = "Completed";

// The algorithm slice lives in its own module file, re-exported so handlers keep naming
// `crate::state::...`: see `state/algorithms.rs`.
pub mod algorithms;
pub use algorithms::*;

// ---------------------------------------------------------------------------
// The resource-tagging slice: the ONE `ResourceArn` -> tag-list resolution that
// `AddTags`, `ListTags` and `DeleteTags` share. A tag belongs to the resource it was
// attached to, so it lives on that resource's record (every taggable record carries a
// `tags` field its `Create*` filled from the request's `Tags`) rather than in a second
// map beside the stores — otherwise a resource would have two tag sets and the
// `Create*` one would be invisible to `ListTags`.
//
// Kept here, beside the hub slice below rather than at either end of the module, for the
// same reason that one is: this slice belongs to no single resource kind, so it must not
// sit in the region every new resource slice is appended to.
// ---------------------------------------------------------------------------

/// The most tags one SageMaker resource holds.
///
/// `TagList` is `len 0..50` wherever this service models it — on `AddTags$Tags`,
/// `ListTags$Tags` and every `Create*$Tags` (`smithy_query shape=TagList`) — so a
/// resource carrying more than 50 could not be reported at all. A write that would take
/// a resource past it is refused instead.
pub const TAGS_PER_RESOURCE_QUOTA: usize = 50;

/// The entity label [`does_not_exist`] names a tag operation's `ResourceArn` by.
///
/// `AddTags`, `ListTags` and `DeleteTags` address ANY SageMaker resource kind by ARN —
/// `ResourceArn`'s pattern constrains only the partition, service, Region and account —
/// so the refusal for an ARN this account holds no resource under names the generic
/// entity and the ARN the request sent, which is the only identifier the caller supplied.
pub const TAGGED_RESOURCE_ENTITY: &str = "Resource";

/// Generates the two tag accessors over the list of stores whose records carry BOTH an
/// `arn` and a `tags` list, so the reader (`ListTags`) and the writers (`AddTags`,
/// `DeleteTags`) resolve one ARN through exactly the same stores and cannot disagree
/// about which resource kinds this service tags.
///
/// The list below is EVERY store this service holds whose record carries both, and the
/// invariant it keeps is checkable with one grep: a record with a `tags: Vec<Tag>` field
/// and an `arn` that is not in the list would have its tags unreachable, and a tag call
/// naming it falsely refused. A store that is a slice of its own rather than a plain
/// `BTreeMap` field joins the list by answering `values`/`values_mut` the way
/// [`cluster_policy::Store`] does, so this expansion stays one shape.
///
/// Every store left out is left out for ONE reason: its record holds no `tags: Vec<Tag>`
/// field at all, so there is no tag set in it to reach. Named here in full so the
/// omission is a statement rather than an oversight — a tag call naming one of these
/// resources is refused with [`does_not_exist`], which is the refusal for an ARN this
/// resolution holds no TAG SET for:
///
/// * `lineage_groups` — materialised by [`ComAmazonawsSagemakerSagemakerState::ensure_lineage_group`]
///   rather than created by a request, and SageMaker models no `Tags` anywhere on a
///   lineage group.
/// * `pipeline_executions` — `StartPipelineExecution` models no `Tags`.
/// * `model_card_export_jobs` — `CreateModelCardExportJob` models no `Tags` either
///   (smithy_query operation=CreateModelCardExportJob), so an export job never carried
///   a tag set.
/// * `image_versions` — the tags belong to the image, which is in the list.
/// * `associations` — an edge between two entities, not a taggable resource.
///
/// Each becomes taggable by growing a `tags` field its writer fills and being added to
/// this one list.
macro_rules! tag_accessors {
    ($($store:ident),+ $(,)?) => {
        impl ComAmazonawsSagemakerSagemakerState {
            /// The tags of the resource `arn` names, or `None` when no store holds a
            /// record with that ARN.
            ///
            /// A pure read: `ListTags` reports this and writes nothing.
            pub fn tags_of(&self, arn: &str) -> Option<&[Tag]> {
                let mut found: Option<&[Tag]> = None;
                $(
                    if found.is_none() {
                        found = self
                            .$store
                            .values()
                            .find(|record| record.arn == arn)
                            .map(|record| record.tags.as_slice());
                    }
                )+
                found
            }

            /// The tag list of the resource `arn` names, to add to or remove from, or
            /// `None` when no store holds a record with that ARN.
            pub fn tags_mut(&mut self, arn: &str) -> Option<&mut Vec<Tag>> {
                let mut found: Option<&mut Vec<Tag>> = None;
                $(
                    if found.is_none() {
                        found = self
                            .$store
                            .values_mut()
                            .find(|record| record.arn == arn)
                            .map(|record| &mut record.tags);
                    }
                )+
                found
            }
        }
    };
}

tag_accessors!(
    actions,
    ai_benchmark_jobs,
    ai_workload_configs,
    algorithms,
    app_image_configs,
    apps,
    artifacts,
    auto_ml_jobs_v2,
    clusters,
    cluster_policies,
    code_repositories,
    compute_quotas,
    contexts,
    endpoint_configs,
    endpoints,
    experiments,
    feature_groups,
    hubs,
    hub_contents,
    images,
    inference_components,
    inference_recommendations_jobs,
    mlflow_apps,
    mlflow_tracking_servers,
    model_cards,
    model_package_groups,
    model_packages,
    models,
    notebook_instance_lifecycle_configs,
    notebook_instances,
    partner_apps,
    pipelines,
    processing_jobs,
    training_jobs,
    transform_jobs,
    trial_components,
    trials,
    tuning_jobs,
);

// ---------------------------------------------------------------------------
// The hub slice: the records `ListHubContents` reports. Kept here, beside the first
// resource record rather than at either end of the module, so it does not sit in the
// region every new resource slice is appended to.
// ---------------------------------------------------------------------------

/// The identity of ONE stored hub content version.
///
/// A hub content is identified by its hub, its `HubContentType` and its
/// `HubContentName` — the three members every hub-content operation requires — and
/// versioned by `HubContentVersion`. The version is held BOTH as its three parsed dotted
/// components (see [`version_key`]), so the map orders `1.10.0` AFTER `1.9.0`, and
/// verbatim, so two versions whose components parse the same are still two records rather
/// than one overwriting the other.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HubContentKey {
    pub hub_name: String,
    pub hub_content_type: String,
    pub hub_content_name: String,
    pub version: (u32, u32, u32),
    /// `HubContentVersion` verbatim — the tie-break that keeps the identity total.
    pub version_text: String,
}

/// One version of a hub content, as `ListHubContents` summarises it.
///
/// The members are `HubContentInfo`'s (the summary shape `ListHubContents` returns), plus
/// what the writer of a reference stores beside them; `ImportHubContent` and
/// `DescribeHubContent` (each its own open issue) extend this record with the document,
/// markdown and dependencies they store.
#[derive(Debug, Clone)]
pub struct HubContent {
    pub name: String,
    pub arn: String,
    /// A `HubContentType` member: `Model`, `Notebook`, `ModelReference`, `DataSet` or
    /// `JsonDoc`.
    pub content_type: String,
    /// `HubContentVersion`, verbatim as stored, e.g. `1.0.0`.
    pub version: String,
    pub document_schema_version: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// A `HubContentSupportStatus` member: `Supported`, `Deprecated` or `Restricted`.
    pub support_status: Option<String>,
    pub search_keywords: Option<Vec<String>>,
    /// A `HubContentStatus` member: `Available`, `Importing`, `Deleting`,
    /// `ImportFailed`, `DeleteFailed`, `PendingImport` or `PendingDelete`.
    pub status: String,
    /// When THIS version was created.
    pub creation_time: SystemTime,
    /// When the content was first created, before any later version replaced it; absent
    /// on a content that was never revised.
    pub original_creation_time: Option<SystemTime>,
    /// The public hub content this one references, for a `ModelReference`.
    pub sage_maker_public_hub_content_arn: Option<String>,
    /// The lowest version of the referenced public content this reference accepts
    /// (`ReferenceMinVersion`), as the request that created the reference named it, and
    /// absent on a content that references nothing.
    pub reference_min_version: Option<String>,
    /// The tags attached to this content version — `CreateHubContentReference$Tags` as
    /// sent, then whatever the tag operations added or removed. See [`tag_accessors`].
    pub tags: Vec<Tag>,
}

/// A dotted `<major>.<minor>.<patch>` version (`HubContentVersion`,
/// `DocumentSchemaVersion`, both `^\d{1,4}.\d{1,4}.\d{1,4}$`) as a comparable key, so
/// `1.10.0` orders ABOVE `1.9.0` rather than below it as the strings would.
///
/// A component that is not a number counts as 0: the model refuses a malformed version
/// before a handler sees it, so this only keeps the ordering total.
pub fn version_key(version: &str) -> (u32, u32, u32) {
    let mut components = version.split('.').map(|part| part.parse::<u32>().unwrap_or(0));
    (
        components.next().unwrap_or(0),
        components.next().unwrap_or(0),
        components.next().unwrap_or(0),
    )
}

/// A model created by `CreateModel`, as `DescribeModel` and `ListModels` replay it.
#[derive(Debug, Clone)]
pub struct Model {
    pub name: String,
    pub arn: String,
    pub primary_container: Option<ContainerDefinition>,
    pub containers: Option<Vec<ContainerDefinition>>,
    pub inference_execution_config: Option<InferenceExecutionConfig>,
    pub execution_role_arn: Option<String>,
    pub vpc_config: Option<VpcConfig>,
    pub enable_network_isolation: Option<bool>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
}

/// How long after `CreateHub` the clock owes the hub's `Creating` -> `InService`
/// transition. `HubStatus` models `Creating` as well as `InService`, so a hub is not
/// born ready: a caller that describes it straight after creating it still observes
/// `Creating`, and one that waits observes `InService`. The span is the emulator's own
/// compressed one, never zero.
pub const HUB_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// The most hubs the store holds, after which `CreateHub` is refused with the modelled
/// `ResourceLimitExceeded`. The emulator's OWN bound, for the reason given on
/// [`MODEL_QUOTA`]: every record it holds is released by `DeleteHub`.
pub const HUB_QUOTA: usize = 1_000;

/// A hub created by `CreateHub`, as `DescribeHub` replays it, `ListHubContents`
/// resolves it and `DeleteHub` releases it.
///
/// `status` is a `HubStatus` member (the model's enum: `InService`, `Creating`,
/// `Updating`, `Deleting`, `CreateFailed`, `UpdateFailed`, `DeleteFailed`).
///
/// `in_service_at` is the instant the clock owes the `Creating` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
#[derive(Debug, Clone)]
pub struct Hub {
    pub name: String,
    pub arn: String,
    pub description: String,
    pub display_name: Option<String>,
    pub search_keywords: Option<Vec<String>>,
    pub s3_storage_config: Option<crate::ops::types::HubS3StorageConfig>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub in_service_at: SystemTime,
}

impl Hub {
    /// Materialise the `Creating` -> `InService` transition the clock already owes; see
    /// [`settle_creating`].
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        settle_creating(
            &mut self.status,
            &mut self.last_modified_time,
            self.in_service_at,
            now,
        )
    }
}

/// How long after `CreateTrainingJob` the clock owes the job's move out of `InProgress`
/// into its terminal status.
///
/// Same contract as [`PROCESSING_JOB_ENDS_AFTER`], and chosen on the same terms (AWS
/// documents no duration for a training run): compressed, never zero, so a caller that
/// describes the job straight after creating it still observes `InProgress` with the
/// secondary status of a job whose work has not started, and one that waits (as `aws
/// sagemaker wait training-job-completed-or-stopped` does) observes a terminal status, so
/// a waiter terminates.
pub const TRAINING_JOB_ENDS_AFTER: Duration = Duration::from_millis(400);

/// `TrainingJobStatus`'s members this emulator moves a training job through (the modelled
/// enum also carries `Completed`, `Stopping`, `Stopped` and `Deleting`, which this slice
/// does not reach: `StopTrainingJob` and `DeleteTrainingJob` are still stubs). Named once
/// here so no handler compares a bare string literal.
pub const TRAINING_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`TRAINING_STATUS_IN_PROGRESS`].
pub const TRAINING_STATUS_FAILED: &str = "Failed";

/// The `SecondaryStatus` an accepted training job carries: the job has been accepted and
/// no stage of its work has begun. Every other non-terminal member of that enum
/// (`Downloading`, `Training`, `Uploading`, …) would name work this emulator does not run.
pub const TRAINING_SECONDARY_STATUS_STARTING: &str = "Starting";
/// See [`TRAINING_SECONDARY_STATUS_STARTING`]: the member that goes with
/// [`TRAINING_STATUS_FAILED`] (`SecondaryStatus` spells it exactly as `TrainingJobStatus`
/// does).
pub const TRAINING_SECONDARY_STATUS_FAILED: &str = "Failed";

/// `FailureReason` of a training job whose algorithm image could not be run here.
///
/// A training job runs a container image (`AlgorithmSpecification$TrainingImage`, or the
/// image an algorithm's `TrainingSpecification` names). This emulator has no container
/// runtime, so the run the caller asked for ends in the modelled `Failed` state with this
/// reason rather than reporting a `Completed` nothing earned, a training duration nothing
/// spent or final metrics nothing measured. Within the modelled `FailureReason` length
/// (0..1024). Everything around the run — accepting the job, validating it, minting its
/// ARN, replaying it, reporting its terminal status — is real.
pub const NO_TRAINING_CONTAINER_RUNTIME_REASON: &str =
    "This emulator has no container runtime, so the training job's algorithm image was \
     never started and no model artifacts were written to the output S3 location";

/// A training job created by `CreateTrainingJob`, as `DescribeTrainingJob` replays it and
/// `ListTrainingJobs` summarises it.
///
/// One record per `TrainingJobName` (the name AWS keys a training job by, unique within an
/// account and Region). The members a `TrainingJobSummary` projects are fields of their
/// own, because the listing sorts and filters on them; everything else the describe
/// reports is the accepted request, kept verbatim.
///
/// The store key is the name AS SENT, not folded through [`entity_key`]: `TrainingJobName`
/// is documented unique "within an Amazon Web Services Region in an Amazon Web Services
/// account" and nothing in the model makes it case-insensitive, where
/// `HyperParameterTuningJobName`'s own doc says in so many words that it "is not case
/// sensitive" — which is why that family folds its key and this one does not.
///
/// `ends_at` is the instant the clock owes the job's move out of `InProgress`; it is
/// stored, never slept on, and materialised by whichever reporting call first runs at or
/// after it ([`TrainingJob::settle_status`]).
#[derive(Debug, Clone)]
pub struct TrainingJob {
    pub name: String,
    pub arn: String,
    /// A `TrainingJobStatus` member (`InProgress` | `Completed` | `Failed` | `Stopping` |
    /// `Stopped` | `Deleting`), as stored — never derived from the read that reports it.
    pub status: String,
    /// A `SecondaryStatus` member, when the job has one.
    pub secondary_status: Option<String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// Set only once the job reached a terminal status, per `TrainingJobSummary`'s
    /// `TrainingEndTime` doc ("set only if the training job has one of the terminal
    /// statuses").
    pub training_end_time: Option<SystemTime>,
    /// The warm pool associated with the job, when it has one.
    pub warm_pool_status: Option<crate::ops::types::WarmPoolStatus>,
    /// The training plan the job reserves capacity from, when it has one.
    pub training_plan_arn: Option<String>,
    /// Why the job is in a terminal failure status, once it reached one.
    pub failure_reason: Option<String>,
    /// When the clock owes this job its move out of `InProgress`.
    pub ends_at: SystemTime,
    /// The hyperparameter tuning job that launched this training job, when one did
    /// (`DescribeTrainingJob$TuningJobArn`). A training job a caller creates has no
    /// parent, and nothing here launches one on a tuning job's behalf — which is why
    /// [`ComAmazonawsSagemakerSagemakerState::parallel_training_jobs_in_use`] counts none.
    pub tuning_job_arn: Option<String>,
    /// The `CreateTrainingJob` request the job was accepted with, kept verbatim so
    /// `DescribeTrainingJob` replays what was stored rather than a reconstruction of it.
    pub request: crate::ops::types::CreateTrainingJobInput,
    /// The request's `Tags`, projected out of [`Self::request`] so the one tag resolution
    /// (`tag_accessors!`) reaches this record's tag set through the `tags` field every
    /// other taggable record exposes.
    pub tags: Vec<Tag>,
}

impl TrainingJob {
    /// Materialise the move out of `InProgress` the clock already owes this job.
    ///
    /// This is the one write a reporting operation may make: the transition was due at
    /// `ends_at` whether or not anybody described the job, so the observer makes no
    /// difference to the result. The terminal status is [`TRAINING_STATUS_FAILED`] with
    /// [`NO_TRAINING_CONTAINER_RUNTIME_REASON`], for the reason that constant gives.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == TRAINING_STATUS_IN_PROGRESS && now >= self.ends_at {
            self.status = TRAINING_STATUS_FAILED.to_string();
            self.secondary_status = Some(TRAINING_SECONDARY_STATUS_FAILED.to_string());
            self.failure_reason = Some(NO_TRAINING_CONTAINER_RUNTIME_REASON.to_string());
            self.last_modified_time = self.ends_at;
            self.training_end_time = Some(self.ends_at);
            return true;
        }
        false
    }

    /// Where the job's model artifacts are written, from the output location the request
    /// named and the job's own name.
    ///
    /// `ModelArtifacts$S3ModelArtifacts` is "the path of the S3 object that contains the
    /// model artifacts", and SageMaker writes it under `<S3OutputPath>/<TrainingJobName>`;
    /// it is DERIVED from the request rather than a report that an object exists, and
    /// stays the same across reads for as long as the job is held.
    pub fn model_artifacts_path(&self) -> String {
        let base = self
            .request
            .output_data_config
            .s3_output_path
            .trim_end_matches('/');
        format!("{base}/{}/output/model.tar.gz", self.name)
    }

    /// "Profiling status of a training job" (`ProfilingStatus`), which follows the
    /// profiler configuration the job was accepted with: a job is profiled only where a
    /// `ProfilerConfig` asked for it and did not switch it off, so a job created with no
    /// profiler configuration at all — which is then reported with none — is not being
    /// profiled.
    pub fn profiling_status(&self) -> String {
        let profiled = self
            .request
            .profiler_config
            .as_ref()
            .is_some_and(|config| !config.disable_profiler.unwrap_or(false));
        if profiled {
            PROFILING_STATUS_ENABLED.to_string()
        } else {
            PROFILING_STATUS_DISABLED.to_string()
        }
    }

    /// "A history of all of the secondary statuses that the training job has transitioned
    /// through" (`SecondaryStatusTransitions`): one entry per secondary status this job
    /// has actually been in, each starting when the job entered it and ending when it
    /// left, built from the instants the record holds rather than from any history kept
    /// beside them.
    ///
    /// A job still in its first secondary status reports that one entry, open-ended; the
    /// terminal one carries the reason the job reports, so the history says the same thing
    /// the job's own `FailureReason` does.
    pub fn secondary_status_transitions(&self) -> Vec<SecondaryStatusTransition> {
        let entered_terminal = self.training_end_time;
        let mut transitions = vec![SecondaryStatusTransition {
            status: TRAINING_SECONDARY_STATUS_STARTING.to_string(),
            start_time: timestamp(self.creation_time),
            end_time: entered_terminal.map(timestamp),
            status_message: None,
        }];
        // The job left `Starting` only for the terminal status the clock owed it, so the
        // second entry exists exactly when that transition has been materialised.
        if let (Some(at), Some(status)) = (entered_terminal, self.secondary_status.as_ref()) {
            transitions.push(SecondaryStatusTransition {
                status: status.clone(),
                start_time: timestamp(at),
                end_time: Some(timestamp(at)),
                status_message: self.failure_reason.clone(),
            });
        }
        transitions
    }
}

/// The two `ProfilingStatus` members: a training job is profiled only where a
/// `ProfilerConfig` asked for it.
pub const PROFILING_STATUS_ENABLED: &str = "Enabled";
/// See [`PROFILING_STATUS_ENABLED`].
pub const PROFILING_STATUS_DISABLED: &str = "Disabled";

/// The hyperparameters a training job KEEPS, out of those its create supplied.
///
/// `HyperParameterValue` is modelled `@length min 0` — an empty value is a legal thing to
/// send — but recorded SageMaker does not keep an entry whose value is the empty string:
/// the described job reports a map without that key, so a key supplied empty is
/// indistinguishable from one never supplied, while every non-empty value round-trips
/// unchanged. A caller that reads the map back to decide whether a knob was configured
/// therefore sees the same key set here as on the service.
///
/// Applied where the job is STORED rather than where one reader reports it, so every
/// reader of the record answers the same key set.
pub fn retained_hyper_parameters(
    supplied: Option<HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    supplied.map(|supplied| {
        supplied
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .collect()
    })
}

/// An endpoint configuration created by `CreateEndpointConfig`, as
/// `DescribeEndpointConfig` replays it and `CreateEndpoint` reads it.
#[derive(Debug, Clone)]
pub struct EndpointConfig {
    pub name: String,
    pub arn: String,
    pub production_variants: Vec<ProductionVariant>,
    pub shadow_production_variants: Option<Vec<ProductionVariant>>,
    pub data_capture_config: Option<DataCaptureConfig>,
    pub async_inference_config: Option<AsyncInferenceConfig>,
    pub explainer_config: Option<ExplainerConfig>,
    pub metrics_config: Option<MetricsConfig>,
    pub kms_key_id: Option<String>,
    pub execution_role_arn: Option<String>,
    pub vpc_config: Option<VpcConfig>,
    pub enable_network_isolation: Option<bool>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
}

/// An endpoint created by `CreateEndpoint`, as `DescribeEndpoint` replays it.
///
/// `in_service_at` is the instant the clock owes the `Creating` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
///
/// `instance_usage` is how many instances of each ML instance type this endpoint holds,
/// snapshotted from the configuration it was created from (like its variant summaries,
/// and for the same reason: a later configuration does not retroactively change what is
/// deployed). It is what the per-instance-type hosting quota counts against
/// ([`ComAmazonawsSagemakerSagemakerState::endpoint_instances_in_use`]), so deleting the
/// endpoint releases exactly what creating it took.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub name: String,
    pub arn: String,
    pub config_name: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub production_variants: Vec<ProductionVariantSummary>,
    pub shadow_production_variants: Vec<ProductionVariantSummary>,
    pub data_capture_config: Option<DataCaptureConfig>,
    pub async_inference_config: Option<AsyncInferenceConfig>,
    pub explainer_config: Option<ExplainerConfig>,
    pub metrics_config: Option<MetricsConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub in_service_at: SystemTime,
    pub instance_usage: BTreeMap<String, i64>,
}

/// The unit the per-instance-type hosting quotas are counted in: what their limit, the
/// account's utilisation of them and a request's delta are numbers OF, as
/// [`account_quota_exceeded`] states them.
pub const INSTANCES_UNIT: &str = "Instances";

/// The published name of the quota that bounds how many instances of ONE ML instance type
/// an account may run behind its endpoints, as a refusal states it.
///
/// Service Quotas names the entry `<instance type> for endpoint usage` (it is listed under
/// that name for this service, beside the notebook-instance and training entries of the
/// same shape), so the instance type is part of the quota's NAME rather than prose around
/// it: a client reads which type it has no room for straight off the message.
pub fn endpoint_usage_quota_name(instance_type: &str) -> String {
    format!("{instance_type} for endpoint usage")
}

/// The instance FAMILIES whose published default endpoint-usage quota is zero instances:
/// the newest accelerated generations (the `p4`/`p5`/`p6` GPU families and the Trainium
/// ones), which an account has to be granted capacity for before it can host anything on
/// them.
///
/// Keyed on the FAMILY — the middle segment of an `ml.<family>.<size>` instance type — so
/// a size AWS adds to one of these families needs no edit here, and so the rule is one
/// about generations rather than a list of every type. Every other family's default is a
/// non-zero number of instances ([`DEFAULT_ENDPOINT_USAGE_QUOTA`]).
const ZERO_DEFAULT_HOSTING_FAMILIES: [&str; 11] = [
    "p4d",
    "p4de",
    "p5",
    "p5e",
    "p5en",
    "p6-b200",
    "p6-b300",
    "p6e-gb200",
    "trn1",
    "trn1n",
    "trn2",
];

/// How many instances of ONE type an account may host, for a type whose published default
/// is not zero.
///
/// This emulator models neither Service Quotas nor an account's adjustments, so — as with
/// [`MODEL_QUOTA`] — this is the emulator's OWN bound rather than a claim about any
/// account's limit for a particular type: far above any deployment an emulator serves, and
/// what stops one request asking for an unbounded number of instances.
pub const DEFAULT_ENDPOINT_USAGE_QUOTA: i64 = 1_000;

/// The account's quota, in instances, for hosting `instance_type` behind an endpoint.
///
/// Zero for a family AWS publishes a zero default for ([`ZERO_DEFAULT_HOSTING_FAMILIES`]),
/// so an endpoint on one is refused until the account is granted capacity;
/// [`DEFAULT_ENDPOINT_USAGE_QUOTA`] for every other type.
pub fn endpoint_usage_quota(instance_type: &str) -> i64 {
    // `ml.<family>.<size>`: the family is the second segment, and a value that is not in
    // that form names no family of the zero-default generations.
    let family = instance_type.split('.').nth(1).unwrap_or_default();
    if ZERO_DEFAULT_HOSTING_FAMILIES.contains(&family) {
        return 0;
    }
    DEFAULT_ENDPOINT_USAGE_QUOTA
}

/// How many instances of each ML instance type `variants` ask for: one entry per type, the
/// instance counts of the variants naming it added up.
///
/// A variant that names an instance type runs at least one instance of it, so a variant
/// with no `InitialInstanceCount` counts as one; a variant that names no single type at all
/// — a serverless one, or one that lists a pool of alternatives for the service to choose
/// from — holds no instances of any one type and contributes nothing.
pub fn instance_demand<'variants>(
    variants: impl IntoIterator<Item = &'variants ProductionVariant>,
) -> BTreeMap<String, i64> {
    let mut demand: BTreeMap<String, i64> = BTreeMap::new();
    for variant in variants {
        let Some(instance_type) = variant.instance_type.as_deref() else {
            continue;
        };
        let count = i64::from(variant.initial_instance_count.unwrap_or(1)).max(1);
        *demand.entry(instance_type.to_string()).or_default() += count;
    }
    demand
}

/// Materialise the `Creating` -> `InService` transition the clock already owes a
/// resource that carries one.
///
/// This is the one write a reporting operation may make: the transition was due at
/// `in_service_at` whether or not anybody called, so the observer makes no difference to
/// the result. `last_modified_time` moves to the instant the transition was due, not to
/// "now", so the value is stable across later reads.
///
/// Answers whether the transition happened on this call, so the caller can record it.
fn settle_creating(
    status: &mut String,
    last_modified_time: &mut SystemTime,
    in_service_at: SystemTime,
    now: SystemTime,
) -> bool {
    if status == STATUS_CREATING && now >= in_service_at {
        *status = STATUS_IN_SERVICE.to_string();
        *last_modified_time = in_service_at;
        return true;
    }
    false
}

impl Endpoint {
    /// Materialise the `Creating` -> `InService` transition the clock already owes; see
    /// [`settle_creating`].
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        settle_creating(
            &mut self.status,
            &mut self.last_modified_time,
            self.in_service_at,
            now,
        )
    }
}

/// How long after `CreateInferenceComponent` the clock owes that component the
/// `Creating` -> `InService` transition, on the same terms as
/// [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the component straight after
/// creating it still observes `Creating` with no copies running, and one that waits
/// observes `InService` with the copies it asked for. Compressed, never zero.
///
/// The span is measured from the later of the create and the instant the hosting
/// endpoint goes `InService`, because a component's copies run on that endpoint's
/// capacity and cannot be placed before it is serving.
pub const INFERENCE_COMPONENT_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// See [`MODEL_QUOTA`]: the same bound on the inference-component store, which
/// `CreateInferenceComponent` answers `ResourceLimitExceeded` past and
/// `DeleteInferenceComponent` releases a slot of.
pub const INFERENCE_COMPONENT_QUOTA: usize = 1_000;

/// An inference component created by `CreateInferenceComponent`, as
/// `DescribeInferenceComponent` replays it.
///
/// The endpoint it is hosted on is snapshotted by name AND by ARN, so the component
/// keeps reporting the endpoint it was deployed to even after that endpoint is deleted.
/// Exactly one of `specification` / `specifications` is ever set: the model doc for
/// `Specifications` says a caller uses "either this parameter or the singular
/// Specification parameter, but not both".
///
/// `in_service_at` is the instant the clock owes the `Creating` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
#[derive(Debug, Clone)]
pub struct InferenceComponent {
    pub name: String,
    pub arn: String,
    pub endpoint_name: String,
    pub endpoint_arn: String,
    pub variant_name: Option<String>,
    pub specification: Option<crate::ops::types::InferenceComponentSpecification>,
    pub specifications: Option<Vec<crate::ops::types::InferenceComponentSpecification>>,
    pub runtime_config: Option<crate::ops::types::InferenceComponentRuntimeConfig>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub in_service_at: SystemTime,
}

impl InferenceComponent {
    /// Materialise the `Creating` -> `InService` transition the clock already owes; see
    /// [`settle_creating`].
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        settle_creating(
            &mut self.status,
            &mut self.last_modified_time,
            self.in_service_at,
            now,
        )
    }

    /// The specifications this component deploys, whether the caller sent the singular
    /// `Specification` or the `Specifications` list: one slice both readers walk.
    ///
    /// A stored record carries at most ONE of the two members —
    /// `CreateInferenceComponent` refuses a request that sends both — so the
    /// both-populated pair is not a case this has to choose between.
    pub fn specifications(&self) -> &[crate::ops::types::InferenceComponentSpecification] {
        match (&self.specification, &self.specifications) {
            (Some(single), None) => ::std::slice::from_ref(single),
            (None, Some(list)) => list.as_slice(),
            _ => &[],
        }
    }
}

/// A hyperparameter tuning job created by `CreateHyperParameterTuningJob`, as
/// `DescribeHyperParameterTuningJob` replays it.
///
/// `settles_at` is the instant the clock owes the `InProgress` -> terminal transition;
/// it is stored, never slept on, and materialised by whichever reporting call first
/// runs at or after it.
#[derive(Debug, Clone)]
pub struct HyperParameterTuningJob {
    pub name: String,
    pub arn: String,
    pub config: HyperParameterTuningJobConfig,
    pub training_job_definition: Option<HyperParameterTrainingJobDefinition>,
    pub training_job_definitions: Option<Vec<HyperParameterTrainingJobDefinition>>,
    pub warm_start_config: Option<HyperParameterTuningJobWarmStartConfig>,
    pub autotune: Option<Autotune>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// `HyperParameterTuningEndTime`: set when the job reaches a terminal status.
    pub end_time: Option<SystemTime>,
    pub settles_at: SystemTime,
}

impl HyperParameterTuningJob {
    /// Materialise the transition the clock already owes: `InProgress` -> `Failed` for a
    /// job left to run, `Stopping` -> `Stopped` for one that was asked to stop.
    ///
    /// The one write a reporting operation may make: the transition was due at
    /// `settles_at` whether or not anybody described the job, so the observer makes no
    /// difference to the result. The end and last-modified times move to the instant the
    /// transition was due rather than to "now", so they are stable across later reads.
    ///
    /// A job left to run fails because this emulator launches no training job — see
    /// [`NO_TRAINING_RUNNER_FAILURE_REASON`]; a job asked to stop carries no failure
    /// reason, because stopping is what the caller asked for.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if now < self.settles_at {
            return false;
        }
        match self.status.as_str() {
            TUNING_STATUS_IN_PROGRESS => {
                self.status = TUNING_STATUS_FAILED.to_string();
                self.failure_reason = Some(NO_TRAINING_RUNNER_FAILURE_REASON.to_string());
            }
            TUNING_STATUS_STOPPING => self.status = TUNING_STATUS_STOPPED.to_string(),
            _ => return false,
        }
        self.end_time = Some(self.settles_at);
        self.last_modified_time = self.settles_at;
        true
    }

    /// Whether the job has reached a status from which nothing more happens on its own.
    ///
    /// `Deleting` and `DeleteFailed` belong to `DeleteHyperParameterTuningJob`, which this
    /// slice does not reach, so the terminal statuses are the three a job here can be in.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            TUNING_STATUS_FAILED | TUNING_STATUS_STOPPED | STATUS_COMPLETED
        )
    }

    /// Record that the caller asked this job to stop: it moves to `Stopping` now, and the
    /// clock owes it `Stopped`. The span is never zero, so the transient status is
    /// observable.
    pub fn request_stop(&mut self, now: SystemTime) {
        self.status = TUNING_STATUS_STOPPING.to_string();
        self.failure_reason = None;
        self.last_modified_time = now;
        self.settles_at = now + TUNING_JOB_STOPS_AFTER;
    }
}

/// A trial created by `CreateTrial`, as `DescribeTrial` replays it.
#[derive(Debug, Clone)]
pub struct Trial {
    pub name: String,
    pub arn: String,
    pub display_name: Option<String>,
    pub experiment_name: String,
    pub metadata_properties: Option<MetadataProperties>,
    pub tags: Vec<Tag>,
    /// Stored at creation rather than rebuilt per read, so every later read reports the
    /// identity that actually created the trial.
    pub created_by: UserContext,
    /// Who last modified the trial: the creating identity until a call modifies it, when
    /// `UpdateTrial` records the identity behind that call.
    pub last_modified_by: UserContext,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// A processing job created by `CreateProcessingJob`, as `DescribeProcessingJob`
/// replays it.
///
/// `ends_at` is the instant the clock owes the move out of the job's current
/// non-terminal status (`InProgress` from the create, `Stopping` from a stop); it is
/// stored, never slept on, and materialised by whichever reporting call first runs at or
/// after it (see [`ProcessingJob::settle_status`]).
#[derive(Debug, Clone)]
pub struct ProcessingJob {
    pub name: String,
    pub arn: String,
    pub role_arn: String,
    pub app_specification: AppSpecification,
    pub processing_inputs: Option<Vec<ProcessingInput>>,
    pub processing_output_config: Option<ProcessingOutputConfig>,
    /// The requested resources with the documented defaults applied and, when the
    /// request named `InstancePreferences`, the instance type the service selected
    /// from them.
    pub processing_resources: ProcessingResources,
    pub stopping_condition: Option<ProcessingStoppingCondition>,
    pub environment: Option<std::collections::HashMap<String, String>>,
    pub network_config: Option<NetworkConfig>,
    pub experiment_config: Option<ExperimentConfig>,
    pub tags: Vec<Tag>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub processing_end_time: Option<SystemTime>,
    pub ends_at: SystemTime,
}

impl ProcessingJob {
    /// Materialise the move out of a non-terminal status the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the move was due at
    /// `ends_at` whether or not anybody described the job, so the observer makes no
    /// difference to the result. `LastModifiedTime` and `ProcessingEndTime` take the
    /// instant the move was due, not "now", so both are stable across later reads.
    ///
    /// A job left to run ends in [`PROCESSING_STATUS_FAILED`] with
    /// [`NO_CONTAINER_RUNTIME_REASON`]: nothing ran the job's container image, so no
    /// `Completed` status, `ProcessingStartTime` or `ExitMessage` is invented for it. A
    /// job the caller asked to stop ends in [`PROCESSING_STATUS_STOPPED`] with no failure
    /// reason instead: stopping is what the caller asked for, not a failure.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if now < self.ends_at {
            return false;
        }
        match self.status.as_str() {
            PROCESSING_STATUS_IN_PROGRESS => {
                self.status = PROCESSING_STATUS_FAILED.to_string();
                self.failure_reason = Some(NO_CONTAINER_RUNTIME_REASON.to_string());
            }
            PROCESSING_STATUS_STOPPING => {
                self.status = PROCESSING_STATUS_STOPPED.to_string();
            }
            // Already terminal: nothing is owed and nothing moves.
            _ => return false,
        }
        // The instant the run ended, which is when the move was due rather than "now", so
        // both are stable across later reads. No start time is recorded beside it: the
        // container never started.
        self.processing_end_time = Some(self.ends_at);
        self.last_modified_time = self.ends_at;
        true
    }

    /// Whether the job has reached a status nothing leaves on its own, i.e. one of
    /// `ProcessingJobStatus`'s terminal members.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            PROCESSING_STATUS_FAILED | PROCESSING_STATUS_STOPPED | STATUS_COMPLETED
        )
    }

    /// Whether a stop the caller already asked for is still in flight, i.e. the job sits
    /// in the transient `Stopping` status.
    pub fn is_stopping(&self) -> bool {
        self.status == PROCESSING_STATUS_STOPPING
    }

    /// Record that the caller asked this job to stop: it moves to the transient
    /// `Stopping` now, and the clock owes it `Stopped` at
    /// [`PROCESSING_JOB_STOPS_AFTER`]. The span is never zero, so the transient status is
    /// observable.
    pub fn request_stop(&mut self, now: SystemTime) {
        self.status = PROCESSING_STATUS_STOPPING.to_string();
        // The run is being abandoned on request, not failing, so nothing is reported
        // beside `Stopping` or the `Stopped` that follows it.
        self.failure_reason = None;
        self.processing_end_time = None;
        self.last_modified_time = now;
        self.ends_at = now + PROCESSING_JOB_STOPS_AFTER;
    }
}

// ---------------------------------------------------------------------------
// Inference Recommender: the recommendation jobs `CreateInferenceRecommendationsJob`
// accepts and `DescribeInferenceRecommendationsJob` replays.
// ---------------------------------------------------------------------------

/// See [`MODEL_QUOTA`]. `DeleteInferenceRecommendationsJob` is the release path AWS
/// documents for a recommendation job; it is still a scaffold stub in this emulator, so
/// today nothing releases a record and this bound is what keeps the store finite.
pub const INFERENCE_RECOMMENDATIONS_JOB_QUOTA: usize = 1_000;

/// The `RecommendationJobStatus` members this emulator moves a recommendation job
/// through (the modelled enum also carries `IN_PROGRESS`, `COMPLETED`, `STOPPING`,
/// `STOPPED`, `DELETING` and `DELETED`, which this slice does not reach). Named once
/// here so no handler compares a bare string literal.
pub const RECOMMENDATION_JOB_STATUS_PENDING: &str = "PENDING";
/// See [`RECOMMENDATION_JOB_STATUS_PENDING`].
pub const RECOMMENDATION_JOB_STATUS_FAILED: &str = "FAILED";

/// How long after `CreateInferenceRecommendationsJob` the clock owes the job's move out
/// of `PENDING` into its terminal status.
///
/// A span chosen by this emulator (AWS documents no duration for a recommendation run),
/// inside the 250–750 ms band the project fixes for a transition the clock owes. It is
/// never zero, so a caller that describes the job straight after creating it still
/// observes `PENDING`, and one that waits observes the terminal status, so a poller
/// terminates.
pub const RECOMMENDATION_JOB_SETTLES_AFTER: Duration = Duration::from_millis(400);

/// `RecommendationJobInputConfig$JobDurationInSeconds`'s documented ceiling:
/// "Specifies the maximum duration of the job, in seconds. The maximum value is 18,000
/// seconds." The model constrains the member only from below (`range 1..`), so the
/// ceiling is enforced in the handler.
pub const MAX_RECOMMENDATION_JOB_DURATION_IN_SECONDS: i32 = 18_000;

/// Why a recommendation job this emulator accepted ends in the modelled `FAILED` status.
///
/// An Inference Recommender job provisions endpoints, replays the job's sample payload
/// against the model's inference container under the requested traffic pattern and
/// reports the latency, throughput and cost it measured. This emulator runs no inference
/// container and generates no load, so nothing is measured: reporting `COMPLETED` with
/// `InferenceRecommendations` or `EndpointPerformances` would be metrics synthesised to
/// look computed, which the engine contract (`vendor/engine/src/lib.rs`) forbids. The run
/// therefore ends in the status the model carries for a job that produced no
/// recommendation, with this reason saying why. Everything around the run — accepting the
/// job, validating it, minting its ARN, replaying it, reporting its terminal status — is
/// real.
pub const NO_INFERENCE_BENCHMARK_REASON: &str =
    "This emulator runs no model inference container and generates no benchmark traffic, so \
     no load test was performed and no inference recommendation was measured.";

/// How every operation that cannot resolve the Inference Recommender job a request names
/// states it: the entity as the service words it, the `JobName` the request carried, and
/// `was not found.`
///
/// A recommendation job is addressed by NAME only
/// (`RecommendationJobName`/`InferenceRecommendationsJobName`), so the identifier is the
/// one the caller sent — unquoted — and a consumer correlates the refusal with its own
/// input. Stated once here so the reader and the recommendation surfaces over a job cannot
/// word the same fact differently.
pub fn inference_recommendations_job_not_found(job_name: &str) -> String {
    format!("Inference recommendations job {job_name} was not found.")
}

/// An Inference Recommender job created by `CreateInferenceRecommendationsJob`, as
/// `DescribeInferenceRecommendationsJob` replays it.
///
/// Every member mirrors what the create request carried, so a describe replays the
/// request rather than a value invented on the read path. `ends_at` is the instant the
/// clock owes the move out of `PENDING`; it is stored, never slept on, and materialised
/// by whichever reporting call first runs at or after it (see
/// [`InferenceRecommendationsJob::settle_status`]).
///
/// `output_config` and `tags` are stored because the create carried them, even though no
/// implemented reader surfaces them yet (`DescribeInferenceRecommendationsJob` reports
/// neither), the way [`CodeRepository`] stores its tags.
///
/// `recommendations` holds what the job MEASURED, which
/// `GetScalingConfigurationRecommendation` computes a scaling configuration from. It is
/// empty for a job this service accepted, because this emulator serves no model inference
/// container and generates no benchmark traffic (see
/// [`NO_INFERENCE_BENCHMARK_REASON`]); nothing synthesises an entry, and a reader with no
/// entry to answer from refuses rather than inventing one.
#[derive(Debug, Clone)]
pub struct InferenceRecommendationsJob {
    pub name: String,
    pub arn: String,
    pub job_type: String,
    pub role_arn: String,
    pub input_config: RecommendationJobInputConfig,
    pub job_description: Option<String>,
    pub stopping_conditions: Option<RecommendationJobStoppingConditions>,
    pub output_config: Option<RecommendationJobOutputConfig>,
    pub tags: Vec<Tag>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub completion_time: Option<SystemTime>,
    pub ends_at: SystemTime,
    pub recommendations: Vec<InferenceRecommendation>,
}

impl InferenceRecommendationsJob {
    /// A job record whose benchmark results this store was GIVEN rather than measured:
    /// the form a test (or a future reader of a seeded corpus) needs so
    /// `GetScalingConfigurationRecommendation` has recommendations to compute from.
    ///
    /// The members a create request would have carried are left at their empty values,
    /// and the job is terminal on arrival (`ends_at` is its `creation_time`), because no
    /// create call accepted it and so no transition is owed for it.
    pub fn seeded(
        name: String,
        arn: String,
        status: String,
        recommendations: Vec<InferenceRecommendation>,
        creation_time: SystemTime,
    ) -> Self {
        Self {
            name,
            arn,
            job_type: String::new(),
            role_arn: String::new(),
            input_config: RecommendationJobInputConfig::default(),
            job_description: None,
            stopping_conditions: None,
            output_config: None,
            tags: Vec::new(),
            status,
            failure_reason: None,
            creation_time,
            last_modified_time: creation_time,
            completion_time: Some(creation_time),
            ends_at: creation_time,
            recommendations,
        }
    }
    /// Materialise the move out of `PENDING` the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the move was due at
    /// `ends_at` whether or not anybody described the job, so the observer makes no
    /// difference to the result. `LastModifiedTime` and `CompletionTime` take the instant
    /// the move was due, not "now", so both are stable across later reads.
    ///
    /// The terminal status is [`RECOMMENDATION_JOB_STATUS_FAILED`] with
    /// [`NO_INFERENCE_BENCHMARK_REASON`]: see that constant for why a job this emulator
    /// accepted cannot end `COMPLETED`.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == RECOMMENDATION_JOB_STATUS_PENDING && now >= self.ends_at {
            self.status = RECOMMENDATION_JOB_STATUS_FAILED.to_string();
            self.failure_reason = Some(NO_INFERENCE_BENCHMARK_REASON.to_string());
            self.completion_time = Some(self.ends_at);
            self.last_modified_time = self.ends_at;
            return true;
        }
        false
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another recommendation job fits, i.e. the store is below
    /// [`INFERENCE_RECOMMENDATIONS_JOB_QUOTA`].
    pub fn inference_recommendations_jobs_below_quota(&self) -> bool {
        self.inference_recommendations_jobs.len() < INFERENCE_RECOMMENDATIONS_JOB_QUOTA
    }
}

/// A pipeline created by `CreatePipeline`, as `DescribePipeline` replays it until
/// `DeletePipeline` removes it.
///
/// A pipeline carries no deletion clock: recorded AWS makes a deleted pipeline
/// unreadable from the delete call on, so `DeletePipeline` drops the record outright
/// and there is no transient state for a later call to materialise.
#[derive(Debug, Clone)]
pub struct Pipeline {
    pub name: String,
    pub arn: String,
    /// The display name the pipeline is shown under. A create that names none leaves the
    /// pipeline displayed under its `PipelineName`, so the member is resolved ONCE where
    /// the record is written and every reader replays the resolved value.
    pub display_name: String,
    pub definition: String,
    pub description: Option<String>,
    pub role_arn: String,
    pub status: String,
    pub parallelism_configuration: Option<ParallelismConfiguration>,
    pub tags: Vec<Tag>,
    /// The identity that created the pipeline, and the identity of the last call that
    /// modified it — recorded when each call happens, as [`caller_user_context`] builds
    /// it, and replayed unchanged.
    pub created_by: UserContext,
    pub last_modified_by: UserContext,
    /// The `ClientRequestToken` the creating call presented, so a retry of that very
    /// call is completed "no more than one time" as the member documents.
    pub client_request_token: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// When an execution of this pipeline last started, which is what
    /// `StartPipelineExecution` stamps; `None` until one has.
    pub last_run_time: Option<SystemTime>,
}

/// One execution of a pipeline, started by `StartPipelineExecution` and replayed by
/// `DescribePipelineExecution`.
///
/// Keyed in the store by its own ARN (`<pipeline arn>/execution/<id>`), the only
/// identifier `DescribePipelineExecution` resolves one by, the way the lineage stores
/// here are keyed by the ARN their create mints.
///
/// `ends_at` is the instant the clock owes the execution's terminal status; it is
/// stored, never slept on, and materialised by [`PipelineExecution::settle_status`].
#[derive(Debug, Clone)]
pub struct PipelineExecution {
    pub arn: String,
    /// The `PipelineName` this execution belongs to — the pipeline store's key, so the
    /// record is found again when the pipeline is deleted or the pipeline's executions
    /// are enumerated.
    pub pipeline_name: String,
    pub pipeline_arn: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub status: String,
    pub failure_reason: Option<String>,
    /// The parameter values the start call supplied, each naming a parameter the stored
    /// definition declares.
    pub parameters: Vec<Parameter>,
    pub parallelism_configuration: Option<ParallelismConfiguration>,
    pub selective_execution_config: Option<SelectiveExecutionConfig>,
    pub pipeline_version_id: i64,
    /// The `MlflowExperimentName` the start call supplied, which is the only member of
    /// the reported `MLflowConfig` this emulator knows: it serves no MLflow tracking
    /// server, so no resource ARN is invented beside it.
    pub mlflow_experiment_name: Option<String>,
    /// The `ClientRequestToken` the starting call presented, so a retry of that very
    /// call completes "no more than one time" as the member documents.
    pub client_request_token: String,
    pub created_by: UserContext,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub ends_at: SystemTime,
}

impl PipelineExecution {
    /// Materialise the move out of `Executing` the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the move was due at
    /// `ends_at` whether or not anybody described the execution, so the observer makes
    /// no difference to the result. `LastModifiedTime` takes the instant the move was
    /// due, not "now", so it is stable across later reads.
    ///
    /// The terminal status is [`PIPELINE_EXECUTION_STATUS_FAILED`] with
    /// [`PIPELINE_EXECUTION_NOT_RUN`]: see that constant for why an execution this
    /// emulator accepted cannot end `Succeeded`.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == PIPELINE_EXECUTION_STATUS_EXECUTING && now >= self.ends_at {
            self.status = PIPELINE_EXECUTION_STATUS_FAILED.to_string();
            self.failure_reason = Some(PIPELINE_EXECUTION_NOT_RUN.to_string());
            self.last_modified_time = self.ends_at;
            return true;
        }
        false
    }
}

/// A batch transform job created by `CreateTransformJob`, as `DescribeTransformJob`
/// replays it and `ListTransformJobs` summarises it.
///
/// `ends_at` is the instant the clock owes the job's terminal status; it is stored,
/// never slept on, and materialised by whichever reporting call first runs at or after
/// it.
///
/// `last_modified_time` starts at the creation instant and moves only when the record
/// does, which is what `ListTransformJobs`' `LastModifiedTime` bounds select on and what
/// its summaries report.
#[derive(Debug, Clone)]
pub struct TransformJob {
    pub name: String,
    pub arn: String,
    pub model_name: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub max_concurrent_transforms: i32,
    pub model_client_config: Option<ModelClientConfig>,
    pub max_payload_in_mb: i32,
    pub batch_strategy: Option<String>,
    pub environment: Option<::std::collections::HashMap<String, String>>,
    pub transform_input: TransformInput,
    pub transform_output: TransformOutput,
    pub data_capture_config: Option<BatchDataCaptureConfig>,
    pub transform_resources: TransformResources,
    pub data_processing: Option<DataProcessing>,
    pub experiment_config: Option<ExperimentConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub transform_end_time: Option<SystemTime>,
    pub ends_at: SystemTime,
}

impl TransformJob {
    /// Materialise the terminal status the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the job was due to end at
    /// `ends_at` whether or not anybody called, so the observer makes no difference to
    /// the result. `TransformEndTime` is the instant the end was due, not "now", so the
    /// value is stable across later reads, and `LastModifiedTime` moves to that same
    /// instant for the same reason.
    ///
    /// The status is [`TRANSFORM_STATUS_FAILED`] with [`TRANSFORM_NOT_RUN_FAILURE`]: see
    /// that constant for why a job this emulator accepted cannot end `Completed`.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == TRANSFORM_STATUS_IN_PROGRESS && now >= self.ends_at {
            self.status = TRANSFORM_STATUS_FAILED.to_string();
            self.failure_reason = Some(TRANSFORM_NOT_RUN_FAILURE.to_string());
            self.transform_end_time = Some(self.ends_at);
            self.last_modified_time = self.ends_at;
            return true;
        }
        false
    }
}

/// An MLflow app created by `CreateMlflowApp`, as `DescribeMlflowApp` replays it.
///
/// `created_at` is the instant the clock owes the `Creating` -> `Created` transition;
/// like an endpoint's, it is stored and materialised by whichever reporting call first
/// runs at or after it.
///
/// Every member mirrors what the create request carried, so a describe replays the
/// request rather than a value invented on the read path. `model_registration_mode` is
/// the one member with a DOCUMENTED default, so it is stored resolved; the others stay
/// `None` when the request named none, because the model documents no default for them.
///
/// `created_by` is stored at creation (through [`caller_user_context`], as
/// `CreateModelPackageGroup` does) rather than rebuilt per read, so every later read
/// reports the identity that actually created the app.
#[derive(Debug, Clone)]
pub struct MlflowApp {
    pub arn: String,
    pub name: String,
    pub artifact_store_uri: String,
    pub role_arn: String,
    pub kms_key_id: Option<String>,
    pub model_registration_mode: String,
    pub weekly_maintenance_window_start: Option<String>,
    pub account_default_status: Option<String>,
    pub default_domain_id_list: Option<Vec<String>>,
    pub tags: Vec<Tag>,
    pub status: String,
    pub created_by: UserContext,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_at: SystemTime,
}

impl MlflowApp {
    /// Materialise the `Creating` -> `Created` transition the clock already owes, the
    /// way [`Endpoint::settle_status`] does: it was due at `created_at` whether or not
    /// anybody described the app, and `LastModifiedTime` moves to the instant it was
    /// due rather than to "now", so the value is stable across later reads.
    ///
    /// Answers whether the transition happened on this call.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == MLFLOW_APP_CREATING && now >= self.created_at {
            self.status = MLFLOW_APP_CREATED.to_string();
            self.last_modified_time = self.created_at;
            return true;
        }
        false
    }
}

/// An AppImageConfig created by `CreateAppImageConfig`, as `DescribeAppImageConfig`
/// replays it.
///
/// AWS returns an AppImageConfig ready the moment it is created — the resource carries
/// no status member at all (`smithy_query` shape=AppImageConfigDetails) — so there is no
/// clock-owed transition to settle here, only the two timestamps.
#[derive(Debug, Clone)]
pub struct AppImageConfig {
    pub name: String,
    pub arn: String,
    pub kernel_gateway_image_config: Option<KernelGatewayImageConfig>,
    pub jupyter_lab_app_image_config: Option<JupyterLabAppImageConfig>,
    pub code_editor_app_image_config: Option<CodeEditorAppImageConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// The `<kind>` component [`resource_arn`] takes for an AppImageConfig, as the modelled
/// `AppImageConfigArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:app-image-config/`). Named once so
/// the create, the readers and the update mint and match the same ARN.
pub const APP_IMAGE_CONFIG_ARN_KIND: &str = "app-image-config";

/// How this service states an `AppImageConfigName` no record is stored under: the
/// `Could not find <kind> "<arn>".` sentence this family refuses a missing resource with,
/// over the ARN the requested name resolves to in the Region the request signed for.
///
/// Stated once so the reader and the update that resolve the same name word the one fact
/// the same way.
pub fn app_image_config_not_found(region: &str, name: &str) -> String {
    format!(
        "Could not find AppImageConfig \"{}\".",
        resource_arn(region, APP_IMAGE_CONFIG_ARN_KIND, name)
    )
}

/// A Git repository registered by `CreateCodeRepository`, as `DescribeCodeRepository`
/// replays it.
///
/// `GitConfig` is required on create, so it is stored unconditionally; the reader's
/// member is optional only because the model marks it so. `tags` is stored so a later
/// `ListTags` reports what the create carried.
/// `last_modified_time` is carried separately from `creation_time` because the modelled
/// output reports both; a repository nothing has changed reports the creation instant.
#[derive(Debug, Clone)]
pub struct CodeRepository {
    pub name: String,
    pub arn: String,
    pub git_config: GitConfig,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// Which Studio principal a Studio app hangs off.
///
/// `CreateApp` / `DescribeApp` / `DeleteApp` all take `UserProfileName` and `SpaceName`
/// as optional members whose model docs say each must be set when the other is not
/// (`smithy_query(query="doc", shape="DeleteAppRequest", member="SpaceName")`), so
/// exactly one of the two identifies the app.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum AppOwner {
    /// `UserProfileName`.
    UserProfile(String),
    /// `SpaceName`.
    Space(String),
}

impl AppOwner {
    /// The owner's name, whichever member named it — the component an app ARN carries.
    pub fn name(&self) -> &str {
        match self {
            Self::UserProfile(name) | Self::Space(name) => name,
        }
    }

    /// `UserProfileName` as the app reports it, i.e. only for a user-profile app.
    pub fn user_profile_name(&self) -> Option<String> {
        match self {
            Self::UserProfile(name) => Some(name.clone()),
            Self::Space(_) => None,
        }
    }

    /// `SpaceName` as the app reports it, i.e. only for a space app.
    pub fn space_name(&self) -> Option<String> {
        match self {
            Self::Space(name) => Some(name.clone()),
            Self::UserProfile(_) => None,
        }
    }
}

/// What identifies one Studio app: the domain, the owning user profile or space, the
/// app type and the app name — the five members every app operation takes. Two apps
/// differing in any one of them are different apps, so this is the store's key.
///
/// Ordered (domain, owner, type, name) so the store has one stable order to list over.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AppKey {
    pub domain_id: String,
    pub owner: AppOwner,
    pub app_type: String,
    pub app_name: String,
}

/// Why a request naming neither or both of `UserProfileName` and `SpaceName` is refused.
///
/// Both members are optional in the model, so the boundary validator passes such a
/// request; the rule lives in their docs ("If this value is not set, then
/// UserProfileName must be set", and the mirror of it), and an app whose owner is
/// unnamed — or named twice — cannot be resolved.
pub const APP_OWNER_REQUIRED: &str =
    "Exactly one of UserProfileName or SpaceName must be specified.";

/// The key the app operations' five input members name, or [`APP_OWNER_REQUIRED`] when
/// the owner is not exactly one of the user profile and the space.
///
/// Shared by `CreateApp`, `DescribeApp` and `DeleteApp` so all three resolve an app the
/// same way: a create and the reader or delete that names the same five members must
/// agree on the key, or the resource is unreachable.
pub fn app_key(
    domain_id: &str,
    user_profile_name: Option<&str>,
    space_name: Option<&str>,
    app_type: &str,
    app_name: &str,
) -> Result<AppKey, &'static str> {
    let owner = match (user_profile_name, space_name) {
        (Some(user_profile), None) => AppOwner::UserProfile(user_profile.to_string()),
        (None, Some(space)) => AppOwner::Space(space.to_string()),
        (Some(_), Some(_)) | (None, None) => return Err(APP_OWNER_REQUIRED),
    };
    Ok(AppKey {
        domain_id: domain_id.to_string(),
        owner,
        app_type: app_type.to_string(),
        app_name: app_name.to_string(),
    })
}

/// A Studio app created by `CreateApp`, as `DescribeApp` replays it.
///
/// `in_service_at` is the instant the clock owes the `Pending` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
#[derive(Debug, Clone)]
pub struct App {
    pub arn: String,
    pub status: String,
    pub resource_spec: Option<ResourceSpec>,
    pub recovery_mode: Option<bool>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub in_service_at: SystemTime,
}

impl App {
    /// Materialise the `Pending` -> `InService` transition the clock already owes, the
    /// way [`Endpoint::settle_status`] does for an endpoint.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == APP_STATUS_PENDING && now >= self.in_service_at {
            self.status = APP_STATUS_IN_SERVICE.to_string();
            return true;
        }
        false
    }
}

/// An artifact created by `CreateArtifact`, as `DescribeArtifact` replays it.
///
/// `properties` is the map the artifact HOLDS, empty while nothing has put an entry in it
/// (see this module's note on an entity's own collections). It is a `BTreeMap` so the map
/// serialises in one stable order whatever order the request arrived in. `source` is held
/// with its `SourceTypes` already resolved to the list the artifact holds, so every reader
/// of it reports the same thing.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub arn: String,
    /// The Region the ARN names, i.e. the one the creating request signed for. Held
    /// separately so the per-Region name uniqueness rule does not have to parse the ARN.
    pub region: String,
    pub name: Option<String>,
    pub artifact_type: String,
    pub source: ArtifactSource,
    pub properties: BTreeMap<String, String>,
    pub metadata_properties: Option<MetadataProperties>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// An artifact's `Source` as the store holds it: the source the request gave, with its
/// `SourceTypes` resolved to the list the artifact HOLDS — the empty list when the request
/// named none (see this module's note on an entity's own collections).
///
/// Applied where an artifact is written, so every reader of the stored source — the
/// describe and the listing's summary — reports the same thing without normalising it
/// again.
pub fn stored_artifact_source(source: ArtifactSource) -> ArtifactSource {
    ArtifactSource {
        source_uri: source.source_uri,
        source_types: Some(source.source_types.unwrap_or_default()),
    }
}

/// A model package group created by `CreateModelPackageGroup`, as
/// `DescribeModelPackageGroup` replays it, the container a VERSIONED model package is
/// registered into, and the resource `PutModelPackageGroupPolicy` attaches a policy to.
///
/// `created_by` is stored at creation rather than rebuilt per read, so every later read
/// reports the identity that actually created the group.
///
/// `resource_policy` is the resource-based policy `PutModelPackageGroupPolicy` attached to
/// the group, exactly as that call sent it, and `None` while the group carries none. It
/// lives ON the group record rather than in a store of its own, so the one group has one
/// policy whichever operation reads it, it is bounded by
/// [`MODEL_PACKAGE_GROUP_QUOTA`] with the record it belongs to, and
/// `DeleteModelPackageGroup` releases the policy with the group.
#[derive(Debug, Clone)]
pub struct ModelPackageGroup {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub managed_configuration: Option<ManagedConfiguration>,
    pub status: String,
    pub created_by: UserContext,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    /// The policy document the last `PutModelPackageGroupPolicy` stored, verbatim.
    pub resource_policy: Option<String>,
}

/// One presigned-URL grant `CreatePartnerAppPresignedUrl` minted: the opaque token the
/// URL carries and when it stops being valid.
///
/// The token is NOT an AWS credential — it is never presented to this emulator and
/// nothing signs with it — so it is a stored grant id rather than a credential-store
/// record.
#[derive(Debug, Clone)]
pub struct PresignedUrlGrant {
    pub token: String,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
    pub session_expires_at: SystemTime,
}

/// What one remembered `BatchAddClusterNodes` `ClientToken` replays: the nodes that call
/// added and the per-instance-group failures it reported, so the same token answers the
/// same thing instead of adding the nodes twice.
#[derive(Debug, Clone)]
pub struct NodeAdditionRecord {
    /// The `NodeLogicalId`s the call added, in the order it answered them.
    pub added: Vec<String>,
    pub failed: Vec<NodeAdditionFailure>,
    /// When the call that this record replays ran, so the token store evicts by age.
    pub issued_at: SystemTime,
}

/// One per-instance-group failure of a `BatchAddClusterNodes` call, as its `Failed` list
/// reported it.
#[derive(Debug, Clone)]
pub struct NodeAdditionFailure {
    pub instance_group_name: String,
    pub error_code: String,
    pub failed_count: i32,
    pub availability_zones: Option<Vec<String>>,
    pub instance_types: Option<Vec<String>>,
    pub message: Option<String>,
}

impl Cluster {
    /// The instance group of this cluster named `name`, as `CreateCluster` stored its
    /// specification.
    pub fn instance_group(
        &self,
        name: &str,
    ) -> Option<&crate::ops::types::ClusterInstanceGroupSpecification> {
        self.instance_groups
            .iter()
            .find(|group| group.instance_group_name == name)
    }

    /// [`Self::instance_group`], for a caller that raises the group's target count.
    pub fn instance_group_mut(
        &mut self,
        name: &str,
    ) -> Option<&mut crate::ops::types::ClusterInstanceGroupSpecification> {
        self.instance_groups
            .iter_mut()
            .find(|group| group.instance_group_name == name)
    }

    /// The node of this cluster with instance id `instance_id`, once the node has one.
    pub fn node_by_instance_id(&self, instance_id: &str) -> Option<&ClusterNode> {
        self.nodes
            .values()
            .find(|node| node.instance_id.as_deref() == Some(instance_id))
    }

    /// [`Self::node_by_instance_id`], for a caller that changes the node it resolves.
    pub fn node_by_instance_id_mut(&mut self, instance_id: &str) -> Option<&mut ClusterNode> {
        self.nodes
            .values_mut()
            .find(|node| node.instance_id.as_deref() == Some(instance_id))
    }

    /// The `NodeLogicalId` of the node of this cluster with instance id `instance_id`, so
    /// a caller holding an instance id reaches the node's own key. Resolved through
    /// [`Self::node_by_instance_id`], so an instance id resolves ONE way whichever caller
    /// asks.
    pub fn logical_id_of_instance(&self, instance_id: &str) -> Option<String> {
        self.node_by_instance_id(instance_id)
            .map(|node| node.logical_id.clone())
    }

    /// Drop the node `logical_id` names from this cluster, lowering by one the target
    /// count of the instance group it belonged to.
    ///
    /// The mirror of what adding a node does (`BatchAddClusterNodes` raises the same
    /// count), so a group's count and the nodes it holds cannot disagree: a group whose
    /// node was deleted asks for one instance fewer, and the provisioning the cluster does
    /// when it reaches `InService` therefore does not mint it again. Answers the node that
    /// was removed, or `None` when this cluster holds no such node.
    pub fn remove_node(&mut self, logical_id: &str) -> Option<ClusterNode> {
        let node = self.nodes.remove(logical_id)?;
        if let Some(group) = self.instance_group_mut(&node.instance_group_name) {
            group.instance_count = group.instance_count.saturating_sub(1);
        }
        Some(node)
    }

    /// Remember `record` under `token`, keeping the token store bounded.
    ///
    /// At most [`NODE_ADDITION_HISTORY_CAP`] tokens are held, so a caller retrying with
    /// token after token cannot grow the store without end. The one dropped is the one
    /// remembered LONGEST AGO (by its `issued_at`, not by how its token sorts), so the
    /// token just recorded is never the one evicted. A token dropped that way is one the
    /// cluster no longer remembers, so the next call carrying it adds nodes rather than
    /// replaying — the same outcome as a token whose idempotency window has passed.
    pub fn remember_node_addition(&mut self, token: String, record: NodeAdditionRecord) {
        self.node_additions.insert(token, record);
        while self.node_additions.len() > NODE_ADDITION_HISTORY_CAP {
            let oldest = self
                .node_additions
                .iter()
                .min_by_key(|(_, held)| held.issued_at)
                .map(|(token, _)| token.clone());
            match oldest {
                Some(token) => self.node_additions.remove(&token),
                // Unreachable while the map is over the cap, and breaking rather than
                // looping forever is the safe answer if it ever is not.
                None => break,
            };
        }
    }
}

/// One node of a cluster, as `DescribeClusterNode` replays it.
///
/// `running_at` is the instant the clock owes the `Pending` -> `Running` transition,
/// which is also when the node gets its EC2 instance id: a caller that describes a node
/// straight after adding it still observes `Pending` with no instance id.
#[derive(Debug, Clone)]
pub struct ClusterNode {
    pub logical_id: String,
    pub instance_id: Option<String>,
    pub instance_group_name: String,
    /// The type the node runs, which is the one its instance group names. Absent for a
    /// group whose specification names `InstanceRequirements` rather than an
    /// `InstanceType`, which the model makes mutually exclusive.
    pub instance_type: Option<String>,
    pub status: String,
    /// Where the node was placed: the zone the add request targeted, since a group's
    /// zones follow from its VPC configuration, which this emulator does not resolve.
    pub availability_zone: Option<String>,
    pub threads_per_core: Option<i32>,
    pub launch_time: SystemTime,
    pub running_at: SystemTime,
    /// The instant the clock owes this node its REPLACEMENT hardware, set by
    /// [`ClusterNode::begin_replacement`]; `None` while no replacement is in flight.
    /// Until then the node reports `ShuttingDown` on the instance being terminated, and
    /// past it it reports `Running` on a freshly minted instance id.
    pub replacement_due_at: Option<SystemTime>,
    /// The volumes attached to this node, keyed by `VolumeId`.
    pub volume_attachments: BTreeMap<String, VolumeAttachment>,
}

/// How a per-node failure words the condition that another operation is already changing
/// the node, which is what `BatchReplaceClusterNodes`' `InstanceIdInUse` and
/// `BatchDeleteClusterNodes`' `NodeIdInUse` both describe ("another operation is already
/// in progress for this node"). Named ONCE here so the two operations cannot word the one
/// condition two ways.
pub const NODE_OPERATION_IN_FLIGHT_MESSAGE: &str = "Another operation is already in \
     progress for this node. Wait for the operation to complete before retrying.";

impl ClusterNode {
    /// Materialise the transition the clock already owes this node — the
    /// `Pending` -> `Running` one of a node that is coming up, or the
    /// `ShuttingDown` -> `Running` one of a node whose replacement hardware is due — and
    /// with it the node's instance id. Answers whether this call made a transition.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == NODE_STATUS_PENDING && now >= self.running_at {
            self.status = NODE_STATUS_RUNNING.to_string();
            self.instance_id = Some(mint_instance_id());
            return true;
        }
        // The replacement instance is a DIFFERENT instance of the same node: it carries a
        // newly minted instance id and its own launch time, while the `NodeLogicalId` —
        // the identifier that "persists throughout the node's lifecycle" — is untouched.
        if let Some(due) = self.replacement_due_at.filter(|due| now >= *due) {
            self.status = NODE_STATUS_RUNNING.to_string();
            self.instance_id = Some(mint_instance_id());
            self.launch_time = due;
            self.running_at = due;
            self.replacement_due_at = None;
            return true;
        }
        false
    }

    /// Whether this node is in a SETTLED status, so an operation that changes or removes
    /// it interrupts nothing the service is already doing to it.
    ///
    /// `Running` is the one settled status this emulator moves a node to; a node that is
    /// still coming up, shutting down, updating or being health-checked is mid-change,
    /// which is the condition the per-node invalid-status codes describe.
    pub fn is_settled(&self) -> bool {
        self.status == NODE_STATUS_RUNNING && self.replacement_due_at.is_none()
    }

    /// Whether this node's hardware may be replaced right now: its status is a settled
    /// one, so nothing the service is already doing to the node is interrupted.
    pub fn accepts_replacement(&self) -> bool {
        self.is_settled()
    }

    /// Whether a replacement is already in flight on this node, i.e. another operation is
    /// in progress for it.
    pub fn is_replacing(&self) -> bool {
        self.replacement_due_at.is_some()
    }

    /// Start the replacement `BatchReplaceClusterNodes` asked for: the instance the node
    /// runs on is terminated — so the node reports `ShuttingDown` and every volume it
    /// held is gone — and the clock owes it fresh hardware
    /// [`TRANSIENT_STATE_SETTLES_AFTER`] later, materialised by
    /// [`ClusterNode::settle_status`].
    ///
    /// The node's `NodeLogicalId` and its instance group, type, zone and threads-per-core
    /// are kept: the replacement runs "the same configuration but fresh hardware". The old
    /// instance id stays until the new instance comes up, because that instance is what
    /// the node is still running on while it shuts down. Replacing a node destroys all its
    /// instance volumes, root and secondary, so none survives the call.
    pub fn begin_replacement(&mut self, now: SystemTime) {
        self.status = NODE_STATUS_SHUTTING_DOWN.to_string();
        self.replacement_due_at = Some(now + TRANSIENT_STATE_SETTLES_AFTER);
        self.volume_attachments.clear();
    }

    /// Materialise every `attaching` -> `attached` transition this node's volumes are
    /// owed.
    pub fn settle_attachments(&mut self, now: SystemTime) {
        for attachment in self.volume_attachments.values_mut() {
            attachment.settle_status(now);
        }
    }

    /// The device name to hand the next volume attached to this node: the first entry of
    /// [`VOLUME_DEVICE_NAMES`] no attachment of this node holds, or `None` once the node
    /// has used them all.
    pub fn free_device_name(&self) -> Option<&'static str> {
        VOLUME_DEVICE_NAMES.into_iter().find(|candidate| {
            !self
                .volume_attachments
                .values()
                .any(|attachment| attachment.device_name == *candidate)
        })
    }
}

/// One EBS volume attached to a cluster node by `AttachClusterNodeVolume`.
#[derive(Debug, Clone)]
pub struct VolumeAttachment {
    pub volume_id: String,
    pub device_name: String,
    pub status: String,
    pub attach_time: SystemTime,
    /// The instant the clock owes the `attaching` -> `attached` transition.
    pub attached_at: SystemTime,
}

impl VolumeAttachment {
    /// Materialise the `attaching` -> `attached` transition the clock already owes.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == VOLUME_STATUS_ATTACHING && now >= self.attached_at {
            self.status = VOLUME_STATUS_ATTACHED.to_string();
            return true;
        }
        false
    }
}

/// One benchmarked recommendation of an Inference Recommender job.
#[derive(Debug, Clone)]
pub struct InferenceRecommendation {
    pub recommendation_id: String,
    pub endpoint_name: String,
    pub instance_type: String,
    /// What one instance of `instance_type` sustained, in invocations per minute.
    pub max_invocations_per_minute: i32,
    /// The model latency measured at that load, in milliseconds.
    pub model_latency_millis: i32,
}

/// Why SageMaker refuses `policy` as a resource policy for a model group, or `None` when
/// it accepts it.
///
/// A resource policy is an IAM policy document: `PolicyString`'s own documentation calls
/// it "the resource policy for the model group", and IAM's policy grammar makes a document
/// a JSON OBJECT (the `Version`/`Statement` elements are its members). The model bounds
/// only the member's length and leaves its pattern `.*`, so the boundary validator passes
/// a document that is not JSON at all and the service is what refuses it.
///
/// Nothing beyond that structural claim is checked here: which elements a statement
/// carries is IAM's business, and a document this accepts is stored and handed back
/// BYTE-FOR-BYTE rather than reformatted, so a caller reads back exactly the text it sent.
///
/// The returned text is the refusal's message; the caller renders it as the failure that
/// operation models for a document it cannot take.
pub fn resource_policy_violation(policy: &str) -> Option<String> {
    match ::serde_json::from_str::<::serde_json::Value>(policy) {
        Ok(::serde_json::Value::Object(_)) => None,
        Ok(_) => Some(
            "ResourcePolicy must be a JSON object: an IAM policy document".to_string(),
        ),
        Err(error) => Some(format!("ResourcePolicy is not valid JSON: {error}")),
    }
}

/// The most notebook instance lifecycle configurations this service's store holds, after
/// which a create is refused with the modelled `ResourceLimitExceeded`. See
/// [`MODEL_QUOTA`] for why the bound is the emulator's own rather than an account's.
pub const NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_QUOTA: usize = 1_000;

/// The `<kind>` component [`resource_arn`] takes for a notebook instance lifecycle
/// configuration, as the AWS ARN reference spells SageMaker's resource type for it.
/// Named once so the create and the describe mint and expect the same ARN.
pub const NOTEBOOK_LIFECYCLE_CONFIG_ARN_KIND: &str = "notebook-instance-lifecycle-config";

/// How this service states a notebook instance lifecycle configuration name no record is
/// stored under: the `Could not find <kind> "<arn>".` sentence this family refuses a
/// missing resource with, over the ARN the requested name resolves to in the Region the
/// request signed for.
///
/// Stated once so the reader and the update that resolve the same name — neither of which
/// models a not-found error of its own, so both refuse with a `ValidationException` — word
/// the one fact the same way.
pub fn notebook_instance_lifecycle_config_not_found(region: &str, name: &str) -> String {
    format!(
        "Could not find notebook instance lifecycle configuration \"{}\".",
        resource_arn(region, NOTEBOOK_LIFECYCLE_CONFIG_ARN_KIND, name)
    )
}

/// A notebook instance lifecycle configuration created by
/// `CreateNotebookInstanceLifecycleConfig`, as `DescribeNotebookInstanceLifecycleConfig`
/// replays it.
///
/// `OnCreate` and `OnStart` are modelled as lists of at most one hook each; they are
/// stored exactly as the create request carried them, because the scripts they hold are
/// opaque base64 the service only ever hands back.
#[derive(Debug, Clone)]
pub struct NotebookInstanceLifecycleConfig {
    pub name: String,
    pub arn: String,
    pub on_create: Option<Vec<crate::ops::types::NotebookInstanceLifecycleHook>>,
    pub on_start: Option<Vec<crate::ops::types::NotebookInstanceLifecycleHook>>,
    /// What the create carried, so a later `ListTags`/`AddTags` reports it; no
    /// implemented operation surfaces it yet (`DescribeNotebookInstanceLifecycleConfig`
    /// models no `Tags` member).
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another notebook instance lifecycle configuration fits, i.e. the store is
    /// below [`NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_QUOTA`].
    ///
    /// Kept with the rest of this resource's slice rather than in the first `impl` block,
    /// as the lineage-action slice at the end of this module is.
    pub fn notebook_instance_lifecycle_configs_below_quota(&self) -> bool {
        self.notebook_instance_lifecycle_configs.len() < NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_QUOTA
    }
}

/// A model package created by `CreateModelPackage`, as `DescribeModelPackage` replays it.
///
/// One record covers both documented kinds: an UNVERSIONED package (the request named
/// `ModelPackageName`, so `group_name` and `version` are absent) and a VERSIONED one (the
/// request named `ModelPackageGroupName`, so `name` is the group's name and `version` is
/// the number this registration was given inside that group).
///
/// `created_by` is stored at creation, as [`ModelPackageGroup`]'s is, so every later read
/// reports the identity that actually registered the package.
///
/// `client_token` is the idempotency token the registering call presented, if any. It is
/// held here rather than in a map of its own so it is bounded by the same
/// [`MODEL_PACKAGE_QUOTA`] as the record and is released with it.
///
/// `completed_at` is the instant the clock owes the `Pending` -> `Completed` transition;
/// it is stored, never slept on, and materialised by whichever reporting call first runs
/// at or after it.
///
/// `last_modified_by` stays absent until a CALL modifies the package (`UpdateModelPackage`
/// records it), the way [`ModelCardVersion`]'s does: the clock settling the registration
/// status is not a caller, so it moves `last_modified_time` without claiming an identity.
///
/// `update_client_tokens` holds the idempotency tokens whose update has already been
/// applied, newest last, so a retry replays instead of appending a second copy of an
/// `AdditionalInferenceSpecificationsToAdd`. It is capped at
/// [`MODEL_PACKAGE_UPDATE_TOKEN_HISTORY`] entries so a caller updating in a loop cannot
/// grow the record without end.
#[derive(Debug, Clone)]
pub struct ModelPackage {
    pub name: String,
    pub group_name: Option<String>,
    pub version: Option<i32>,
    pub arn: String,
    pub description: Option<String>,
    pub registration_type: Option<String>,
    pub inference_specification: Option<InferenceSpecification>,
    pub validation_specification: Option<ModelPackageValidationSpecification>,
    pub source_algorithm_specification: Option<SourceAlgorithmSpecification>,
    pub certify_for_marketplace: Option<bool>,
    pub model_approval_status: Option<String>,
    pub approval_description: Option<String>,
    pub metadata_properties: Option<MetadataProperties>,
    pub model_metrics: Option<ModelMetrics>,
    pub domain: Option<String>,
    pub task: Option<String>,
    pub sample_payload_url: Option<String>,
    pub customer_metadata_properties: Option<HashMap<String, String>>,
    pub drift_check_baselines: Option<DriftCheckBaselines>,
    pub additional_inference_specifications: Option<Vec<AdditionalInferenceSpecificationDefinition>>,
    pub skip_model_validation: Option<String>,
    pub source_uri: Option<String>,
    pub security_config: Option<ModelPackageSecurityConfig>,
    pub model_card: Option<ModelPackageModelCard>,
    pub model_life_cycle: Option<ModelLifeCycle>,
    pub managed_storage_type: Option<String>,
    pub tags: Vec<Tag>,
    pub created_by: UserContext,
    pub last_modified_by: Option<UserContext>,
    pub client_token: Option<String>,
    /// The `ClientToken`s of the updates already applied to this package, newest last.
    pub update_client_tokens: Vec<String>,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub completed_at: SystemTime,
}

impl ModelPackage {
    /// The key this package is stored under: its name for an unversioned package,
    /// `<group>/<version>` for a version registered into a group. It is also the
    /// resource part of the package's ARN, so one identifier serves both.
    pub fn key(&self) -> String {
        model_package_key(&self.name, self.version)
    }

    /// Whether `token` has already applied an update to this package, i.e. this call is a
    /// retry of one this record has seen.
    pub fn update_token_applied(&self, token: &str) -> bool {
        self.update_client_tokens.iter().any(|seen| seen == token)
    }

    /// Remember `token` as applied, dropping the oldest entries once the history is at
    /// [`MODEL_PACKAGE_UPDATE_TOKEN_HISTORY`], so the record stays bounded.
    pub fn remember_update_token(&mut self, token: String) {
        self.update_client_tokens.push(token);
        let excess = self
            .update_client_tokens
            .len()
            .saturating_sub(MODEL_PACKAGE_UPDATE_TOKEN_HISTORY);
        if excess > 0 {
            self.update_client_tokens.drain(..excess);
        }
    }

    /// Materialise the `Pending` -> `Completed` transition the clock already owes.
    ///
    /// The same single write a reporting operation may make as [`Endpoint::settle_status`]:
    /// the transition was due at `completed_at` whether or not anybody described the
    /// package, so the observer makes no difference to the result, and
    /// `LastModifiedTime` moves to the instant it was due rather than to "now" so the
    /// value is stable across later reads.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == MODEL_PACKAGE_STATUS_PENDING && now >= self.completed_at {
            self.status = MODEL_PACKAGE_STATUS_COMPLETED.to_string();
            self.last_modified_time = self.completed_at;
            return true;
        }
        false
    }

    /// `ModelPackageStatusDetails` for this package: one validation item per profile the
    /// request's `ValidationSpecification` named (no profiles, no items), each reporting
    /// `NotStarted` while the package is `Pending` and `Completed` once it has settled.
    ///
    /// `ImageScanStatuses` is reported EMPTY rather than absent, the way an algorithm's
    /// [`Algorithm::status_details`] reports it: the member is how the package reports the
    /// image scans it holds statuses for, and this emulator scans no container image, so
    /// it holds none — which a caller cannot tell from a missing member.
    pub fn status_details(&self) -> ModelPackageStatusDetails {
        let item_status = if self.status == MODEL_PACKAGE_STATUS_COMPLETED {
            DETAILED_MODEL_PACKAGE_STATUS_COMPLETED
        } else {
            DETAILED_MODEL_PACKAGE_STATUS_NOT_STARTED
        };
        let validation_statuses = self
            .validation_specification
            .as_ref()
            .map(|spec| {
                spec.validation_profiles
                    .iter()
                    .map(|profile| ModelPackageStatusItem {
                        name: profile.profile_name.clone(),
                        status: item_status.to_string(),
                        failure_reason: None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        ModelPackageStatusDetails {
            validation_statuses,
            image_scan_statuses: Some(Vec::new()),
        }
    }
}

/// The store key (and ARN resource part) of a model package: `name` on its own for an
/// unversioned package, `name/version` for a version inside a model package group.
///
/// The two forms cannot collide: `name` is an `EntityName` / `ModelPackageGroupName`,
/// whose modelled pattern (`^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`) admits no `/`, and the
/// boundary validator refuses a name outside it before any handler runs.
pub fn model_package_key(name: &str, version: Option<i32>) -> String {
    match version {
        Some(version) => format!("{name}/{version}"),
        None => name.to_string(),
    }
}

/// The ARN segment that marks an identifier as a model package ARN rather than a name:
/// the `:model-package/` before the key [`model_package_lookup_key`] cuts out.
const MODEL_PACKAGE_ARN_SEGMENT: &str = ":model-package/";

/// The store key a caller-supplied model package identifier names.
///
/// `DescribeModelPackage` takes a `VersionedArnOrName`, whose modelled pattern admits
/// exactly three forms: a bare name, a `name/version` pair, or this service's
/// `...:model-package/<key>` ARN — so an identifier reaching here is one of them. An ARN
/// resolves to everything after its `:model-package/` segment, which is exactly the key
/// form above; anything else is already a key.
pub fn model_package_lookup_key(identifier: &str) -> &str {
    identifier
        .split_once(MODEL_PACKAGE_ARN_SEGMENT)
        .map_or(identifier, |(_, resource)| resource)
}

/// Per-service state for com-amazonaws-sagemaker-sagemaker. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakerSagemakerState {
    /// HyperPod clusters and the cluster policies on them: one field for that whole
    /// slice, whose records, quota, statuses and id mint live in [`cluster_policy`].
    pub cluster_policies: cluster_policy::Store,
    /// Keyed by [`entity_key`] of `ExperimentName`. See [`Experiment`], with the rest
    /// of the experiment-management slice.
    pub experiments: BTreeMap<String, Experiment>,
    /// Whether this account's SageMaker Service Catalog portfolio is enabled, as
    /// `EnableSagemakerServicecatalogPortfolio` and
    /// `DisableSagemakerServicecatalogPortfolio` set it and
    /// `GetSagemakerServicecatalogPortfolioStatus` reports it, through
    /// [`Self::servicecatalog_portfolio_status`].
    ///
    /// Account-level, so it is a single flag rather than a map: the three operations take
    /// no input and name no resource. `Default` is `false` — an account that has never
    /// enabled the portfolio reads `Disabled`, the only other member of the modelled enum.
    pub servicecatalog_portfolio_enabled: bool,
    /// Keyed by `ModelName`.
    pub models: BTreeMap<String, Model>,
    /// Keyed by `TrainingJobName`.
    pub training_jobs: BTreeMap<String, TrainingJob>,
    /// Keyed by `InferenceComponentName`.
    pub inference_components: BTreeMap<String, InferenceComponent>,
    /// Keyed by `EndpointConfigName`.
    pub endpoint_configs: BTreeMap<String, EndpointConfig>,
    /// Keyed by `EndpointName`.
    pub endpoints: BTreeMap<String, Endpoint>,
    /// Keyed by [`entity_key`] of their `HyperParameterTuningJobName`:
    /// `HyperParameterTuningJobName`'s doc says the name "must be unique within the same
    /// Amazon Web Services account and Amazon Web Services Region" and "is not case
    /// sensitive", so two spellings differing only in case are ONE job, while the record
    /// keeps the casing the create sent (and the ARN minted from it).
    pub tuning_jobs: BTreeMap<String, HyperParameterTuningJob>,
    /// Trials, keyed by [`entity_key`] of their `TrialName`: the name "must be unique in
    /// your Amazon Web Services account and is not case-sensitive"
    /// (`CreateTrial$TrialName`), so two spellings differing only in case are ONE trial.
    /// The record keeps the casing the create sent. Reached through [`Self::trial`].
    pub trials: BTreeMap<String, Trial>,
    /// Keyed by `AlgorithmName`.
    pub algorithms: BTreeMap<String, Algorithm>,
    /// AutoML jobs V2, keyed by [`entity_key`] of their `AutoMLJobName`.
    pub auto_ml_jobs_v2: BTreeMap<String, AutoMLJobV2>,
    /// Model-customization jobs, keyed by `JobName`, which AWS documents unique "within
    /// your account and Amazon Web Services Region". See [`Job`], whose slice holds the
    /// record, the lookup and the constants.
    pub jobs: BTreeMap<String, Job>,
    /// Lineage contexts, keyed by `ContextArn`: `ContextName` is documented unique to an
    /// account in a Region, so the ARN — which carries the Region beside the name — is
    /// what identifies one context. See [`context_key`].
    pub contexts: BTreeMap<String, Context>,
    /// Keyed by `PipelineName`.
    pub pipelines: BTreeMap<String, Pipeline>,
    /// Pipeline executions, keyed by `PipelineExecutionArn` — the one identifier
    /// `DescribePipelineExecution` resolves an execution by, and the one
    /// `StartPipelineExecution` mints.
    pub pipeline_executions: BTreeMap<String, PipelineExecution>,
    /// Keyed by `TransformJobName`.
    pub transform_jobs: BTreeMap<String, TransformJob>,
    /// Keyed by `MlflowAppArn` — the app's ARN is its identity, since the model
    /// documents that an MLflow app's NAME is not part of its ARN.
    pub mlflow_apps: BTreeMap<String, MlflowApp>,
    /// Keyed by `TrackingServerName`. See [`MlflowTrackingServer`], further down this
    /// module with the rest of the MLflow tracking-server slice.
    pub mlflow_tracking_servers: BTreeMap<String, MlflowTrackingServer>,
    /// Keyed by `NotebookInstanceLifecycleConfigName`.
    pub notebook_instance_lifecycle_configs: BTreeMap<String, NotebookInstanceLifecycleConfig>,
    /// Keyed by `AppImageConfigName`, which AWS documents as unique to the account.
    pub app_image_configs: BTreeMap<String, AppImageConfig>,
    /// Keyed by `CodeRepositoryName`.
    pub code_repositories: BTreeMap<String, CodeRepository>,
    /// Keyed by [`AppKey`] — an app is named by five members, not by one name.
    pub apps: BTreeMap<AppKey, App>,
    /// Keyed by `ModelPackageGroupName`.
    pub model_package_groups: BTreeMap<String, ModelPackageGroup>,
    /// Keyed by `RecommendationJobName`, which AWS documents unique to the account in a
    /// Region. See [`InferenceRecommendationsJob`], with the rest of the Inference
    /// Recommender slice.
    pub inference_recommendations_jobs: BTreeMap<String, InferenceRecommendationsJob>,
    /// Keyed by [`model_package_key`], so an unversioned package and the versions of a
    /// group share one store and one stable order.
    pub model_packages: BTreeMap<String, ModelPackage>,
    /// Lineage artifacts, keyed by `ArtifactArn` — the only identifier `DescribeArtifact`
    /// resolves one by, and the one `CreateArtifact` mints.
    pub artifacts: BTreeMap<String, Artifact>,
    /// Keyed by `FeatureGroupName`. Served by the "Feature Store control plane" section
    /// of this module, which holds the whole feature-group slice — its constants, its
    /// record and its quota helper — in one block.
    pub feature_groups: BTreeMap<String, FeatureGroup>,
    /// Keyed by `NotebookInstanceName`. See [`NotebookInstance`], at the end of this
    /// module with the rest of the notebook-instance slice.
    pub notebook_instances: BTreeMap<String, NotebookInstance>,
    /// Lineage actions, keyed by `ActionArn` — the identifier that carries both the
    /// Region and the name, which is what `ActionName` is documented unique within, so a
    /// name taken in one Region leaves it free in another. See [`Action`], at the end of
    /// this module with the rest of the lineage-action slice.
    pub actions: BTreeMap<String, Action>,
    /// Keyed by `ModelCardName`. See [`ModelCard`], at the end of this module with the
    /// rest of the model-card slice.
    pub model_cards: BTreeMap<String, ModelCard>,
    /// Model card export jobs, keyed by [`model_card_export_job_key`] — the card the job
    /// exports plus the job's own name, which is exactly what the job's ARN names, so the
    /// export jobs of one card sit together in one stable order. See
    /// [`ModelCardExportJob`], at the end of this module with the rest of the model-card
    /// slice.
    pub model_card_export_jobs: BTreeMap<String, ModelCardExportJob>,
    /// Keyed by `AIWorkloadConfigName`. See [`AIWorkloadConfig`], at the end of this
    /// module with the rest of the AI-benchmarking workload-configuration slice.
    pub ai_workload_configs: BTreeMap<String, AIWorkloadConfig>,
    /// Keyed by `AIBenchmarkJobName`. See [`AIBenchmarkJob`], at the end of this module
    /// with the rest of the AI-benchmarking slice.
    pub ai_benchmark_jobs: BTreeMap<String, AIBenchmarkJob>,
    /// Keyed by `AIRecommendationJobName`. See [`AIRecommendationJob`], which lives with
    /// the rest of its slice in the [`ai_recommendation_job`] submodule.
    pub ai_recommendation_jobs: BTreeMap<String, AIRecommendationJob>,
    /// Keyed by `HubName`. See [`Hub`].
    pub hubs: BTreeMap<String, Hub>,
    /// Every stored hub content VERSION, keyed by [`HubContentKey`].
    pub hub_contents: BTreeMap<HubContentKey, HubContent>,
    /// Keyed by `TrialComponentName`, case-folded with [`entity_key`] (the name "is not
    /// case-sensitive" per its model doc), so one component cannot be stored twice under
    /// two spellings. See [`TrialComponent`], at the end of this module with the rest of
    /// the experiment-tracking slice.
    pub trial_components: BTreeMap<String, TrialComponent>,
    /// Which trial components are part of which trial: the
    /// `([entity_key] of TrialName, [entity_key] of TrialComponentName)` pair that IS one
    /// membership's whole identity, the way [`Self::associations`] keys a lineage edge by
    /// its two ends. A component may be part of several trials, and a trial holds many
    /// components, so the membership is an edge rather than a member of either record.
    ///
    /// Written by `AssociateTrialComponent` — the call that attaches a component to a
    /// trial — and read by `ListTrialComponents`, whose `TrialName` and `ExperimentName`
    /// filters are exactly "the components part of this trial" and "part of any trial of
    /// this experiment". The index lives here, beside both stores it relates, rather than
    /// in either of them: a component may be part of several trials and a trial holds many
    /// components, so neither record owns the membership.
    ///
    /// Bounded by [`TRIAL_COMPONENT_TRIAL_QUOTA`].
    pub trial_component_trials: BTreeSet<(String, String)>,
    /// Keyed by `ProcessingJobName`.
    pub processing_jobs: BTreeMap<String, ProcessingJob>,
    /// Keyed by `ComputeQuotaId` — the identifier SageMaker mints and every
    /// compute-quota operation names the resource by (its `Name` is unique only within
    /// one cluster, so the name is not a key). See [`ComputeQuota`], at the end of this
    /// module with the rest of the compute-allocation slice.
    pub compute_quotas: BTreeMap<String, ComputeQuota>,
    /// Keyed by `(Region, LineageGroupName)`: a lineage group is a REGIONAL resource
    /// (see [`lineage_group_arn`], "the ARN of the Region's default lineage group"), so
    /// the Region a lineage entity materialised it in is part of its identity and a
    /// reader signing for another Region does not find it. See [`LineageGroup`], at the
    /// end of this module with the rest of the lineage-group slice.
    pub lineage_groups: BTreeMap<(String, String), LineageGroup>,
    /// Keyed by the partner app's `app-...` id. See [`PartnerApp`], at the end of this
    /// module with the rest of the partner-AI-app slice.
    pub partner_apps: BTreeMap<String, PartnerApp>,
    /// The `PartnerAppType`s this account holds a partner-application subscription for.
    /// See [`ComAmazonawsSagemakerSagemakerState::subscribed_to_partner_app`], which is
    /// what `CreatePartnerApp` admits a type by. EMPTY for an account nothing has
    /// subscribed for: subscribing is a purchase in AWS Marketplace, which no operation
    /// SageMaker models performs, so nothing a client sends over the wire adds to this set
    /// — an in-process caller (a test, or a future AWS Marketplace integration of this
    /// emulator) is what grants one, exactly as an AWS account that has subscribed to
    /// nothing cannot create a partner app at all.
    pub partner_app_subscriptions: BTreeSet<String>,
    /// Keyed by `ImageName`. See [`Image`], at the end of this module with the rest of
    /// the custom-image slice.
    pub images: BTreeMap<String, Image>,
    /// Every stored image VERSION, keyed by `(ImageName, Version)` so the versions of
    /// one image sit together in ascending version order and the latest one is the last
    /// of that range. See [`ImageVersion`].
    pub image_versions: BTreeMap<(String, i32), ImageVersion>,
    /// Keyed by `ClusterName`. See [`Cluster`], at the end of this module with the rest
    /// of the HyperPod cluster slice.
    pub clusters: BTreeMap<String, Cluster>,
    /// Lineage associations, keyed by the `(SourceArn, DestinationArn)` pair that IS one
    /// association's identity — the only identifier `AddAssociation` and
    /// `DeleteAssociation` address one by. See [`Association`], at the end of this module
    /// with the rest of the lineage-association slice.
    pub associations: BTreeMap<(String, String), Association>,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// The account's Service Catalog portfolio status, as the modelled
    /// `SagemakerServicecatalogStatus` enum names it: its two members are `Enabled` and
    /// `Disabled` (`ops::types::SAGEMAKER_SERVICECATALOG_STATUS_VALUES`, generated from
    /// the model shape), and this is the one place either is named.
    pub fn servicecatalog_portfolio_status(&self) -> &'static str {
        if self.servicecatalog_portfolio_enabled {
            "Enabled"
        } else {
            "Disabled"
        }
    }

    /// Whether another model fits, i.e. the store is below [`MODEL_QUOTA`].
    pub fn models_below_quota(&self) -> bool {
        self.models.len() < MODEL_QUOTA
    }

    /// Whether another inference component fits, i.e. the store is below
    /// [`INFERENCE_COMPONENT_QUOTA`].
    pub fn inference_components_below_quota(&self) -> bool {
        self.inference_components.len() < INFERENCE_COMPONENT_QUOTA
    }

    /// Whether another endpoint configuration fits, i.e. the store is below
    /// [`ENDPOINT_CONFIG_QUOTA`].
    pub fn endpoint_configs_below_quota(&self) -> bool {
        self.endpoint_configs.len() < ENDPOINT_CONFIG_QUOTA
    }

    /// Whether another endpoint fits, i.e. the store is below [`ENDPOINT_QUOTA`].
    pub fn endpoints_below_quota(&self) -> bool {
        self.endpoints.len() < ENDPOINT_QUOTA
    }

    /// How many instances of `instance_type` the endpoints this account holds are already
    /// using — the utilisation its hosting quota is measured against.
    ///
    /// Read off the endpoints themselves rather than off the configurations they name, so
    /// what an endpoint holds is what it was created with and a deleted endpoint uses
    /// nothing.
    pub fn endpoint_instances_in_use(&self, instance_type: &str) -> i64 {
        self.endpoints
            .values()
            .filter_map(|endpoint| endpoint.instance_usage.get(instance_type))
            .sum()
    }

    /// Why this account cannot host `demand` on top of what its endpoints already hold, or
    /// `None` when every instance type it names is within quota.
    ///
    /// One instance type past its quota is enough to refuse the whole deployment, and the
    /// one reported is the first in type order, so the same request is always refused the
    /// same way. The sentence is [`account_quota_exceeded`]'s, counted in
    /// [`INSTANCES_UNIT`]: the limit, the utilisation and the delta, which is what a client
    /// reads to decide between asking for fewer instances and requesting an increase.
    pub fn hosting_quota_exceeded(&self, demand: &BTreeMap<String, i64>) -> Option<String> {
        demand.iter().find_map(|(instance_type, delta)| {
            let limit = endpoint_usage_quota(instance_type);
            let in_use = self.endpoint_instances_in_use(instance_type);
            (in_use + delta > limit).then(|| {
                account_quota_exceeded(
                    &endpoint_usage_quota_name(instance_type),
                    INSTANCES_UNIT,
                    limit,
                    in_use,
                    *delta,
                )
            })
        })
    }

    /// Whether another hub fits, i.e. the store is below [`HUB_QUOTA`].
    pub fn hubs_below_quota(&self) -> bool {
        self.hubs.len() < HUB_QUOTA
    }

    /// How many content VERSIONS the hub `hub_name` holds, across every
    /// `HubContentType`: what [`HUB_CONTENT_QUOTA`] bounds.
    pub fn hub_contents_held(&self, hub_name: &str) -> usize {
        self.hub_contents
            .keys()
            .filter(|key| key.hub_name == hub_name)
            .count()
    }

    /// Whether another hyperparameter tuning job fits, i.e. the store is below
    /// [`HYPER_PARAMETER_TUNING_JOB_QUOTA`].
    pub fn tuning_jobs_below_quota(&self) -> bool {
        self.tuning_jobs.len() < HYPER_PARAMETER_TUNING_JOB_QUOTA
    }

    /// Whether another pipeline fits, i.e. the store is below [`PIPELINE_QUOTA`].
    pub fn pipelines_below_quota(&self) -> bool {
        self.pipelines.len() < PIPELINE_QUOTA
    }

    /// Whether another pipeline execution fits, i.e. the store is below
    /// [`PIPELINE_EXECUTION_QUOTA`].
    pub fn pipeline_executions_below_quota(&self) -> bool {
        self.pipeline_executions.len() < PIPELINE_EXECUTION_QUOTA
    }

    /// The execution of `pipeline_name` the `ClientRequestToken` `token` started, if one
    /// is still stored.
    ///
    /// How `StartPipelineExecution` completes "no more than one time": a retry of a
    /// start call answers the execution that call started rather than starting a second
    /// one. The token is scoped to the pipeline, so the same token under another
    /// pipeline's name starts that pipeline's own execution.
    pub fn pipeline_execution_by_token(
        &self,
        pipeline_name: &str,
        token: &str,
    ) -> Option<&PipelineExecution> {
        self.pipeline_executions.values().find(|execution| {
            execution.pipeline_name == pipeline_name && execution.client_request_token == token
        })
    }

    /// Remove the pipeline stored under `name`, with everything that belongs to it, and
    /// answer the record that went — `None` when the account holds no such pipeline.
    ///
    /// `DeletePipeline` is observable AS a delete: recorded AWS makes the pipeline
    /// unreadable from the delete call on, so the record goes at once rather than
    /// lingering in a transient status for a later call to materialise.
    ///
    /// A pipeline's executions go with it: an execution belongs to the pipeline that ran
    /// it, so once the pipeline is gone its executions are no longer resolvable either —
    /// and leaving them would let a pipeline recreated under the freed name inherit the
    /// executions of the one deleted before it.
    pub fn remove_pipeline(&mut self, name: &str) -> Option<Pipeline> {
        let removed = self.pipelines.remove(name)?;
        self.pipeline_executions
            .retain(|_, execution| execution.pipeline_name != removed.name);
        Some(removed)
    }

    /// Whether another transform job fits, i.e. the store is below
    /// [`TRANSFORM_JOB_QUOTA`].
    pub fn transform_jobs_below_quota(&self) -> bool {
        self.transform_jobs.len() < TRANSFORM_JOB_QUOTA
    }

    /// Whether another MLflow app fits, i.e. the store is below [`MLFLOW_APP_QUOTA`].
    pub fn mlflow_apps_below_quota(&self) -> bool {
        self.mlflow_apps.len() < MLFLOW_APP_QUOTA
    }

    /// Whether another AppImageConfig fits, i.e. the store is below
    /// [`APP_IMAGE_CONFIG_QUOTA`].
    pub fn app_image_configs_below_quota(&self) -> bool {
        self.app_image_configs.len() < APP_IMAGE_CONFIG_QUOTA
    }

    /// Whether another model package fits, i.e. the store is below
    /// [`MODEL_PACKAGE_QUOTA`].
    pub fn model_packages_below_quota(&self) -> bool {
        self.model_packages.len() < MODEL_PACKAGE_QUOTA
    }

    /// The version number the next registration into `group` gets: one past the highest
    /// version the group currently holds, so a group's versions count up from 1 and each
    /// registration into it gets its own.
    pub fn next_model_package_version(&self, group: &str) -> i32 {
        self.model_packages
            .values()
            .filter(|package| package.group_name.as_deref() == Some(group))
            .filter_map(|package| package.version)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }

    /// Whether another code repository fits, i.e. the store is below
    /// [`CODE_REPOSITORY_QUOTA`].
    pub fn code_repositories_below_quota(&self) -> bool {
        self.code_repositories.len() < CODE_REPOSITORY_QUOTA
    }

    /// Whether another app fits, i.e. the store is below [`APP_QUOTA`].
    pub fn apps_below_quota(&self) -> bool {
        self.apps.len() < APP_QUOTA
    }

    /// Whether another model package group fits, i.e. the store is below
    /// [`MODEL_PACKAGE_GROUP_QUOTA`].
    pub fn model_package_groups_below_quota(&self) -> bool {
        self.model_package_groups.len() < MODEL_PACKAGE_GROUP_QUOTA
    }

    /// Whether another artifact fits, i.e. the store is below [`ARTIFACT_QUOTA`].
    pub fn artifacts_below_quota(&self) -> bool {
        self.artifacts.len() < ARTIFACT_QUOTA
    }

    /// The artifact called `name` in `region`, if one is stored.
    ///
    /// `CreateArtifact`'s `ArtifactName` is documented unique to an account in a Region,
    /// so this is what a create checks before taking a name. Artifacts are keyed by ARN
    /// (the name is optional), hence the scan over a store bounded by [`ARTIFACT_QUOTA`].
    pub fn artifact_named(&self, region: &str, name: &str) -> Option<&Artifact> {
        self.artifacts
            .values()
            .find(|artifact| artifact.region == region && artifact.name.as_deref() == Some(name))
    }

    /// The ARN of the artifact in `region` whose source URI is `source_uri`, if one is
    /// stored — the OLDEST such artifact when several share the URI.
    ///
    /// `DeleteArtifact` accepts either an `ArtifactArn` or a `Source`, and `ListArtifacts`
    /// filters artifacts by `SourceUri` alone: the URI is what identifies a source, so a
    /// source-addressed request resolves on it rather than on the optional
    /// `SourceTypes` beside it. Nothing stops two creates recording the same URI (the
    /// store is keyed by ARN), so the match is made deterministic by creation time, with
    /// the ARN breaking a tie. Same scan, and same reason, as [`Self::artifact_named`].
    pub fn artifact_arn_for_source(&self, region: &str, source_uri: &str) -> Option<String> {
        self.artifacts
            .values()
            .filter(|artifact| {
                artifact.region == region && artifact.source.source_uri == source_uri
            })
            .min_by(|left, right| {
                left.creation_time
                    .cmp(&right.creation_time)
                    .then_with(|| left.arn.cmp(&right.arn))
            })
            .map(|artifact| artifact.arn.clone())
    }

    /// Whether another processing job fits, i.e. the store is below
    /// [`PROCESSING_JOB_QUOTA`].
    pub fn processing_jobs_below_quota(&self) -> bool {
        self.processing_jobs.len() < PROCESSING_JOB_QUOTA
    }

    /// Whether another training job fits, i.e. the store is below
    /// [`TRAINING_JOB_QUOTA`].
    pub fn training_jobs_below_quota(&self) -> bool {
        self.training_jobs.len() < TRAINING_JOB_QUOTA
    }

    /// How much of the account's parallel-training-jobs quota its hyperparameter tuning
    /// jobs are using: the training jobs a tuning job launched that are still running.
    ///
    /// The quota is counted in TrainingJobs, so the utilisation is a count of training-job
    /// records — the ones a tuning job owns ([`TrainingJob::tuning_job_arn`]) and that have
    /// not yet reached a terminal status. Nothing in this emulator launches a training job
    /// on a tuning job's behalf, so an account that has only ever created training jobs
    /// itself is using none of this quota, and that is read off the store rather than
    /// assumed.
    pub fn parallel_training_jobs_in_use(&self) -> usize {
        self.training_jobs
            .values()
            .filter(|job| {
                job.tuning_job_arn.is_some() && job.status == TRAINING_STATUS_IN_PROGRESS
            })
            .count()
    }

    /// The model package the call that presented `token` registered under the same
    /// identity (`name` plus `group`), if one is stored.
    ///
    /// This is what makes `CreateModelPackage` idempotent: a retry carrying the same
    /// `ClientToken` answers the ARN of the package found here instead of registering
    /// another. The match is scoped to the identity the retry asks for, so a token reused
    /// for a DIFFERENT package is not answered with an unrelated package's ARN.
    ///
    /// The token lives ON the package record rather than in a map of its own, so it is
    /// bounded by [`MODEL_PACKAGE_QUOTA`] and released with the record it belongs to —
    /// hence the scan, as [`Self::artifact_named`] does for the same reason.
    pub fn model_package_for_client_token(
        &self,
        token: &str,
        name: &str,
        group: Option<&str>,
    ) -> Option<&ModelPackage> {
        self.model_packages.values().find(|package| {
            package.client_token.as_deref() == Some(token)
                && package.name == name
                && package.group_name.as_deref() == group
        })
    }

    /// Whether another model card fits, i.e. the store is below [`MODEL_CARD_QUOTA`].
    pub fn model_cards_below_quota(&self) -> bool {
        self.model_cards.len() < MODEL_CARD_QUOTA
    }

    /// Whether another model card export job fits, i.e. the store is below
    /// [`MODEL_CARD_EXPORT_JOB_QUOTA`].
    pub fn model_card_export_jobs_below_quota(&self) -> bool {
        self.model_card_export_jobs.len() < MODEL_CARD_EXPORT_JOB_QUOTA
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakerSagemakerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakerSagemakerState>)
}

/// The SHARED feature-group directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A feature group is DEFINED here (`CreateFeatureGroup`) and USED by the SageMaker
/// FeatureStore Runtime data plane, whose own API carries none of that definition: a
/// `PutRecord` names a feature group and nothing else. So the data plane can resolve that
/// name — and know which feature of a record identifies it and which carries its event
/// time — only because this service RECORDED it, in ONE directory under one shared type
/// ([`::service_common::feature_store_groups::FeatureGroupDirectory`]). The engine keys
/// state by TYPE, so a copy of its own inside [`ComAmazonawsSagemakerSagemakerState`]
/// would be invisible to that reader.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and no
/// reader holds this one while taking another store, so the order cannot invert.
///
/// `FeatureGroupDirectory::forget` belongs to `DeleteFeatureGroup`, which calls it as
/// soon as a deletion is asked for: "Data cannot be accessed from the OnlineStore
/// immediately after DeleteFeatureGroup is called", so the directory stops resolving the
/// name from that instant even though the group itself reports `Deleting` until the clock
/// completes its removal.
///
/// Answer `None` by leaving the directory unwritten: the record is an aid to another
/// service, never part of this operation's own answer.
pub fn feature_group_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::feature_store_groups::FeatureGroupDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::feature_store_groups::FeatureGroupDirectory,
            >,
        )
}

/// The SHARED job directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A job of a reinforcement-fine-tuning category is CREATED here (`CreateJob`) and its
/// rollouts are SERVED by the AgenticRFT job runtime data plane, whose own API carries no
/// job definition and binds no call that creates one: a sampling request names a job ARN
/// in a header and nothing else. So that data plane can resolve the ARN only because this
/// service RECORDED the job, in ONE directory under one shared type
/// ([`::service_common::agentic_rft_jobs::JobDirectory`]). The engine keys state by TYPE,
/// so a copy of its own inside [`ComAmazonawsSagemakerSagemakerState`] would be invisible
/// to that reader.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and no
/// reader holds this one while taking another store, so the order cannot invert.
///
/// `JobDirectory::forget` belongs to `DeleteJob`, which is still a scaffold stub
/// answering 501 (its own issue): nothing in this emulator deletes a job yet, so nothing
/// calls it yet.
///
/// Answer `None` by leaving the directory unwritten: the record is an aid to another
/// service, never part of this operation's own answer.
pub fn job_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::agentic_rft_jobs::JobDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::agentic_rft_jobs::JobDirectory>)
}

/// The message a handler refuses with when the service is mounted on an engine that
/// holds no store for it. Unreachable in the assembled emulator (the router mounts
/// every service on the one shared `InMemoryEngine`); stated once so no handler
/// invents its own wording — and never by building a private engine, which would
/// drop the request's writes.
pub const NO_STATE: &str =
    "SageMaker state is unavailable: this service is not mounted on the shared in-memory engine";

/// The message SageMaker's `ResourceInUse` carries for the resources whose OWN refusal of
/// a name already taken is this sentence.
///
/// It carries nothing from the request: for these resources the service answers the same
/// sentence whatever name the caller asked for, and does not repeat the name back. It is
/// NOT a service-wide default — a name collision is worded per RESOURCE TYPE, and a create
/// whose resource has its own sentence (`CreateAIWorkloadConfig`, `CreateFeatureGroup`,
/// `CreateHub`, `CreateImage`) states that one in its own handler rather than borrowing
/// this. Stated once here so the creates that DO share this sentence — the app image
/// config and the AI job family — cannot word the same fact differently.
pub const RESOURCE_IN_USE_MESSAGE: &str = "The ID or Name specified is already in use.";

/// Whether `value` is a well-formed AWS region token: lowercase alphanumerics and
/// hyphens, at most `emulator_telemetry::policy::REGION_MAX_LEN` (32) bytes.
///
/// An ARN's region component is positional, so a scope carrying `:` or `/` would mint a
/// structurally malformed ARN. Anything outside the grammar is not accepted as a region.
fn is_region_token(value: &str) -> bool {
    is_dns_label(value, 32)
}

/// Whether `label` is a DNS label of at most `max_len` bytes: non-empty, and lowercase
/// alphanumerics and hyphens only. The one definition of that grammar in this crate —
/// [`is_region_token`], which bounds a credential scope's region, and the host-label
/// check `CreatePresignedMlflowAppUrl` applies to an ARN id it splices into a host name,
/// differ only in the cap they pass.
pub(crate) fn is_dns_label(label: &str, max_len: usize) -> bool {
    !label.is_empty()
        && label.len() <= max_len
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The region this request's SigV4 credential scope names, or [`DEFAULT_REGION`] when
/// the request is unsigned or its scope region is not a region token. Every ARN this
/// service mints takes its region from here, so a client signing for one region never
/// reads back an ARN naming another.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_token(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Record a call that changed this service's state, or a status transition it
/// materialised, so an emulator run is auditable the way CloudTrail makes a real one.
///
/// Carries only request metadata: the operation, the resource ARN it changed and the
/// access-key id the REQUEST presented (`emulator_server::extract_akid`). The
/// emulator's own credentials and every secret stay out of it.
pub fn record_state_change(
    operation: &str,
    resource_arn: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) {
    tracing::info!(
        operation,
        resource = resource_arn,
        access_key_id = %::emulator_server::extract_akid(&ctx.parts().headers),
        "sagemaker state change"
    );
}

/// `arn:aws:sagemaker:<region>:<account>:<kind>/<name>` — the one ARN form this
/// service mints, for the resource kinds it owns. `name` is the segment the modelled
/// ARN shape requires: the resource's name for most kinds, and the minted
/// `[a-z0-9]{12}` id for a cluster (see [`mint_cluster_id`]).
pub fn resource_arn(region: &str, kind: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, &format!("{kind}/{name}"))
}

/// The RESOURCE portion of `arn` when `arn` is one [`resource_arn`] mints in `region` for
/// this account, or `None` when it names anything else.
///
/// The read side of [`resource_arn`], and the one place the fixed segments are checked:
/// a resolver that takes an identifier in ARN form (an `…NameOrArn` member, or an
/// ARN-typed member naming a resource) must refuse an ARN of another partition, service,
/// Region or account rather than addressing whatever local record shares its trailing
/// path. The six fields themselves are read by `service_common::arn`, the ONE ARN parse
/// this workspace has, so no service splits an ARN of its own again; only the `aws`
/// partition is accepted, because that is the one every ARN this emulator mints carries
/// and a partition is a closed world — an `aws-cn` ARN names no resource in `aws`.
fn arn_resource<'v>(arn: &'v str, region: &str) -> Option<&'v str> {
    let parsed = ::service_common::arn::parse_of(arn, ARN_SERVICE)?;
    (parsed.partition == "aws"
        && parsed.region == region
        && parsed.account == ::emulator_server::EMULATOR_ACCOUNT_ID)
        .then_some(parsed.resource)
}

/// How SageMaker's lineage, pipeline and experiment-trial readers state a resource the
/// account does not hold: the entity kind, then the identifier RESOLVED TO ITS ARN in
/// single quotes, then `does not exist.`
///
/// These resources are addressed by name OR by ARN (`ExperimentEntityNameOrArn`,
/// `PipelineNameOrArn`) — or, for a trial, by name alone — and the recorded refusals
/// report the ARN the request resolved to whichever form arrived, so the identifier a
/// consumer extracts from the message is the same one it would read off a successful
/// call. The single quotes are part of that rule: a parser that cuts on them has to find
/// them, and they are what delimits the ARN when the resource name carries no separator
/// of its own.
///
/// Stated once here so the reader and the delete of one resource kind, and the kinds
/// sharing the template, cannot word the same fact differently. The hosting resources
/// (model, endpoint configuration, endpoint) use their OWN recorded template instead —
/// `Could not find <kind> "<identifier as the caller sent it>".` — so they do not come
/// through here.
pub fn does_not_exist(entity: &str, arn: &str) -> String {
    entity_not_found(entity, arn, ArnQuoting::SingleQuoted)
}

/// The entity label [`does_not_exist`] names a lineage action by.
pub const ACTION_ENTITY: &str = "Action";

/// The entity label [`does_not_exist`] names a lineage artifact by.
pub const ARTIFACT_ENTITY: &str = "Artifact";

/// How this service states an `ArtifactName` the Region already has an artifact under:
/// the name the request sent, then the ARN of the artifact holding it.
///
/// `ArtifactName` is documented unique to an account in a Region, so every call that
/// takes a name for an artifact — the create that mints one and the update that renames
/// one — refuses a name already taken, and they word that one fact here rather than
/// twice.
pub fn artifact_name_in_use(name: &str, holder_arn: &str) -> String {
    format!("The artifact name \"{name}\" is already in use by \"{holder_arn}\".")
}

/// The entity label [`does_not_exist`] names a lineage context by.
pub const CONTEXT_ENTITY: &str = "Context";

/// The entity label [`does_not_exist`] names ONE END of a lineage association by.
///
/// An `AssociationEntityArn` may name any of five entity kinds (experiment,
/// experiment-trial-component, artifact, action, context), and the operations that resolve
/// one do not care which: the refusal names the end generically rather than guessing the
/// kind out of the ARN it could not resolve.
pub const ASSOCIATION_END_ENTITY: &str = "Resource";

/// The entity label [`does_not_exist`] names a lineage group by.
pub const LINEAGE_GROUP_ENTITY: &str = "Lineage group";

/// The entity label [`does_not_exist`] names a pipeline by.
pub const PIPELINE_ENTITY: &str = "Pipeline";

/// The entity label [`does_not_exist`] names one execution of a pipeline by.
pub const PIPELINE_EXECUTION_ENTITY: &str = "Pipeline execution";

/// The entity label [`does_not_exist`] names an experiment trial by.
pub const TRIAL_ENTITY: &str = "Trial";

/// The entity label [`does_not_exist`] names an experiment by.
pub const EXPERIMENT_ENTITY: &str = "Experiment";

/// The entity label [`does_not_exist`] names an experiment trial component by — two
/// words, as the sentence spells the kind the ARN segment hyphenates
/// ([`TRIAL_COMPONENT_KIND`]).
pub const TRIAL_COMPONENT_ENTITY: &str = "Trial component";

/// How every operation that cannot resolve the experiment a request names states it: the
/// [`does_not_exist`] sentence over the experiment's ARN, resolved from the requested
/// `ExperimentName` against the Region the request signed for.
///
/// The same template as [`trial_does_not_exist`], because an experiment is addressed the
/// same way its trials are — by NAME only (`ExperimentEntityName`) — so the ARN in the
/// message is one [`resource_arn`] resolves rather than the name echoed, and a consumer
/// cuts the same identifier out of the refusal that a successful call would have answered.
pub fn experiment_does_not_exist(region: &str, experiment_name: &str) -> String {
    does_not_exist(
        EXPERIMENT_ENTITY,
        &resource_arn(region, EXPERIMENT_KIND, experiment_name),
    )
}

/// The ARN resource kind an experiment trial carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `TrialArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial/`), and named once
/// here so the create mints and every reader matches the same ARN.
pub const TRIAL_KIND: &str = "experiment-trial";

/// How every operation that cannot resolve the trial a request names states it: the
/// [`does_not_exist`] sentence over the trial's ARN, resolved from the requested
/// `TrialName` against the Region the request signed for.
///
/// A trial is addressed by NAME only (`ExperimentEntityName`), so the ARN in the message
/// is always one [`resource_arn`] resolves — and it is resolved, rather than the name
/// echoed, so the identifier a consumer cuts out of the refusal is the same one a
/// successful call would have answered. Stated once here so the readers, the delete and
/// the two membership operations cannot word the same fact differently.
pub fn trial_does_not_exist(region: &str, trial_name: &str) -> String {
    does_not_exist(TRIAL_ENTITY, &resource_arn(region, TRIAL_KIND, trial_name))
}

/// How every operation that cannot resolve the trial component a request names states it:
/// the [`does_not_exist`] sentence over the component's ARN, resolved from the requested
/// `TrialComponentName` against the Region the request signed for.
///
/// The same template as [`trial_does_not_exist`], for the same reason: a component is
/// addressed by name (or, for the reader, by the ARN that name resolves to), so the
/// identifier the sentence names is the one a successful call would have answered. Stated
/// once here so the reader, the delete and the two membership operations cannot word the
/// same fact differently.
pub fn trial_component_does_not_exist(region: &str, trial_component_name: &str) -> String {
    does_not_exist(
        TRIAL_COMPONENT_ENTITY,
        &resource_arn(region, TRIAL_COMPONENT_KIND, trial_component_name),
    )
}

/// How the `does not exist.` sentence renders the ARN it names — the ONE thing the two
/// recorded templates that use it differ in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArnQuoting {
    /// As the lineage and pipeline family records it: `'<arn>'`.
    SingleQuoted,
    /// As the algorithm and model-package-group family records it: `<arn>`.
    Bare,
}

/// The one `<Entity> <identifier> does not exist.` sentence, with `quoting` deciding how
/// the resolved ARN is rendered.
///
/// Private, and reached through [`does_not_exist`] / [`entity_does_not_exist`], so a call
/// site names the recorded family it belongs to rather than passing a bare flag — and so
/// the sentence itself is written once and the two shapes cannot drift apart.
fn entity_not_found(entity: &str, arn: &str, quoting: ArnQuoting) -> String {
    let identifier = match quoting {
        ArnQuoting::SingleQuoted => format!("'{arn}'"),
        ArnQuoting::Bare => arn.to_string(),
    };
    format!("{entity} {identifier} does not exist.")
}

/// How SageMaker's algorithm and model-package-group operations state a resource the
/// account does not hold: the entity label as the service words it, then the identifier
/// RESOLVED TO ITS ARN — UNQUOTED — then `does not exist.`
///
/// Same sentence as [`does_not_exist`] and the same reason for reporting the resolved ARN
/// (both kinds are addressed by name or by ARN, so the identifier a consumer extracts is
/// the one a successful call would report); the single quotes that family's recorded
/// refusals carry are what differs, which is why the quoting is [`ArnQuoting`]'s to decide
/// and not each caller's.
///
/// Stated once here so the readers, the deletes and the creates that reference one of
/// these resources cannot word the same fact differently.
pub fn entity_does_not_exist(entity: &str, arn: &str) -> String {
    entity_not_found(entity, arn, ArnQuoting::Bare)
}

/// The entity label [`entity_does_not_exist`] names an algorithm by.
pub const ALGORITHM_ENTITY: &str = "Algorithm";

/// The entity label [`entity_does_not_exist`] names a model package group by — one word,
/// as the service spells it there, where the group's own conflict refusal spells it out.
pub const MODEL_PACKAGE_GROUP_ENTITY: &str = "ModelPackageGroup";

/// How SageMaker states that there is no resource policy to report or release for a model
/// group: the fixed clause, then the group's ARN BARE — no quotes around it, and no
/// closing period.
///
/// A name no group is stored under and a group carrying no policy are the SAME answer —
/// there is no policy — so the sentence names the ARN the request's `ModelPackageGroupName`
/// resolves to in the Region it signed for rather than distinguishing the two. Stated once
/// here so the read and the delete of a group policy word the same fact the same way.
pub fn resource_policy_not_found(group_arn: &str) -> String {
    format!("Cannot find resource policy for: {group_arn}")
}

/// How SageMaker's hosting operations state a resource the account does not hold: the
/// resource kind as the service words it, then the identifier AS THE CALLER SENT IT, in
/// double quotes.
///
/// A model, an endpoint configuration and an endpoint are addressed by NAME, and the
/// recorded refusals quote that name — never an ARN the service synthesised from it,
/// which is an identifier the caller never supplied and which a consumer extracting the
/// offending resource from the message cannot match against what it sent.
///
/// Stated once here so every operation that resolves one of these references — the
/// readers, the deletes, and the creates that reference another resource — words the
/// same fact the same way.
pub fn could_not_find(kind: &str, identifier: &str) -> String {
    format!("Could not find {kind} \"{identifier}\".")
}

/// The kind label [`could_not_find`] names a model by.
pub const MODEL_KIND_LABEL: &str = "model";

/// The kind label [`could_not_find`] names an endpoint configuration by.
pub const ENDPOINT_CONFIG_KIND_LABEL: &str = "endpoint configuration";

/// The kind label [`could_not_find`] names an endpoint by.
pub const ENDPOINT_KIND_LABEL: &str = "endpoint";

/// The entity label [`entity_does_not_exist`] names a model package by — one word, the
/// way the service spells it for this resource.
pub const MODEL_PACKAGE_ENTITY: &str = "ModelPackage";

/// The ARN resource kind a model package carries, i.e. the `<kind>` in [`resource_arn`],
/// as the modelled `ModelPackageArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]{9,16}:[0-9]{12}:model-package/`).
pub const MODEL_PACKAGE_KIND: &str = "model-package";

/// How every operation that cannot resolve the model package a request names states it:
/// the [`entity_does_not_exist`] sentence over the package's ARN.
///
/// The identifier is the ARN [`arn_from_name_or_arn`] decides rather than the name echoed,
/// so a bare name, a `<group>/<version>` pair and an ARN all report the one identifier a
/// successful call would have answered with. Stated once here so the readers and the
/// deletes of this resource cannot word the same fact differently.
pub fn model_package_not_found(region: &str, identifier: &str) -> String {
    entity_does_not_exist(
        MODEL_PACKAGE_ENTITY,
        &arn_from_name_or_arn(region, MODEL_PACKAGE_KIND, identifier),
    )
}

/// The `<Entity> '<identifier>' not found` clause SageMaker's AI benchmarking family
/// states an unresolvable reference with, written once so the two sentences built on it
/// cannot drift apart.
fn not_found_clause(entity: &str, identifier: &str) -> String {
    format!("{entity} '{identifier}' not found")
}

/// How SageMaker's AI benchmarking family states a resource the account does not hold: the
/// entity label as the service words it, then the identifier AS THE CALLER SENT IT in
/// single quotes, then `not found.`
///
/// An AI workload configuration, a benchmark job and a recommendation job are addressed by
/// NAME (a job reference is an `AIEntityName`; a configuration reference also admits its
/// ARN), and the recorded refusals quote the identifier the REQUEST carried — never an ARN
/// the service synthesised from it, which is a value the caller never supplied and which a
/// consumer extracting the offending resource from the message cannot match against what
/// it sent.
///
/// Stated once here so the readers, the stops and the deletes of this family word the same
/// fact the same way; a CREATE that cannot resolve a reference of this family states the
/// scope it searched instead ([`ai_entity_not_found_for_account`]).
pub fn ai_entity_not_found(entity: &str, identifier: &str) -> String {
    format!("{}.", not_found_clause(entity, identifier))
}

/// How a CREATE of this family states a referenced resource the account does not hold: the
/// [`ai_entity_not_found`] clause, then the scope the service searched — the account the
/// request was signed for.
///
/// The extra clause is the recorded difference between resolving a reference on the way to
/// creating something and reading the resource itself: a create's caller named a
/// configuration it expected the account to hold, so the refusal says where it was looked
/// for.
pub fn ai_entity_not_found_for_account(entity: &str, identifier: &str) -> String {
    format!("{} for this account.", not_found_clause(entity, identifier))
}

/// The internal service the AI family's DELETE operations front, as the rendering they
/// relay names it.
const AI_INNER_SERVICE: &str = "SageMakerLumen";

/// The status that internal service raised the refusal with, as its rendering states it —
/// the client-error status, the same one this endpoint answers the call with.
const AI_INNER_SERVICE_STATUS: u16 = 400;

/// How many attempts the relayed call took: one. A client error is not retried, so the
/// attempt that raised it is the only one the rendering can count.
const AI_INNER_SERVICE_ATTEMPTS: u8 = 1;

/// How the AI family's DELETE operations state a resource the account does not hold: the
/// family's [`ai_entity_not_found`] sentence, then the tail that names where the failure
/// came from.
///
/// These deletes front an internal service, and what they report is THAT service's own
/// rendering of the failure — the sentence, the service it was raised by, the status it was
/// raised with, the request id of that inner call and how many attempts it took. The
/// family's reads and creates answer the bare sentence, so the tail belongs to the deletes
/// alone.
///
/// The request id is the only per-run field: it is minted per call from the emulator's one
/// request-id mint (`service_common::query_error_envelope::new_request_id`, a fresh v4
/// UUID), because nothing in the request carries one to echo and the inner call is this
/// call's own — so it is indistinguishable from the id the framework's own renderers
/// report, and no value of one recorded run is baked in.
pub fn ai_entity_not_found_relayed(entity: &str, identifier: &str) -> String {
    format!(
        "{} (Service: {AI_INNER_SERVICE}, Status Code: {AI_INNER_SERVICE_STATUS}, \
         Request ID: {}) (SDK Attempt Count: {AI_INNER_SERVICE_ATTEMPTS})",
        ai_entity_not_found(entity, identifier),
        ::service_common::query_error_envelope::new_request_id(),
    )
}

/// The entity label the AI family's not-found sentences name an AI workload configuration
/// by — one word, as the service spells the resource in these messages.
pub const AI_WORKLOAD_CONFIG_ENTITY: &str = "AIWorkloadConfig";

/// The entity label the AI family's not-found sentences name an AI benchmark job by.
pub const AI_BENCHMARK_JOB_ENTITY: &str = "AIBenchmarkJob";

/// How SageMaker's training-job readers state a job this account does not hold.
///
/// A FIXED sentence naming no identifier at all — the recorded wording for this resource,
/// where the hosting operations quote the name the caller sent ([`could_not_find`]) and
/// the lineage family reports a resolved ARN ([`does_not_exist`]). A reader that answers
/// it has stored nothing under the name the request carried.
pub const REQUESTED_RESOURCE_NOT_FOUND: &str = "Requested resource not found.";

/// How SageMaker states a hyperparameter tuning job the account does not hold: the name
/// AS THE CALLER SENT IT, in the sentence that tells it to send another one.
///
/// The recorded wording for this resource — neither the hosting family's quoted
/// identifier ([`could_not_find`]) nor the lineage family's resolved ARN
/// ([`does_not_exist`]) — stated once so every operation that resolves a tuning-job
/// reference words the same fact the same way.
pub fn tuning_job_not_found(name: &str) -> String {
    format!("Amazon SageMaker can't find a tuning job called {name}. Type another name.")
}

/// How SageMaker words a HyperPod cluster this account does not hold, for the operations
/// whose SUBJECT is something other than the cluster itself — a node of it, or a policy
/// hanging off it.
///
/// A FIXED sentence carrying NOTHING from the request: the recorded refusals of those
/// operations state the condition and no identifier, so no refusal leaks an ARN the caller
/// never supplied or a name it can only match against its own input. The sentence is the
/// whole message — the class of the failure travels in the error CODE, so no name of a
/// condition or type is prefixed onto it. Stated once here so every one of them words the
/// same fact the same way. The operations whose subject IS the cluster name it instead
/// ([`cluster_not_found`]).
pub const CLUSTER_NOT_FOUND: &str = "Specified cluster not found";

/// How SageMaker words a HyperPod cluster this account does not hold, for the operations
/// that address the cluster ITSELF: the identifier the request carried, unquoted, inside
/// the recorded sentence — which ends with no period.
///
/// `ClusterName` is a `ClusterNameOrArn`, so the identifier is whichever form arrived,
/// echoed rather than resolved: a cluster's ARN carries the minted cluster id rather than
/// its name, so resolving the name of a cluster that does not exist would report an
/// identifier nobody supplied, and a caller reading the refusal matches it against what it
/// sent. Stated once here so the cluster readers, the delete and the node-inventory and
/// node-batch operations cannot word the same fact differently.
pub fn cluster_not_found(identifier: &str) -> String {
    format!("Cluster with name {identifier} not found")
}

/// How the HyperPod node lookup words a node the named cluster does not own.
///
/// Fixed text, and DIFFERENT from [`CLUSTER_NOT_FOUND`]: the two
/// unknown-identifier cases of one request — the cluster, and the node inside it — are
/// what a caller tells apart by the refusal text, since both carry the same code.
pub const CLUSTER_NODE_NOT_FOUND: &str = "Specified cluster node not found";

/// The one `<Entity> with arn <arn> could not be found` sentence — how the HyperPod
/// task-governance resources (the cluster policy and the compute quota) state one this
/// account does not hold.
///
/// Private, and reached through [`cluster_scheduler_config_not_found`] /
/// [`compute_quota_not_found`], so a call site names the resource it refuses rather than
/// spelling the sentence again. Both resources are addressed by the id SageMaker minted,
/// and both refusals name the ARN that id resolves to — unquoted, with no trailing period.
fn arn_could_not_be_found(entity: &str, arn: &str) -> String {
    format!("{entity} with arn {arn} could not be found")
}

/// The ARN resource kind a cluster policy carries, i.e. the `<kind>` in [`resource_arn`],
/// as the modelled `ClusterSchedulerConfigArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]{9,16}:[0-9]{12}:cluster-scheduler-config/`).
pub const CLUSTER_SCHEDULER_CONFIG_KIND: &str = "cluster-scheduler-config";

/// The entity label [`arn_could_not_be_found`] names a cluster policy by, as the service
/// words it — not the modelled shape name, and not this crate's `cluster policy` prose.
const CLUSTER_SCHEDULER_CONFIG_ENTITY: &str = "Cluster scheduler config";

/// How every operation that cannot resolve the cluster policy a request names states it:
/// the [`arn_could_not_be_found`] sentence over the policy's ARN, resolved from the
/// requested `ClusterSchedulerConfigId` against the Region the request signed for.
pub fn cluster_scheduler_config_not_found(region: &str, config_id: &str) -> String {
    arn_could_not_be_found(
        CLUSTER_SCHEDULER_CONFIG_ENTITY,
        &resource_arn(region, CLUSTER_SCHEDULER_CONFIG_KIND, config_id),
    )
}

/// The ARN resource kind a compute quota carries, i.e. the `<kind>` in [`resource_arn`],
/// as the modelled `ComputeQuotaArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:compute-quota/`).
pub const COMPUTE_QUOTA_KIND: &str = "compute-quota";

/// The entity label [`arn_could_not_be_found`] names a compute quota by, as the service
/// words it.
const COMPUTE_QUOTA_ENTITY: &str = "Compute Quota";

/// How every operation that cannot resolve the compute quota a request names states it:
/// the [`arn_could_not_be_found`] sentence over the quota's ARN, resolved from the
/// requested `ComputeQuotaId` against the Region the request signed for.
pub fn compute_quota_not_found(region: &str, quota_id: &str) -> String {
    arn_could_not_be_found(
        COMPUTE_QUOTA_ENTITY,
        &resource_arn(region, COMPUTE_QUOTA_KIND, quota_id),
    )
}

/// The ARN resource kind a JumpStart hub carries, i.e. the `<kind>` in [`resource_arn`],
/// as the modelled `HubNameOrArn` pattern spells it
/// (`^(arn:[a-z0-9-\.]{1,63}:sagemaker:\w+(?:-\w+)+:(\d{12}|aws):hub\/)?`).
pub const HUB_KIND: &str = "hub";

/// How every operation that cannot resolve the hub a request names states it: the entity,
/// the hub's NAME, and `does not exist.`
///
/// A hub is addressed by `HubNameOrArn` and its ARN carries the hub's name, so both forms
/// resolve to the one name this sentence reports — never an ARN, which is not what the
/// service names here. Stated once so the readers, the delete and the content operations
/// that resolve a hub cannot word the same fact differently.
pub fn hub_not_found(hub_name: &str) -> String {
    format!("Hub with name {hub_name} does not exist.")
}

/// The ARN resource kind a hub content carries, i.e. the `<kind>` in [`resource_arn`], as
/// the modelled `SageMakerPublicHubContentArn` pattern spells it
/// (`^arn:…:sagemaker:\w+(?:-\w+)+:aws:hub-content\/…`).
pub const HUB_CONTENT_KIND: &str = "hub-content";

/// The ARN of ONE version of one hub content:
/// `arn:aws:sagemaker:<region>:<account>:hub-content/<hub>/<type>/<name>/<version>`.
///
/// The hub, the `HubContentType` and the name are what identify a hub content (they are
/// the three members every hub-content operation requires) and the version segment is what
/// makes the ARN name one VERSION of it, which is what the store holds one record per.
pub fn hub_content_arn(
    region: &str,
    hub_name: &str,
    content_type: &str,
    content_name: &str,
    version: &str,
) -> String {
    resource_arn(
        region,
        HUB_CONTENT_KIND,
        &format!("{hub_name}/{content_type}/{content_name}/{version}"),
    )
}

/// How this service refuses a `SageMakerPublicHubContentArn` that resolves to no stored
/// hub content: recorded wire behaviour, carried on `ValidationException` and HTTP 400.
///
/// The sentence carries NOTHING from the request — the service answers the same words
/// whatever ARN arrived — and it is answered BEFORE the hub a reference would be added to
/// is resolved, so a request that names neither a stored source nor an existing hub is
/// refused for the source. Stated once here so every operation that resolves a referenced
/// source cannot word the same fact differently.
pub const SOURCE_ARN_NOT_FOUND: &str = "Requested source ARN does not exist.";

/// The most content versions one hub holds, after which a write that would add another is
/// refused with the modelled `ResourceLimitExceeded`. The emulator's OWN bound, for the
/// reason given on [`MODEL_QUOTA`], and per HUB because that is the resource whose contents
/// grow.
pub const HUB_CONTENT_QUOTA: usize = 1_000;

/// The ARN resource kind a model card carries, i.e. the `<kind>` in [`resource_arn`], as
/// the modelled `ModelCardArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:model-card/`).
pub const MODEL_CARD_KIND: &str = "model-card";

/// How every operation that cannot resolve the model card a request names states it: the
/// entity as the service spells it — one word — the card's ARN, and `does not exist` with
/// NO trailing period.
///
/// A card is addressed by name or by ARN and the refusal reports the ARN
/// [`arn_from_name_or_arn`] decides, so the identifier a consumer cuts out of the message is
/// the one a successful call would have answered with. Stated once here so the readers, the
/// update, the delete and the export jobs over a card word the same fact the same way.
pub fn model_card_not_found(region: &str, identifier: &str) -> String {
    format!(
        "Modelcard {} does not exist",
        arn_from_name_or_arn(region, MODEL_CARD_KIND, identifier)
    )
}

/// Why SageMaker refuses this pipeline definition, or `None` when it accepts it.
///
/// A pipeline definition is a JSON document in the schema SageMaker Pipelines
/// publishes: a `Version` (the only published one is [`PIPELINE_DEFINITION_VERSION`])
/// and a `Steps` list whose entries each carry a `Name` and a `Type`, optionally
/// depending on other steps of the same definition through `DependsOn`. The service
/// resolves that dependency graph when the pipeline is written — not when it runs — so
/// a definition is refused unless
///
/// * it parses as a JSON object carrying those two keys,
/// * every step declares a name no other step in the definition declares,
/// * every `DependsOn` entry names another step the definition declares — never the
///   waiting step itself, and never a step of a kind nothing can wait on
///   ([`PIPELINE_UNDEPENDABLE_STEP_TYPES`]) — and the resulting graph is acyclic,
/// * every property reference to one of the pipeline's own parameters names a parameter
///   the definition declares, and
/// * it declares at most [`PIPELINE_MAX_STEPS`] steps and
///   [`PIPELINE_MAX_STEP_DEPENDENCIES`] dependency edges between them, the two counts
///   the walk's work is the product of — so validating a definition stays short whatever
///   the megabyte the member's modelled length admits is filled with.
///
/// The returned text is the refusal's message; the caller renders it as the 400
/// `ValidationException` that an unsatisfiable definition is refused with. Each DEFECT
/// CLASS carries its own diagnosis, so a caller matching on the reason can tell one
/// definition defect from another: see [`PIPELINE_DAG_CYCLE`],
/// [`step_cannot_depend_on_itself`], [`step_depends_on_unknown_step`],
/// [`step_cannot_depend_on_step_kind`] and [`unknown_parameter_property_reference`].
pub fn pipeline_definition_violation(definition: &str) -> Option<String> {
    let parsed = match ::serde_json::from_str::<::serde_json::Value>(definition) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Some(format!("PipelineDefinition is not valid JSON: {error}"));
        }
    };
    let Some(document) = parsed.as_object() else {
        return Some(
            "PipelineDefinition must be a JSON object carrying the keys Version and Steps"
                .to_string(),
        );
    };
    match document.get("Version").and_then(::serde_json::Value::as_str) {
        None => {
            return Some(format!(
                "PipelineDefinition is missing the required key Version, whose only \
                 supported value is {PIPELINE_DEFINITION_VERSION}"
            ));
        }
        Some(version) if version != PIPELINE_DEFINITION_VERSION => {
            return Some(format!(
                "PipelineDefinition declares the unsupported Version {version}; the only \
                 supported value is {PIPELINE_DEFINITION_VERSION}"
            ));
        }
        Some(_) => {}
    }
    let Some(steps) = document.get("Steps").and_then(::serde_json::Value::as_array) else {
        return Some(
            "PipelineDefinition is missing the required key Steps, a list of the pipeline's \
             steps"
                .to_string(),
        );
    };
    if steps.is_empty() {
        return Some(PIPELINE_DEFINITION_NEEDS_A_STEP.to_string());
    }
    // The cheap pre-check, before any graph work: the step list alone is already past
    // the bound, so nothing below needs to look at it.
    if steps.len() > PIPELINE_MAX_STEPS {
        return Some(too_many_steps(steps.len()));
    }

    let mut declared = DeclaredSteps::default();
    if let Some(violation) = collect_pipeline_steps(steps, 1, &mut declared) {
        return Some(violation);
    }
    // Resolution and the cycle walk both reach a dependency through `by_name`, so each
    // is O((steps + edges) log steps) over counts the two bounds above cap. Every way one
    // EDGE can be unsatisfiable is diagnosed before the graph as a whole is walked, so the
    // refusal states the defect the caller can act on rather than the condition it shows
    // up as: a step naming itself, a target no step declares, or a target of a kind
    // nothing can wait on.
    for step in &declared.steps {
        for dependency in &step.depends_on {
            if *dependency == step.name {
                return Some(step_cannot_depend_on_itself(step.name));
            }
            let Some(&target) = declared.by_name.get(dependency) else {
                return Some(step_depends_on_unknown_step(step.name, dependency));
            };
            // `by_name` only ever holds the index of a step already collected, so the
            // lookup resolves; going on rather than indexing keeps the walk total.
            let Some(target_step) = declared.steps.get(target) else {
                continue;
            };
            if PIPELINE_UNDEPENDABLE_STEP_TYPES.contains(&target_step.step_type) {
                return Some(step_cannot_depend_on_step_kind(
                    step.name,
                    target_step.step_type,
                    dependency,
                ));
            }
        }
    }
    if pipeline_graph_has_cycle(&declared) {
        return Some(PIPELINE_DAG_CYCLE.to_string());
    }
    // Resolved last: a definition whose step graph does not resolve is refused for THAT,
    // and only a definition that does have a graph has arguments worth resolving the
    // references of.
    let parameters: BTreeSet<&str> = declared_parameter_names(&parsed).into_iter().collect();
    unresolved_parameter_reference(&parsed, &parameters)
}

/// How SageMaker diagnoses a step that names ITSELF among the steps it depends on — an
/// edge that can never be satisfied, and the one cycle a caller can read off the step
/// alone, so it is stated as the step's own defect rather than as a property of the graph.
fn step_cannot_depend_on_itself(step: &str) -> String {
    format!("Step: {step} can not depend on itself.")
}

/// How SageMaker diagnoses a `DependsOn` entry naming a step the definition does not
/// declare: both the step that waits and the name nothing declares, as the request sent
/// them, so the caller can tell a misspelled dependency from a missing declaration.
fn step_depends_on_unknown_step(step: &str, missing: &str) -> String {
    format!("Step: {step} depends on unknown Step: {missing}")
}

/// How SageMaker diagnoses a `DependsOn` entry whose target is one of the step kinds
/// nothing may wait on ([`PIPELINE_UNDEPENDABLE_STEP_TYPES`]): the waiting step, the
/// target's KIND (the step's own `Type` with `Step` after it, as the service names the
/// kind) and the target.
fn step_cannot_depend_on_step_kind(step: &str, target_type: &str, target: &str) -> String {
    format!("Step: {step} can not depends on {target_type}Step: {target}")
}

/// How SageMaker diagnoses a definition whose `Steps` list holds nothing.
///
/// FIXED text carrying nothing from the request: the sentence names the part of the
/// request being judged (`Pipeline definition`) and the requirement it breaks — the
/// minimum number of steps a pipeline must declare — with no closing period, because the
/// requirement is quoted rather than written out as prose.
const PIPELINE_DEFINITION_NEEDS_A_STEP: &str =
    "Pipeline definition: At least 1 step must be provided";

/// How SageMaker diagnoses a step graph that is not a DAG.
///
/// FIXED text carrying nothing from the request: the service reports that the resolution
/// found a cycle, and neither the steps on it nor the path through them — a caller acts on
/// the defect class, and the enumeration of a path is one of many the same graph admits.
const PIPELINE_DAG_CYCLE: &str = "Detected cycle in the DAG.";

/// How SageMaker diagnoses a property reference to a pipeline parameter the definition
/// does not declare: the reference AS WRITTEN, in the brackets the service quotes a
/// reference in.
fn unknown_parameter_property_reference(parameter: &str) -> String {
    format!("Unknown property reference [{PIPELINE_PARAMETER_REFERENCE_PREFIX}{parameter}].")
}

/// The first property reference in `document` that addresses a pipeline parameter not
/// among `declared`, as the refusal's message, or `None` when every one of them resolves.
///
/// A definition's steps reach the pipeline's own parameters through the schema's
/// `{"Get": "Parameters.<name>"}` references, which may sit anywhere inside a step's
/// arguments, so the whole document is walked — in document order, and iteratively, so a
/// deeply nested definition cannot drive the walk into unbounded recursion. The work is
/// linear in the document already parsed above, whose size the member's modelled length
/// bounds. A reference with any other prefix addresses something else (a step's
/// properties, the execution) and is not a parameter reference.
fn unresolved_parameter_reference(
    document: &::serde_json::Value,
    declared: &BTreeSet<&str>,
) -> Option<String> {
    let mut pending = vec![document];
    while let Some(value) = pending.pop() {
        match value {
            ::serde_json::Value::Object(members) => {
                if let Some(parameter) = members
                    .get(PIPELINE_PROPERTY_REFERENCE_KEY)
                    .and_then(::serde_json::Value::as_str)
                    .and_then(|reference| {
                        reference.strip_prefix(PIPELINE_PARAMETER_REFERENCE_PREFIX)
                    })
                {
                    if !declared.contains(parameter) {
                        return Some(unknown_parameter_property_reference(parameter));
                    }
                }
                // Reversed, so the walk pops the members in the order the document wrote
                // them and the refusal names the FIRST unresolved reference.
                pending.extend(members.values().rev());
            }
            ::serde_json::Value::Array(entries) => pending.extend(entries.iter().rev()),
            _ => {}
        }
    }
    None
}

/// The refusal for a definition declaring more steps than [`PIPELINE_MAX_STEPS`].
fn too_many_steps(declared: usize) -> String {
    format!(
        "PipelineDefinition declares {declared} steps, more than the {PIPELINE_MAX_STEPS} \
         steps one pipeline definition may hold"
    )
}

/// `<pipeline arn>/execution/<id>` — the one `PipelineExecutionArn` form this service
/// mints, and the form the modelled pattern
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:pipeline\/.*\/execution\/.*$`
/// describes: an execution's ARN extends the ARN of the pipeline it runs.
pub fn pipeline_execution_arn(pipeline_arn: &str, execution_id: &str) -> String {
    format!("{pipeline_arn}/execution/{execution_id}")
}

/// Every step name a pipeline definition declares, in declaration order, counting the
/// steps nested in a condition step's branches — the one namespace a step name lives in.
///
/// Reads the SAME walk `pipeline_definition_violation` validates with, so what a stored
/// definition declares is never computed twice from two readings of the schema. A
/// definition the walk cannot resolve declares nothing addressable, so the answer is
/// then whatever it resolved before the violation; a definition that reached the store
/// was accepted by that validator, so this walk reports none.
pub fn pipeline_definition_step_names(definition: &str) -> Vec<String> {
    pipeline_definition_step_declarations(definition)
        .into_iter()
        .map(|step| step.name)
        .collect()
}

/// One step a pipeline definition declares, in the members a step EXECUTION of it
/// reports: the name it is addressed by, plus the display name and description the
/// definition optionally gives it.
///
/// `PipelineExecutionStep` carries exactly these three as `StepName`,
/// `StepDisplayName` and `StepDescription`
/// (`smithy_query(query="shape",shape="PipelineExecutionStep")`), and a definition's step
/// object carries them under `Name`, `DisplayName` and `Description` — the keys the
/// SageMaker Python SDK's `Step` serialises its own `name`, `display_name` and
/// `description` to. A step that declares neither optional key reports neither member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineStepDeclaration {
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
}

/// Every step a pipeline definition declares, in declaration order, as the control plane
/// knows it — the same walk (and so the same order and the same one step-name namespace)
/// [`pipeline_definition_step_names`] reports the names of.
pub fn pipeline_definition_step_declarations(definition: &str) -> Vec<PipelineStepDeclaration> {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(definition) else {
        return Vec::new();
    };
    let Some(steps) = parsed.get("Steps").and_then(::serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut declared = DeclaredSteps::default();
    let _ = collect_pipeline_steps(steps, 1, &mut declared);
    declared
        .steps
        .iter()
        .map(|step| PipelineStepDeclaration {
            name: step.name.to_string(),
            display_name: step.display_name.map(str::to_string),
            description: step.description.map(str::to_string),
        })
        .collect()
}

/// Every parameter name a pipeline definition declares, in declaration order.
///
/// A definition's optional top-level `Parameters` list is what names the parameters an
/// execution may assign values to — `Parameter$Name`'s model doc: "This parameter name
/// must match a named parameter in the pipeline definition." A definition declaring no
/// `Parameters` names none, so every value a start call supplies for it is unmatched.
pub fn pipeline_definition_parameter_names(definition: &str) -> Vec<String> {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(definition) else {
        return Vec::new();
    };
    declared_parameter_names(&parsed)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The parameter names a parsed definition's top-level `Parameters` list declares, in
/// declaration order — borrowed from the document, so the validator resolves a reference
/// against them without copying the names it already parsed.
///
/// The one reading of that list: [`pipeline_definition_parameter_names`] reports it to the
/// start call that assigns values, and [`pipeline_definition_violation`] resolves the
/// definition's own property references against it, so the two cannot disagree about what
/// a definition declares.
fn declared_parameter_names(document: &::serde_json::Value) -> Vec<&str> {
    document
        .get("Parameters")
        .and_then(::serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("Name").and_then(::serde_json::Value::as_str))
                .collect()
        })
        .unwrap_or_default()
}

/// One step of a pipeline definition, reduced to what the dependency graph is built
/// from and what a step execution of it reports: the name and kind it declares, the step
/// names it depends on, and the display name and description it optionally carries — all
/// borrowed from the parsed document so a large definition is not copied to walk it.
struct PipelineStep<'a> {
    name: &'a str,
    /// The step's declared `Type`, which decides whether another step may depend on it
    /// ([`PIPELINE_UNDEPENDABLE_STEP_TYPES`]) and whether it holds branches of its own.
    step_type: &'a str,
    depends_on: Vec<&'a str>,
    display_name: Option<&'a str>,
    description: Option<&'a str>,
}

/// Every step one definition declares, in declaration order, plus the index that makes
/// a name lookup logarithmic instead of a scan of everything declared so far — the
/// uniqueness check, dependency resolution and the cycle walk all go through it, and
/// `edges` is the running total of `DependsOn` entries, which is bounded as it grows.
#[derive(Default)]
struct DeclaredSteps<'a> {
    steps: Vec<PipelineStep<'a>>,
    by_name: BTreeMap<&'a str, usize>,
    edges: usize,
}

/// Walk `steps` (and the branches of any condition step among them) into `out`,
/// answering the first violation of the per-step rules, if any.
///
/// A condition step's `IfSteps`/`ElseSteps` are steps of the same pipeline: they share
/// the one step-name namespace and may depend on steps outside their branch, so the
/// graph is built over all of them. SageMaker nests them under the step's `Arguments`;
/// both placements are walked, so a definition written either way resolves the same.
///
/// The walk stops as soon as the definition is past [`PIPELINE_MAX_STEPS`] or
/// [`PIPELINE_MAX_STEP_DEPENDENCIES`], so the work it does is bounded by those two
/// counts rather than by the megabyte the member's modelled length admits.
fn collect_pipeline_steps<'a>(
    steps: &'a [::serde_json::Value],
    depth: usize,
    out: &mut DeclaredSteps<'a>,
) -> Option<String> {
    if depth > PIPELINE_MAX_STEP_DEPTH {
        return Some(format!(
            "PipelineDefinition nests steps more than {PIPELINE_MAX_STEP_DEPTH} levels deep"
        ));
    }
    for step in steps {
        let Some(member) = step.as_object() else {
            return Some("every entry of Steps must be a JSON object".to_string());
        };
        let Some(name) = member
            .get("Name")
            .and_then(::serde_json::Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            return Some("a step of PipelineDefinition is missing the required key Name".to_string());
        };
        let Some(step_type) = member.get("Type").and_then(::serde_json::Value::as_str) else {
            return Some(format!(
                "step {name} of PipelineDefinition is missing the required key Type"
            ));
        };
        if out.steps.len() == PIPELINE_MAX_STEPS {
            return Some(too_many_steps(out.steps.len() + 1));
        }
        if out.by_name.contains_key(name) {
            return Some(format!(
                "step name {name} is declared more than once; a pipeline definition's step \
                 names must be unique"
            ));
        }
        let mut depends_on = Vec::new();
        match member.get("DependsOn") {
            None | Some(::serde_json::Value::Null) => {}
            Some(::serde_json::Value::Array(entries)) => {
                if out.edges + entries.len() > PIPELINE_MAX_STEP_DEPENDENCIES {
                    return Some(format!(
                        "PipelineDefinition declares more than the \
                         {PIPELINE_MAX_STEP_DEPENDENCIES} DependsOn entries one pipeline \
                         definition may hold"
                    ));
                }
                out.edges += entries.len();
                for entry in entries {
                    match entry.as_str() {
                        Some(dependency) => depends_on.push(dependency),
                        None => {
                            return Some(format!(
                                "step {name} declares a DependsOn entry that is not a step name"
                            ));
                        }
                    }
                }
            }
            Some(_) => {
                return Some(format!(
                    "step {name} declares DependsOn as something other than a list of step names"
                ));
            }
        }
        out.by_name.insert(name, out.steps.len());
        out.steps.push(PipelineStep {
            name,
            step_type,
            depends_on,
            display_name: member
                .get("DisplayName")
                .and_then(::serde_json::Value::as_str),
            description: member
                .get("Description")
                .and_then(::serde_json::Value::as_str),
        });

        // Only a condition step holds steps of its own, and only under the two branch
        // keys the schema gives it: any other step's `Arguments` are its own payload,
        // so a list that merely happens to be named `Steps` there is NOT walked as
        // pipeline steps.
        if step_type == PIPELINE_CONDITION_STEP_TYPE {
            for branch in ["IfSteps", "ElseSteps"] {
                let placements = [
                    member.get(branch),
                    member.get("Arguments").and_then(|args| args.get(branch)),
                ];
                for nested in placements
                    .into_iter()
                    .flatten()
                    .filter_map(::serde_json::Value::as_array)
                {
                    if let Some(violation) = collect_pipeline_steps(nested, depth + 1, out) {
                        return Some(violation);
                    }
                }
            }
        }
    }
    None
}

/// Whether the steps' `DependsOn` edges form a cycle, i.e. whether the definition's
/// dependency graph is not a DAG.
///
/// A depth-first walk that carries its own stack rather than recursing, so a definition
/// with a long dependency chain cannot exhaust the call stack, and that resolves each
/// edge through the name index rather than scanning the steps. Answers only WHETHER the
/// graph is cyclic, because that is what the refusal states ([`PIPELINE_DAG_CYCLE`]
/// carries no path). A dependency no step declares is skipped here: it is refused before
/// this runs.
fn pipeline_graph_has_cycle(declared: &DeclaredSteps<'_>) -> bool {
    /// Not yet visited.
    const UNSEEN: u8 = 0;
    /// On the current walk's path.
    const ON_PATH: u8 = 1;
    /// Visited, and every path out of it was walked.
    const SETTLED: u8 = 2;

    let steps = &declared.steps;
    let mut colour = vec![UNSEEN; steps.len()];
    for start in 0..steps.len() {
        if colour[start] != UNSEEN {
            continue;
        }
        colour[start] = ON_PATH;
        let mut path: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some(&(node, cursor)) = path.last() {
            let Some(dependency) = steps[node].depends_on.get(cursor) else {
                colour[node] = SETTLED;
                path.pop();
                continue;
            };
            if let Some(frame) = path.last_mut() {
                frame.1 += 1;
            }
            let Some(&next) = declared.by_name.get(dependency) else {
                continue;
            };
            match colour[next] {
                ON_PATH => return true,
                UNSEEN => {
                    colour[next] = ON_PATH;
                    path.push((next, 0));
                }
                _ => {}
            }
        }
    }
    false
}

/// An app's ARN: `...:app/<DomainId>/<owner>/<AppType>/<AppName>`.
///
/// A Studio app is identified by five members, so its ARN names all of them — a single
/// `AppName` would not distinguish two apps of different types under one user profile.
/// Each component is carried exactly as the request supplied it, which is safe for a
/// positional ARN segment because every one of them is already refused at the boundary
/// unless it matches its modelled constraint: `AppName`, `UserProfileName` and
/// `SpaceName` are `len 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`, `DomainId`
/// is `len 0..63, pattern ^d-(-*[a-z0-9]){1,61}$` and `AppType` is an enum
/// (`smithy_query(query="operation", operation="CreateApp")`), so none can carry a `:`,
/// a `/` or a control character and the result always matches `AppArn`'s
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:app/`. The real service composes
/// the ARN from the same five members, so its `len 0..256` bound binds the same inputs.
pub fn app_arn(region: &str, key: &AppKey) -> String {
    resource_arn(
        region,
        "app",
        &format!(
            "{}/{}/{}/{}",
            key.domain_id,
            key.owner.name(),
            key.app_type,
            key.app_name
        ),
    )
}

/// The resource policy a resource nothing has shared carries: an IAM policy document with
/// no statements, which is the empty JSON object.
///
/// `GetLineageGroupPolicy` reports a document for every lineage group it finds — both of
/// its output members are optional, yet a group nobody has granted access to still has a
/// policy, and an IAM policy document is a JSON object (see
/// [`resource_policy_violation`]). So this is the document a group starts with, rather than
/// an absent member a caller would have to read as "there is no policy".
pub const EMPTY_RESOURCE_POLICY: &str = "{}";

/// The ARN resource kind a lineage group carries, i.e. the `<kind>` in [`resource_arn`].
/// Fixed by `LineageGroupArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:lineage-group/`), and named once
/// here so the ARN mint and the readers that resolve an ARN back to a name cannot drift.
pub const LINEAGE_GROUP_KIND: &str = "lineage-group";

/// The ARN of one named lineage group, as `LineageGroupArn` carries it. The single owner
/// of the `lineage-group` ARN resource kind, so a listing and a describe cannot drift
/// apart.
pub fn named_lineage_group_arn(region: &str, name: &str) -> String {
    resource_arn(region, LINEAGE_GROUP_KIND, name)
}

/// The ARN of the Region's default lineage group (see [`DEFAULT_LINEAGE_GROUP`]), as
/// `LineageGroupArn` carries it for an entity that belongs to no other group.
pub fn lineage_group_arn(region: &str) -> String {
    named_lineage_group_arn(region, DEFAULT_LINEAGE_GROUP)
}

/// Mint the opaque id an ARN carries for a resource whose ARN does NOT contain the
/// resource's name: 32 lowercase hex digits, the form SageMaker's `artifact/<id>` and
/// `mlflow-app/<id>` ARNs take. The caller never supplies it — such a resource is named
/// by its ARN, so the id is the server's to generate, and it is derived from nothing in
/// the request. (`MlflowAppName` is `len 1..256` while `MlflowAppArn` is `len 1..128`,
/// and the `CreateMlflowApp` model doc says the name "is not part of the ... ARN": an
/// id-bearing ARN is what keeps a minted ARN inside the modelled length whatever the
/// name's length.)
///
/// Emulator-grade and deliberately NOT cryptographic: the bits come from SHA-256 over
/// three sources, which make an id unique within a run but no more unpredictable than
/// what they carry — a `RandomState` hash of the counter (std's hash-DoS seed, not a
/// CSPRNG), a monotonic counter, so two ids minted in the same nanosecond on different
/// threads differ by construction, and the wall clock in nanoseconds, so ids do not
/// repeat across processes. Such an id is an identifier inside an authenticated store,
/// never a secret; nothing here may be relied on as one.
pub fn mint_opaque_resource_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static MINTED: AtomicU64 = AtomicU64::new(0);
    let counter = MINTED.fetch_add(1, Ordering::Relaxed);

    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(counter);
    let entropy = hasher.finish();

    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or_default();

    let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
    ::sha2::Digest::update(&mut digest, entropy.to_le_bytes());
    ::sha2::Digest::update(&mut digest, counter.to_le_bytes());
    ::sha2::Digest::update(&mut digest, nanos.to_le_bytes());
    let hashed = ::sha2::Digest::finalize(digest);

    // 16 bytes of the digest, hex-rendered: the 32-digit form the real ARNs carry.
    ::hex::encode(&hashed[..16])
}

/// The digest this emulator reports for a container image, in the form `ImageDigest`
/// carries (`len 0..72`, pattern `^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$`).
///
/// It is the emulator's OWN identifier for the image reference, not a digest any registry
/// resolved: nothing here pulls an image, so there is no manifest to hash and the value
/// cannot match what ECR would report for that reference. What it does keep is every
/// property a caller can check without a registry — the modelled form, one digest per
/// reference, the same digest on every read of it, and practically never a collision
/// between two references — because it is a SHA-256 over the reference itself and over
/// nothing else, so it depends on neither the clock nor the request that asked for it.
pub fn derived_image_digest(image: &str) -> String {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(image.as_bytes());
    format!("sha256:{}", ::hex::encode(digest))
}

/// Fill in what SageMaker itself resolves for one container of an inference
/// specification: the digest of the image the container names, and whether its model data
/// is a training checkpoint — which a request that says nothing leaves `false`.
///
/// Called on the WRITE path, so the stored record is the materialised container every
/// reader of it replays, rather than each reader defaulting again.
fn materialise_container(container: &mut ModelPackageContainerDefinition) {
    if container.image_digest.is_none() {
        container.image_digest = container.image.as_deref().map(derived_image_digest);
    }
    container.is_checkpoint = Some(container.is_checkpoint.unwrap_or(false));
}

/// [`materialise_container`] for every container of an inference specification.
pub fn materialise_inference_specification(specification: &mut InferenceSpecification) {
    for container in &mut specification.containers {
        materialise_container(container);
    }
}

/// [`materialise_container`] for every container of an ADDITIONAL inference
/// specification, whose containers are the same shape and are resolved the same way.
pub fn materialise_additional_inference_specifications(
    specifications: &mut [AdditionalInferenceSpecificationDefinition],
) {
    for specification in specifications {
        for container in &mut specification.containers {
            materialise_container(container);
        }
    }
}

/// The `ContainerMode` a model's container hosts its models in when the request named
/// none: one model per container, as the model's `ContainerMode` enum spells it.
pub const CONTAINER_MODE_SINGLE_MODEL: &str = "SingleModel";

/// Fill in what SageMaker itself resolves for a model's container: whether it "hosts a
/// single model or multiple models" (`ContainerDefinition$Mode`), which a request that
/// says nothing leaves at one model per container.
pub fn materialise_container_mode(container: &mut ContainerDefinition) {
    if container.mode.is_none() {
        container.mode = Some(CONTAINER_MODE_SINGLE_MODEL.to_string());
    }
}

/// Fill in what SageMaker itself resolves for an algorithm's training specification: the
/// digest of the training image, and each training channel's `IsRequired` — which a
/// request that says nothing leaves `false`.
pub fn materialise_training_specification(specification: &mut TrainingSpecification) {
    if specification.training_image_digest.is_none() {
        specification.training_image_digest =
            Some(derived_image_digest(&specification.training_image));
    }
    for channel in &mut specification.training_channels {
        channel.is_required = Some(channel.is_required.unwrap_or(false));
    }
}

/// The resource NAME an `ArnOrName`-typed member addresses.
///
/// Several SageMaker readers accept either a bare name or the resource's full ARN for the
/// same resource (the `ArnOrName` and `HubNameOrArn` shapes, the latter modelled
/// `^(arn:…:hub\/)?<name>$`), and the store is keyed by the name — so an ARN is
/// reduced to the resource portion after its final `/`. A value that is not an ARN is
/// already a name and is returned unchanged.
///
/// KNOWN ASYMMETRY, recorded here rather than fixed in one operation: this reduction does
/// NOT check the ARN's partition, Region or account, so an ARN naming another Region or
/// account still resolves the locally stored resource of the same name, and an ARN this
/// service mints for it carries the REQUEST's Region rather than the one the caller
/// supplied. The resolvers for ARN-typed members are strict instead (they go through
/// `arn_resource`, which is what [`entity_name_from_name_or_arn`] and
/// [`model_card_export_job_key_from_arn`] check with). Tightening this one changes every
/// `ArnOrName` reader in the service at once — `DescribeModelCard`, `UpdateModelCard`,
/// `CreateModelCardExportJob`, the hub readers — so it belongs to a change that covers
/// them together, with their refusal wording, rather than to any single operation.
pub fn name_from_arn_or_name(value: &str) -> &str {
    match value.strip_prefix(ARN_PREFIX) {
        Some(_) => value.split('/').next_back().unwrap_or(value),
        None => value,
    }
}

/// The IAM user name the request's SigV4 credential names, or a fixed `emulator`
/// principal when the request carries no usable access key id.
///
/// The access key id is request text and the value is spliced into an ARN, whose segments
/// are positional, so anything not shaped like an access key id is replaced rather than
/// interpolated. `extract_akid` performs no verification — the emulator verifies signatures
/// at its boundary.
fn caller_principal(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    let akid = ::emulator_server::extract_akid(&ctx.parts().headers);
    let shaped_like_an_access_key_id = !akid.is_empty()
        && akid.len() <= 128
        && akid.chars().all(|character| character.is_ascii_alphanumeric());
    if shaped_like_an_access_key_id {
        akid
    } else {
        "emulator".to_string()
    }
}

/// The `UserContext` SageMaker records as the creator of a resource.
///
/// The emulator serves no Studio domain, so the Studio members (`UserProfileArn`,
/// `UserProfileName`, `DomainId`) are ABSENT rather than empty: a caller cannot tell an
/// empty profile name from a missing one. The IAM identity is the one the request signed
/// with, reported as both the ARN of that identity and the unique id AWS gives it — the id
/// is a function of the ARN ([`service_common::caller_principal::aws_unique_id`]), so it is
/// stable across reads and across restarts and two identities practically never share one.
/// `SourceIdentity` stays absent: it is set only when a role session carries one, and no
/// call here does.
pub fn caller_user_context(ctx: &smithy_server_runtime::OperationContext<'_>) -> UserContext {
    let principal = caller_principal(ctx);
    // IAM is global, so the region segment is empty.
    let arn = ::emulator_server::emulator_arn("iam", "", &format!("user/{principal}"));
    UserContext {
        user_profile_arn: None,
        user_profile_name: None,
        domain_id: None,
        iam_identity: Some(IamIdentity {
            principal_id: Some(::service_common::caller_principal::aws_unique_id(
                IAM_USER_ID_PREFIX,
                &arn,
            )),
            arn: Some(arn),
            source_identity: None,
        }),
    }
}

/// The type prefix an IAM USER's unique id carries, from IAM's "unique identifier
/// prefixes" table — the identity kind this emulator's long-term credential speaks for.
const IAM_USER_ID_PREFIX: &str = "AIDA";

/// A stored `SystemTime` as the modelled `Timestamp` the wire carries.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since_epoch) => ::aws_smithy_types::DateTime::from_secs_and_nanos(
            i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
            since_epoch.subsec_nanos(),
        ),
        // A host clock set before the epoch: keep the sign rather than clamping the
        // instant to zero and reporting a time the resource was not created at. Built
        // from whole seconds plus a non-negative nanos remainder, so this branch is as
        // lossless as the one above.
        Err(before_epoch) => {
            let magnitude = before_epoch.duration();
            let secs = i64::try_from(magnitude.as_secs()).unwrap_or(i64::MAX);
            match magnitude.subsec_nanos() {
                0 => ::aws_smithy_types::DateTime::from_secs(-secs),
                nanos => ::aws_smithy_types::DateTime::from_secs_and_nanos(
                    -secs - 1,
                    1_000_000_000 - nanos,
                ),
            }
        }
    }
}

/// A modelled `Timestamp` as a comparable key, so a stored time and a filter bound
/// order against each other without going through a lossy float.
pub fn timestamp_key(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

// ---------------------------------------------------------------------------
// Feature Store control plane
//
// Everything the feature-group slice of this service needs — its constants, its record,
// its status transition and its quota helper — is kept together in this ONE section, the
// way the lineage-action and AI-workload-configuration slices below are, so one
// resource's state reads as one block. Its shapes are named by their full
// `crate::ops::types::` path rather than through a `use`, so the slice carries no import
// of its own.
// ---------------------------------------------------------------------------

/// How long after `CreateFeatureGroup` the clock owes the `Creating` -> `Created`
/// transition.
///
/// Same contract as [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the feature
/// group straight after creating it still observes `Creating`, and one that polls (as
/// `aws sagemaker wait` and the Feature Store SDK's own wait loop do) observes
/// `Created`. The model's doc puts the real service at "approximately 10-15 minutes to
/// provision an OnlineStore FeatureGroup with the InMemory StorageType", so the span is
/// compressed, never zero.
pub const FEATURE_GROUP_CREATED_AFTER: Duration = Duration::from_millis(300);

/// How long after `DeleteFeatureGroup` the clock owes the `Deleting` -> gone transition
/// of a feature group.
///
/// The same compressed, never-zero span as [`FEATURE_GROUP_CREATED_AFTER`]: a caller
/// that describes the group straight after asking for its deletion still observes
/// `Deleting`, and one that polls observes the group gone. The model's doc puts the real
/// service at "approximately 10-15 minutes to delete an OnlineStore FeatureGroup with the
/// InMemory StorageType", so the span is compressed, never zero.
pub const FEATURE_GROUP_RELEASED_AFTER: Duration = FEATURE_GROUP_CREATED_AFTER;

/// See [`MODEL_QUOTA`]. `CreateFeatureGroup`'s doc points at the published quota
/// ("Check Amazon Web Services service quotas to see the FeatureGroups quota for your
/// Amazon Web Services account"), which is per-account and adjustable, so this is the
/// emulator's own bound rather than a claim about any account's limit.
pub const FEATURE_GROUP_QUOTA: usize = 1_000;

/// `FeatureGroupStatus`'s members this emulator moves a feature group through (the
/// modelled enum also carries `CreateFailed` and `DeleteFailed`, which this slice does
/// not reach: nothing here fails a feature group). Named once here so no handler compares
/// a bare string literal. Deliberately separate from [`STATUS_CREATING`] even though the
/// two spell the same value today: they are members of two different modelled enums, and
/// either may be re-spelled without the other.
pub const FEATURE_GROUP_STATUS_CREATING: &str = "Creating";
/// See [`FEATURE_GROUP_STATUS_CREATING`].
pub const FEATURE_GROUP_STATUS_CREATED: &str = "Created";
/// See [`FEATURE_GROUP_STATUS_CREATING`]. What a group reports from the
/// `DeleteFeatureGroup` that asked for its deletion until the clock completes it.
pub const FEATURE_GROUP_STATUS_DELETING: &str = "Deleting";

/// The `OfflineStoreStatusValue` an offline store this emulator holds reports: the
/// replication it describes never fails here, so it is never `Blocked`.
pub const OFFLINE_STORE_STATUS_ACTIVE: &str = "Active";

/// The `ThroughputMode` a feature group runs under when its create stated no
/// `ThroughputConfig`: the mode that bills per read and write and needs no stated
/// capacity ("With on-demand mode, you are charged for data reads and writes that your
/// application performs on your feature group. You do not need to specify read and write
/// throughput" — `ThroughputConfig`), spelled as the model's `ThroughputMode` enum spells
/// it.
pub const THROUGHPUT_MODE_ON_DEMAND: &str = "OnDemand";

/// The throughput configuration a feature group whose create stated none runs under.
pub fn default_throughput_config() -> crate::ops::types::ThroughputConfig {
    crate::ops::types::ThroughputConfig {
        throughput_mode: THROUGHPUT_MODE_ON_DEMAND.to_string(),
        // "This field is not applicable for on-demand feature groups."
        provisioned_read_capacity_units: None,
        provisioned_write_capacity_units: None,
    }
}

/// How SageMaker words a feature group this account does not hold: the `Resource Not
/// Found:` prefix, then the resource type and the name the request resolved to — a
/// sentence that ends with NO period.
///
/// Stated once here so the feature-group readers and the delete cannot word the same fact
/// differently. The name is the one the request addressed the group by, reduced from the
/// ARN form where the member accepts one ([`name_from_arn_or_name`]).
pub fn feature_group_not_found(name: &str) -> String {
    format!("Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name {name}")
}

/// How SageMaker words a refusal to delete a feature group that is not provisioned yet:
/// the `Validation Error:` prefix, the group's name, and the status that stands in the
/// way — reported in the upper case this message spells a `FeatureGroupStatus` in.
pub fn feature_group_not_deletable(name: &str, status: &str) -> String {
    format!(
        "Validation Error: FeatureGroup with name {name} cannot be deleted, because \
         FeatureGroupStatus is '{}'.",
        status.to_uppercase()
    )
}

/// The feature names a `FeatureGroup` may not declare, from `FeatureDefinitions`' own
/// doc: "FeatureNames cannot be any of the following: is_deleted, write_time,
/// api_invocation_time" — Feature Store writes those three columns itself.
pub const RESERVED_FEATURE_NAMES: &[&str] = &["is_deleted", "write_time", "api_invocation_time"];

/// A feature group created by `CreateFeatureGroup`, as `DescribeFeatureGroup` replays
/// it.
///
/// `created_due_at` is the instant the clock owes the `Creating` -> `Created` transition;
/// like [`Endpoint::in_service_at`] it is stored, never slept on, and materialised by
/// whichever reporting call first runs at or after it.
#[derive(Debug, Clone)]
pub struct FeatureGroup {
    pub name: String,
    pub arn: String,
    pub record_identifier_feature_name: String,
    pub event_time_feature_name: String,
    pub feature_definitions: Vec<crate::ops::types::FeatureDefinition>,
    pub online_store_config: Option<crate::ops::types::OnlineStoreConfig>,
    pub offline_store_config: Option<crate::ops::types::OfflineStoreConfig>,
    pub throughput_config: Option<crate::ops::types::ThroughputConfig>,
    pub role_arn: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<Tag>,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// WHEN the `Created` status falls due, not when the group was created — that is
    /// `creation_time`. Named the way [`Endpoint::in_service_at`] names the same thing.
    pub created_due_at: SystemTime,
    /// The instant the clock owes this group's removal, set by
    /// [`FeatureGroup::begin_deletion`]; `None` while no `DeleteFeatureGroup` has asked
    /// for one. Past it the group is gone for every caller
    /// ([`FeatureGroup::is_released`]), whether or not a sweep has dropped the record
    /// yet — the way [`Cluster::removal_due_at`] carries the same fact.
    pub removal_due_at: Option<SystemTime>,
}

impl FeatureGroup {
    /// Materialise the `Creating` -> `Created` transition the clock already owes.
    ///
    /// The one write a reporting operation may make, for the reason spelled out on
    /// [`Endpoint::settle_status`]: the transition was due at `created_due_at` whether or
    /// not anybody described the group.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == FEATURE_GROUP_STATUS_CREATING && now >= self.created_due_at {
            self.status = FEATURE_GROUP_STATUS_CREATED.to_string();
            self.last_modified_time = self.created_due_at;
            return true;
        }
        false
    }

    /// Start the deletion `DeleteFeatureGroup` asked for: the group reports `Deleting`,
    /// and the clock owes its removal [`FEATURE_GROUP_RELEASED_AFTER`] later.
    ///
    /// The record is NOT dropped here: `FeatureGroupStatus` carries `Deleting` and
    /// SageMaker answers it until the deletion finishes, so the transient status is
    /// observable and the removal is the clock's — stored as an instant and never slept
    /// on. The status change is an update to the group, so it moves
    /// `last_modified_time` too.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = FEATURE_GROUP_STATUS_DELETING.to_string();
        self.last_modified_time = now;
        self.removal_due_at = Some(now + FEATURE_GROUP_RELEASED_AFTER);
    }

    /// Whether a `DeleteFeatureGroup` has asked for this group's deletion, i.e. it is
    /// `Deleting` and on its way out.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the deletion the clock owed has completed, i.e. the group is gone.
    ///
    /// A lookup answers `None` for such a group and a listing leaves it out, so no caller
    /// sees a group whose removal was already due — the record itself is dropped by
    /// [`ComAmazonawsSagemakerSagemakerState::release_deleted_feature_groups`], which
    /// frees the name and the quota slot it held.
    pub fn is_released(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due| now >= due)
    }

    /// This group's throughput configuration as `DescribeFeatureGroup` reports it.
    ///
    /// `ThroughputConfig` (what a create states) and `ThroughputConfigDescription`
    /// (what a describe reports) are two shapes with the same three members, so the
    /// active configuration is the one stored for the group, re-shaped. Every group has
    /// one: a create that stated none is stored with
    /// [`default_throughput_config`], the mode the group then runs under.
    pub fn throughput_config_description(
        &self,
    ) -> Option<crate::ops::types::ThroughputConfigDescription> {
        self.throughput_config.as_ref().map(|config| {
            crate::ops::types::ThroughputConfigDescription {
                throughput_mode: config.throughput_mode.clone(),
                provisioned_read_capacity_units: config.provisioned_read_capacity_units,
                provisioned_write_capacity_units: config.provisioned_write_capacity_units,
            }
        })
    }

    /// Whether this group keeps an `OnlineStore`: "Turn OnlineStore off by specifying
    /// False for the EnableOnlineStore flag. … The default value is False."
    /// (`OnlineStoreConfig$EnableOnlineStore`.)
    pub fn online_store_enabled(&self) -> bool {
        self.online_store_config
            .as_ref()
            .and_then(|config| config.enable_online_store)
            .unwrap_or(false)
    }
}

/// The feature-group half of the store's API, in its own `impl` block for the same
/// reason as the section: a resource's state reads as one block.
impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another feature group fits, i.e. the store is below
    /// [`FEATURE_GROUP_QUOTA`].
    pub fn feature_groups_below_quota(&self) -> bool {
        self.feature_groups.len() < FEATURE_GROUP_QUOTA
    }

    /// The feature group a `FeatureGroupName` or `FeatureGroupNameOrArn` names, or `None`
    /// when this account holds none under it.
    ///
    /// The one lookup every feature-group operation resolves that member with, so no
    /// handler re-implements it: the ARN form is reduced to the name the store is keyed by
    /// ([`name_from_arn_or_name`]), and a group whose removal the clock already completed
    /// resolves to nothing — past that instant the group is gone whether or not the record
    /// has been dropped yet ([`FeatureGroup::is_released`]).
    pub fn feature_group_mut(&mut self, name_or_arn: &str) -> Option<&mut FeatureGroup> {
        let now = SystemTime::now();
        self.feature_groups
            .get_mut(name_from_arn_or_name(name_or_arn))
            .filter(|group| !group.is_released(now))
    }

    /// Drop every feature group whose removal the clock already completed, answering
    /// their ARNs in store order.
    ///
    /// Materialises only what was already due: the removal was owed at the stored instant
    /// whether or not anybody called, and a lookup already answers `None` for such a group
    /// ([`FeatureGroup::is_released`]), so this changes nothing a later caller sees — it
    /// frees the name and the quota slot the gone group still held.
    pub fn release_deleted_feature_groups(&mut self, now: SystemTime) -> Vec<String> {
        let released: Vec<(String, String)> = self
            .feature_groups
            .values()
            .filter(|group| group.is_released(now))
            .map(|group| (group.name.clone(), group.arn.clone()))
            .collect();
        for (name, _) in &released {
            self.feature_groups.remove(name);
        }
        released.into_iter().map(|(_, arn)| arn).collect()
    }
}

// ---------------------------------------------------------------------------
// Notebook instances
//
// Everything `CreateNotebookInstance`, `StopNotebookInstance` and
// `DescribeNotebookInstance` need, kept in one
// block: the record the store holds, the spans and defaults those operations apply,
// and the two values the create derives (the Jupyter host and the network-interface id).
// ---------------------------------------------------------------------------

/// How long after `CreateNotebookInstance` the clock owes the `Pending` -> `InService`
/// transition. Same shape as [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the
/// notebook instance straight after creating it still observes `Pending`, one that waits
/// (as `aws sagemaker wait notebook-instance-in-service` does) observes `InService`.
/// Real SageMaker takes minutes; the span is compressed, never zero.
pub const NOTEBOOK_INSTANCE_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// See [`MODEL_QUOTA`]: the same emulator-owned bound, for the notebook-instance store.
pub const NOTEBOOK_INSTANCE_QUOTA: usize = 1_000;

/// The `NotebookInstanceStatus` member a fresh instance carries. The modelled enum also
/// carries the update, delete and maintenance states, which this slice does not
/// reach; `InService` is spelled the same as an endpoint's, so [`STATUS_IN_SERVICE`]
/// stands for both.
pub const NOTEBOOK_INSTANCE_STATUS_PENDING: &str = "Pending";

/// The `NotebookInstanceStatus` an instance carries while `StopNotebookInstance` is
/// terminating its ML compute instance, from the modelled `NotebookInstanceStatus` enum
/// (`smithy_query shape=NotebookInstanceStatus`).
pub const NOTEBOOK_INSTANCE_STATUS_STOPPING: &str = "Stopping";

/// The `NotebookInstanceStatus` an instance carries once its ML compute instance is
/// terminated, from the modelled `NotebookInstanceStatus` enum.
pub const NOTEBOOK_INSTANCE_STATUS_STOPPED: &str = "Stopped";

/// How long after `StopNotebookInstance` the clock owes the `Stopping` -> `Stopped`
/// transition. Same contract as [`NOTEBOOK_INSTANCE_IN_SERVICE_AFTER`]: a caller that
/// describes the instance straight after stopping it still observes `Stopping`, one that
/// waits (as `aws sagemaker wait notebook-instance-stopped` does) observes `Stopped`.
/// Real SageMaker takes minutes to detach the ML storage volume and terminate the
/// compute instance; the span is this emulator's own compressed choice, never zero, and
/// the due instant is stored rather than slept on.
pub const NOTEBOOK_INSTANCE_STOPPED_AFTER: Duration = Duration::from_millis(300);

/// The statuses a `StopNotebookInstance` transitions an instance OUT of: the ones where
/// an ML compute instance exists to terminate, because it is serving (`InService`) or
/// coming up (`Pending`). From every other `NotebookInstanceStatus` there is nothing to
/// stop, so the call is refused rather than moving the status.
pub const NOTEBOOK_INSTANCE_STOPPABLE_STATUSES: [&str; 2] =
    [NOTEBOOK_INSTANCE_STATUS_PENDING, STATUS_IN_SERVICE];

/// The IMDS version a notebook instance reports when the create named none:
/// `MinimumInstanceMetadataServiceVersion`'s doc says that "when passed as part of
/// `CreateNotebookInstance`, if no value is selected, then it defaults to IMDSv1".
pub const DEFAULT_IMDS_VERSION: &str = "1";

/// `DirectInternetAccess`'s default. Its `CreateNotebookInstance` doc points at
/// "Notebook Instances Are Internet-Enabled by Default" and allows `Disabled` only
/// alongside a `SubnetId`.
pub const DEFAULT_DIRECT_INTERNET_ACCESS: &str = "Enabled";

/// `RootAccess`'s default: its `CreateNotebookInstance` doc says "the default value is
/// Enabled".
pub const DEFAULT_ROOT_ACCESS: &str = "Enabled";

/// `IpAddressType`'s default. AWS's `CreateNotebookInstance` API reference documents the
/// member as defaulting to IPv4-only connectivity when the request names no value.
pub const DEFAULT_IP_ADDRESS_TYPE: &str = "ipv4";

/// `PlatformIdentifier`'s default: its `CreateNotebookInstance` doc says "the default
/// value is notebook-al2023-v1".
pub const DEFAULT_PLATFORM_IDENTIFIER: &str = "notebook-al2023-v1";

/// `VolumeSizeInGB`'s default: its `CreateNotebookInstance` doc says "the default value
/// is 5 GB".
pub const DEFAULT_VOLUME_SIZE_IN_GB: i32 = 5;

/// How long the presigned URL `CreatePresignedNotebookInstanceUrl` answers stays valid.
///
/// A DOCUMENTED duration, so it keeps its published value rather than being compressed:
/// the operation's own doc says "the URL that you get from a call to
/// CreatePresignedNotebookInstanceUrl is valid only for 5 minutes".
pub const PRESIGNED_NOTEBOOK_URL_TTL: Duration = Duration::from_secs(5 * 60);

/// The session a presigned notebook-instance URL opens when the request names no
/// `SessionExpirationDurationInSeconds`.
///
/// Also DOCUMENTED, by the member itself: "The duration of the session, in seconds. The
/// default is 12 hours." — which is the top of its modelled `range 1800..43200`.
pub const DEFAULT_NOTEBOOK_SESSION_DURATION_SECONDS: i32 = 12 * 60 * 60;

/// The resource kind a notebook instance's ARN carries, as `NotebookInstanceArn`'s
/// documented `arn:aws:sagemaker:<region>:<account>:notebook-instance/<name>` form
/// spells it. Named once so the create and the reader agree; both pass it to
/// [`resource_arn`].
pub const NOTEBOOK_INSTANCE_ARN_KIND: &str = "notebook-instance";

/// The Jupyter host a notebook instance serves on:
/// `<name>.notebook.<region>.sagemaker.aws`, the host `DescribeNotebookInstance`'s
/// `Url` carries and `CreatePresignedNotebookInstanceUrl` signs against.
///
/// Derived from the instance name and the region the caller signed for, so it is stable
/// across reads and distinct per instance.
pub fn notebook_instance_url(region: &str, name: &str) -> String {
    format!("{name}.notebook.{region}.sagemaker.aws")
}

/// The presigned URL `CreatePresignedNotebookInstanceUrl` answers: the instance's own
/// Jupyter host (the `Url` [`notebook_instance_url`] minted and
/// `DescribeNotebookInstance` reports) with the grant this call recorded on the instance
/// carried as the `authToken` query parameter — the same shape the presigned partner-app
/// URL takes, so one scheme covers both.
pub fn presigned_notebook_instance_url(host: &str, token: &str) -> String {
    format!("https://{}/auth?authToken={token}", host.trim_end_matches('/'))
}

/// The id of the network interface `CreateNotebookInstance` reports it created:
/// `eni-` plus 17 lowercase hex digits, the form EC2 network-interface ids take.
///
/// The digits come from this module's one id mint, [`mint_opaque_resource_id`], so an ENI
/// id is server-generated and derived from nothing in the request, with the same
/// emulator-grade, non-cryptographic caveats that mint documents: it identifies a record
/// in this store and is never a secret. `CreateNotebookInstance` mints it ONCE and stores
/// it, so every later read of the same instance reports the same id.
pub fn mint_network_interface_id() -> String {
    let digits = mint_opaque_resource_id();
    // The mint renders a 32-digit hex id, but the slice is taken defensively so this
    // helper does not depend on that width: a shorter id is used whole.
    format!("eni-{}", digits.get(..17).unwrap_or(&digits))
}

/// A notebook instance created by `CreateNotebookInstance`, as
/// `DescribeNotebookInstance` replays it.
///
/// Every member the create carried is kept as given, so the reader replays the request
/// rather than re-deriving it; the members the create left out are kept at the default
/// the model documents for them (the `DEFAULT_*` constants above). `in_service_at` is the
/// instant the clock owes the `Pending` -> `InService` transition; it is stored, never
/// slept on, and materialised by whichever reporting call first runs at or after it.
#[derive(Debug, Clone)]
pub struct NotebookInstance {
    pub name: String,
    pub arn: String,
    pub status: String,
    /// Why the instance failed, for the `Failed` status. No implemented operation moves
    /// an instance there yet (the start and update operations are still stubs), so
    /// it is stored `None` and reported absent.
    pub failure_reason: Option<String>,
    /// The Jupyter host the instance serves on, minted at create from the name and the
    /// signed region (see [`notebook_instance_url`]).
    pub url: String,
    pub instance_type: String,
    pub ip_address_type: String,
    pub subnet_id: Option<String>,
    pub security_groups: Vec<String>,
    pub role_arn: String,
    pub kms_key_id: Option<String>,
    /// The id of the network interface the create minted for the instance.
    pub network_interface_id: String,
    pub lifecycle_config_name: Option<String>,
    pub direct_internet_access: String,
    pub volume_size_in_gb: i32,
    pub accelerator_types: Vec<String>,
    pub default_code_repository: Option<String>,
    pub additional_code_repositories: Vec<String>,
    pub root_access: String,
    pub platform_identifier: String,
    pub instance_metadata_service_configuration:
        crate::ops::types::InstanceMetadataServiceConfiguration,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub in_service_at: SystemTime,
    /// The instant the clock owes the `Stopping` -> `Stopped` transition, set by
    /// `StopNotebookInstance` and cleared once the transition is materialised. `None`
    /// while no stop is in flight.
    pub stopped_at: Option<SystemTime>,
    /// Whether the instance ever finished provisioning, i.e. the `Pending` ->
    /// `InService` transition was materialised. Kept beside the status rather than
    /// derived from it, because a stop that lands while the instance is still `Pending`
    /// leaves it `Stopping` and then `Stopped` without it ever having come up — and an
    /// instance that never came up has no Jupyter endpoint and no network interface to
    /// report (see [`NotebookInstance::is_provisioned`]).
    pub provisioned: bool,
    /// Every presigned-URL grant `CreatePresignedNotebookInstanceUrl` minted for this
    /// instance whose session could still be in use, oldest first. Same record as a
    /// partner app's, [`PresignedUrlGrant`], because the two grants carry the same thing.
    pub url_grants: Vec<PresignedUrlGrant>,
}

impl NotebookInstance {
    /// Materialise whichever status transition the clock already owes, the way
    /// [`Endpoint::settle_status`] does for an endpoint: `Pending` -> `InService` after a
    /// create, `Stopping` -> `Stopped` after a stop. Each was due at its stored instant
    /// whether or not anybody called, and `LastModifiedTime` moves to the instant it was
    /// due rather than to "now", so the value is stable across later reads.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == NOTEBOOK_INSTANCE_STATUS_PENDING && now >= self.in_service_at {
            self.status = STATUS_IN_SERVICE.to_string();
            self.last_modified_time = self.in_service_at;
            self.provisioned = true;
            return true;
        }
        // The stop's own due instant, kept beside the create's so a stop that arrives
        // while the instance is still `Pending` does not later snap back to `InService`.
        let stop_due = match self.stopped_at {
            Some(due) if self.status == NOTEBOOK_INSTANCE_STATUS_STOPPING && now >= due => {
                Some(due)
            }
            _ => None,
        };
        if let Some(due) = stop_due {
            self.status = NOTEBOOK_INSTANCE_STATUS_STOPPED.to_string();
            self.last_modified_time = due;
            self.stopped_at = None;
            return true;
        }
        false
    }

    /// Whether `StopNotebookInstance` has an ML compute instance to terminate, i.e. the
    /// status is one of [`NOTEBOOK_INSTANCE_STOPPABLE_STATUSES`].
    pub fn is_stoppable(&self) -> bool {
        NOTEBOOK_INSTANCE_STOPPABLE_STATUSES.contains(&self.status.as_str())
    }

    /// Start terminating the ML compute instance: the status moves to `Stopping` now and
    /// the clock owes `Stopped` at [`NOTEBOOK_INSTANCE_STOPPED_AFTER`] from `now`, which
    /// [`NotebookInstance::settle_status`] materialises once it comes due.
    ///
    /// Every stored member is left as it stands — SageMaker preserves the ML storage
    /// volume across a stop — so a later start replays the same configuration.
    pub fn begin_stop(&mut self, now: SystemTime) {
        self.status = NOTEBOOK_INSTANCE_STATUS_STOPPING.to_string();
        self.last_modified_time = now;
        self.stopped_at = Some(now + NOTEBOOK_INSTANCE_STOPPED_AFTER);
    }

    /// Record a presigned-URL grant against this instance and answer it: the opaque token
    /// the URL carries, the [`PRESIGNED_NOTEBOOK_URL_TTL`] the URL itself is good for, and
    /// the session `session_duration_seconds` opens once the URL is followed.
    ///
    /// A grant whose session the clock has already ended is released first — it was over
    /// at its stored instant whether or not anybody called — and the history is bounded by
    /// [`GRANT_HISTORY_CAP`], exactly as a partner app's grants are, so a caller asking
    /// for URL after URL cannot grow the store without end.
    pub fn grant_presigned_url(
        &mut self,
        now: SystemTime,
        session_duration_seconds: i32,
    ) -> PresignedUrlGrant {
        let grant = PresignedUrlGrant {
            token: mint_grant_token(),
            issued_at: now,
            expires_at: now + PRESIGNED_NOTEBOOK_URL_TTL,
            session_expires_at: now
                + Duration::from_secs(u64::try_from(session_duration_seconds).unwrap_or_default()),
        };
        self.url_grants.retain(|held| held.session_expires_at > now);
        self.url_grants.push(grant.clone());
        if self.url_grants.len() > GRANT_HISTORY_CAP {
            let overflow = self.url_grants.len() - GRANT_HISTORY_CAP;
            self.url_grants.drain(..overflow);
        }
        grant
    }

    /// Whether the instance has finished provisioning, i.e. the `Pending` ->
    /// `InService` transition the create owed has been materialised.
    ///
    /// An instance that has not come up has no Jupyter endpoint and no network interface
    /// to report, so `DescribeNotebookInstance` omits `Url` and `NetworkInterfaceId`
    /// until it is provisioned — including for one stopped before it ever came up.
    pub fn is_provisioned(&self) -> bool {
        self.provisioned
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another notebook instance fits, i.e. the store is below
    /// [`NOTEBOOK_INSTANCE_QUOTA`].
    pub fn notebook_instances_below_quota(&self) -> bool {
        self.notebook_instances.len() < NOTEBOOK_INSTANCE_QUOTA
    }
}

// ---------------------------------------------------------------------------
// Model cards. `CreateModelCard` stores one here and `DescribeModelCard` replays it;
// the store itself is `ComAmazonawsSagemakerSagemakerState::model_cards` above, and the
// export jobs `CreateModelCardExportJob` starts against a card sit beside it in
// `model_card_export_jobs`. Kept together at the end of the file so a model-card change
// touches one span of it.
// ---------------------------------------------------------------------------

/// The version number a model card's FIRST version carries. `DescribeModelCard`'s
/// `ModelCardVersion` is modelled as an `Integer` and the API's version history starts
/// at one, so `CreateModelCard` stores version 1 and an `UpdateModelCard` that carries
/// new `Content` appends the next integer ([`ModelCard::next_version`]).
pub const FIRST_MODEL_CARD_VERSION: i32 = 1;

/// One version of a model card, as `CreateModelCard` stores the first one and
/// `DescribeModelCard` replays it.
///
/// A model card is versioned: `Content`, `ModelCardStatus` and `SecurityConfig` belong
/// to a version, while the name, the ARN and the tags belong to the card.
///
/// `created_by` is stored when the version is created rather than rebuilt per read, so
/// every later read reports the identity that actually created it; `last_modified_by`
/// stays absent until a call modifies the version.
#[derive(Debug, Clone)]
pub struct ModelCardVersion {
    pub version: i32,
    pub content: String,
    pub status: String,
    pub security_config: Option<ModelCardSecurityConfig>,
    pub creation_time: SystemTime,
    pub created_by: UserContext,
    pub last_modified_time: SystemTime,
    pub last_modified_by: Option<UserContext>,
}

/// A model card created by `CreateModelCard`, with every version it has had.
///
/// `versions` is ordered by `version` ascending and is never empty: a card exists only
/// once its first version does.
#[derive(Debug, Clone)]
pub struct ModelCard {
    pub name: String,
    pub arn: String,
    pub tags: Vec<Tag>,
    pub versions: Vec<ModelCardVersion>,
}

impl ModelCard {
    /// A card holding exactly `first`, the version `CreateModelCard` minted.
    pub fn new(name: String, arn: String, tags: Vec<Tag>, first: ModelCardVersion) -> Self {
        Self {
            name,
            arn,
            tags,
            versions: vec![first],
        }
    }

    /// The highest-numbered version — what a reader that names no version describes.
    pub fn latest_version(&self) -> Option<&ModelCardVersion> {
        self.versions.last()
    }

    /// The version numbered `version`, or `None` when the card never had it.
    pub fn version(&self, version: i32) -> Option<&ModelCardVersion> {
        self.versions.iter().find(|held| held.version == version)
    }

    /// The highest-numbered version, mutably — the one version an in-place change (a
    /// status update) applies to; the versions before it are history and stay as they
    /// were recorded.
    pub fn latest_version_mut(&mut self) -> Option<&mut ModelCardVersion> {
        self.versions.last_mut()
    }

    /// The number the NEXT version of this card carries: one past the highest it holds,
    /// so a card's version history is the consecutive run starting at
    /// [`FIRST_MODEL_CARD_VERSION`].
    pub fn next_version(&self) -> i32 {
        self.versions
            .last()
            .map_or(FIRST_MODEL_CARD_VERSION, |held| {
                held.version.saturating_add(1)
            })
    }

    /// Whether this card can gain another version. See [`MODEL_CARD_VERSION_QUOTA`].
    pub fn versions_below_quota(&self) -> bool {
        self.versions.len() < MODEL_CARD_VERSION_QUOTA
    }
}

/// Whether `document` is a JSON object.
///
/// The one structural claim this service makes about a caller-supplied JSON document
/// whose model bounds only its length — a model card's `Content`
/// ([`is_model_card_content`]), a model package group's `ResourcePolicy` — so the
/// boundary validator passes such a value and the service is what refuses it. Stated
/// once here rather than once per operation.
pub fn is_json_object(document: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(document).is_ok_and(|parsed| parsed.is_object())
}

/// Whether `content` can be a model card's `Content`.
///
/// `Content`'s own documentation says it "must be in model card JSON schema and provided
/// as a string", so a value that is not even a JSON object is not a model card. The
/// model constrains only the member's length, so the boundary validator passes such a
/// value and the service refuses it. The schema's own required members are NOT checked
/// here: this is the one structural claim the model's documentation makes.
pub fn is_model_card_content(content: &str) -> bool {
    is_json_object(content)
}

/// `ModelCardExportJobStatus`'s members this emulator moves an export job through (the
/// modelled enum carries exactly `InProgress`, `Completed` and `Failed`). Named once
/// here so no handler compares a bare string literal.
pub const MODEL_CARD_EXPORT_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`MODEL_CARD_EXPORT_STATUS_IN_PROGRESS`].
pub const MODEL_CARD_EXPORT_STATUS_FAILED: &str = "Failed";

/// How long after `CreateModelCardExportJob` the clock owes the job's terminal status.
///
/// Same contract as [`TRANSFORM_JOB_SETTLES_AFTER`]: a caller that describes the job
/// straight after creating it still observes `InProgress`, one that waits observes the
/// terminal status, so a waiter terminates. Real SageMaker takes as long as rendering
/// and uploading the document takes; the span is compressed, never zero, and the due
/// instant is stored rather than slept on.
pub const MODEL_CARD_EXPORT_JOB_SETTLES_AFTER: Duration = Duration::from_millis(300);

/// Why a model card export job this emulator accepted ends in the modelled `Failed`
/// status.
///
/// The job renders the card's version as the model card PDF document SageMaker
/// publishes and writes it to the request's `OutputConfig$S3OutputPath`. That renderer
/// is AWS's own and this emulator does not have it, and the engine contract
/// (`vendor/engine/src/lib.rs`) forbids reporting a status or an artifact for work
/// nothing performed — so reporting `Completed` with `ExportArtifacts` naming an object
/// nothing wrote would claim an export that never happened. The job therefore ends in
/// the status the model carries for an export that produced no document, with this
/// reason saying why, and no `ExportArtifacts` beside it. Everything around the render —
/// resolving the card and its version, accepting the job, minting its ARN, replaying it,
/// reporting its terminal status — is real.
pub const MODEL_CARD_EXPORT_NOT_RUN_FAILURE: &str =
    "This emulator renders no model card PDF, so the model card was not exported and no \
     artifact was written to the job's S3OutputPath.";

/// The store key one model card export job is held under: the card it exports and the
/// job's own name.
///
/// A job's ARN is `<card ARN>/export-job/<job name>`, so the job's name is unique within
/// the card rather than within the account — two cards may each have an export job of
/// the same name, and they are two jobs.
pub fn model_card_export_job_key(card_name: &str, job_name: &str) -> String {
    format!("{card_name}/{job_name}")
}

/// The store key the export-job ARN `arn` addresses, or `None` when the value does not
/// name an export job of a model card of this service, in this account and in `region`.
///
/// `DescribeModelCardExportJob` resolves a job by ARN alone, and the store is keyed by
/// the card and job names the ARN carries — so the ARN is parsed and its fixed segments
/// checked the way [`entity_name_from_name_or_arn`] checks an entity's, before the
/// resource portion `model-card/<card>/export-job/<job>` is reduced to that key. An ARN
/// naming another partition, service, Region, account or resource shape names no job
/// here, and a name segment carrying a further `/` is refused rather than folded into the
/// key, so two different ARNs can never address one record.
pub fn model_card_export_job_key_from_arn(arn: &str, region: &str) -> Option<String> {
    let (card_name, job_name) = arn_resource(arn, region)?
        .strip_prefix("model-card/")?
        .split_once("/export-job/")?;
    (!card_name.is_empty()
        && !card_name.contains('/')
        && !job_name.is_empty()
        && !job_name.contains('/'))
        .then(|| model_card_export_job_key(card_name, job_name))
}

/// A model card export job created by `CreateModelCardExportJob`, as
/// `DescribeModelCardExportJob` replays it.
///
/// `card_version` is the version the create RESOLVED (the requested one, or the card's
/// latest when the request named none), so a later read reports the version that was
/// exported rather than whichever is latest by then. `ends_at` is the instant the clock
/// owes the move out of `InProgress`; it is stored, never slept on, and materialised by
/// whichever reporting call first runs at or after it ([`ModelCardExportJob::settle_status`]).
#[derive(Debug, Clone)]
pub struct ModelCardExportJob {
    pub name: String,
    pub arn: String,
    pub card_name: String,
    pub card_version: i32,
    pub status: String,
    pub output_config: ModelCardExportOutputConfig,
    pub failure_reason: Option<String>,
    pub created_at: SystemTime,
    pub last_modified_at: SystemTime,
    pub ends_at: SystemTime,
}

impl ModelCardExportJob {
    /// Materialise the move out of `InProgress` the clock already owes.
    ///
    /// The one write a reporting operation may make: the move was due at `ends_at`
    /// whether or not anybody described the job, so the observer makes no difference to
    /// the result, and `LastModifiedAt` takes the instant the move was due rather than
    /// "now", so the value is stable across later reads.
    ///
    /// The terminal status is [`MODEL_CARD_EXPORT_STATUS_FAILED`] with
    /// [`MODEL_CARD_EXPORT_NOT_RUN_FAILURE`]; see that constant for why an export this
    /// emulator accepted cannot end `Completed`.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == MODEL_CARD_EXPORT_STATUS_IN_PROGRESS && now >= self.ends_at {
            self.status = MODEL_CARD_EXPORT_STATUS_FAILED.to_string();
            self.failure_reason = Some(MODEL_CARD_EXPORT_NOT_RUN_FAILURE.to_string());
            self.last_modified_at = self.ends_at;
            return true;
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Lineage actions (`CreateAction` / `DescribeAction` / `DeleteAction`).
//
// Kept together at the end of the file, rather than interleaved with the stores
// above, so this slice is one block to read — and one block to merge.
// ---------------------------------------------------------------------------

/// See [`MODEL_QUOTA`]. Bounds the lineage actions `CreateAction` registers; every
/// record it holds is released by `DeleteAction`.
pub const ACTION_QUOTA: usize = 1_000;

/// A lineage action created by `CreateAction`, as `DescribeAction` replays it and
/// `DeleteAction` releases it.
///
/// Every optional member is held as an `Option` rather than flattened to a default, so
/// a reader can tell "the caller sent nothing" from "the caller sent an empty value";
/// `properties`, which is not optional but the map the action HOLDS (see this module's
/// note on an entity's own collections), is a `BTreeMap` so it serialises in one stable
/// order whatever order the request arrived in.
#[derive(Debug, Clone)]
pub struct Action {
    pub name: String,
    pub arn: String,
    /// The Region the ARN names, i.e. the one the creating request signed for. Held
    /// separately so the lineage group reported for this action does not have to be
    /// parsed back out of the ARN.
    pub region: String,
    pub source: crate::ops::types::ActionSource,
    pub action_type: String,
    pub description: Option<String>,
    pub status: Option<String>,
    pub properties: BTreeMap<String, String>,
    pub metadata_properties: Option<MetadataProperties>,
    /// What the create carried, so a later `ListTags`/`AddTags` reports it; no
    /// implemented operation surfaces it yet (`DescribeAction` models no `Tags` member).
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another lineage action fits, i.e. the store is below [`ACTION_QUOTA`].
    pub fn actions_below_quota(&self) -> bool {
        self.actions.len() < ACTION_QUOTA
    }

    /// The key [`ComAmazonawsSagemakerSagemakerState::actions`] holds an action under,
    /// for an identifier the caller gave as either a name or an ARN (the lineage
    /// readers' `ExperimentEntityNameOrArn`). See [`arn_from_name_or_arn`], which the
    /// context slice resolves its own key with too.
    pub fn action_key(region: &str, identifier: &str) -> String {
        arn_from_name_or_arn(region, ACTION_KIND, identifier)
    }
}

// ---------------------------------------------------------------------------
// The AI-benchmarking workload-configuration slice: `CreateAIWorkloadConfig`,
// `DescribeAIWorkloadConfig` and the `DeleteAIWorkloadConfig` that releases the record.
// Kept together at the end of this module, rather than spread through the blocks above,
// so this one resource kind reads as one slice.
// ---------------------------------------------------------------------------

/// The ARN resource kind an AI workload configuration carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `AIWorkloadConfigArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:ai-workload-config/...`), and named
/// once here so the create and the describe mint and match the same ARN.
pub const AI_WORKLOAD_CONFIG_KIND: &str = "ai-workload-config";

/// See [`MODEL_QUOTA`]. Bounds the AI workload configuration store the same way, and is
/// what `CreateAIWorkloadConfig` answers `ResourceLimitExceeded` past.
pub const AI_WORKLOAD_CONFIG_QUOTA: usize = 1_000;

/// Why a reader could not resolve an AI workload configuration, named once here so every
/// operation that READS one refuses an unknown name with the same sentence: the family's
/// [`ai_entity_not_found`], quoting the identifier the request presented. The delete
/// relays the inner service's rendering of it instead
/// ([`ai_entity_not_found_relayed`]).
pub fn ai_workload_config_not_found(identifier: &str) -> String {
    ai_entity_not_found(AI_WORKLOAD_CONFIG_ENTITY, identifier)
}

/// Why a create that references an AI workload configuration could not resolve it: the
/// family's [`ai_entity_not_found_for_account`], quoting the
/// `AIWorkloadConfigIdentifier` the request presented.
///
/// Named once here so `CreateAIBenchmarkJob` and `CreateAIRecommendationJob` refuse an
/// unresolvable reference with the same sentence.
pub fn ai_workload_config_not_found_for_account(identifier: &str) -> String {
    ai_entity_not_found_for_account(AI_WORKLOAD_CONFIG_ENTITY, identifier)
}

/// An AI workload configuration created by `CreateAIWorkloadConfig`, as
/// `DescribeAIWorkloadConfig` replays it.
///
/// A workload configuration is inert data — a dataset configuration plus a benchmark
/// tool specification — so it carries no status: the service returns it ready, and
/// there is no transition for the clock to owe.
#[derive(Debug, Clone)]
pub struct AIWorkloadConfig {
    pub name: String,
    pub arn: String,
    pub dataset_config: Option<crate::ops::types::AIDatasetConfig>,
    pub workload_configs: Option<crate::ops::types::AIWorkloadConfigs>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another AI workload configuration fits, i.e. the store is below
    /// [`AI_WORKLOAD_CONFIG_QUOTA`]. A second inherent `impl` block, beside this slice's
    /// own types rather than in the one above.
    pub fn ai_workload_configs_below_quota(&self) -> bool {
        self.ai_workload_configs.len() < AI_WORKLOAD_CONFIG_QUOTA
    }

    /// Whether another AI benchmark job fits, i.e. the store is below
    /// [`AI_BENCHMARK_JOB_QUOTA`].
    pub fn ai_benchmark_jobs_below_quota(&self) -> bool {
        self.ai_benchmark_jobs.len() < AI_BENCHMARK_JOB_QUOTA
    }
}

// ---------------------------------------------------------------------------
// The AI benchmark job slice: what `CreateAIBenchmarkJob` writes and
// `DescribeAIBenchmarkJob` replays, beside the workload-configuration slice the jobs
// reference.
// ---------------------------------------------------------------------------

/// The ARN resource kind an AI benchmark job carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `AIBenchmarkJobArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:ai-benchmark-job/...`), and named
/// once here so the create and the describe mint and match the same ARN.
pub const AI_BENCHMARK_JOB_KIND: &str = "ai-benchmark-job";

/// See [`MODEL_QUOTA`]. Bounds the AI benchmark job store the same way, and is what
/// `CreateAIBenchmarkJob` answers `ResourceLimitExceeded` past.
pub const AI_BENCHMARK_JOB_QUOTA: usize = 1_000;

/// Why a reader could not resolve an AI benchmark job: the family's
/// [`ai_entity_not_found`], quoting the name the request presented. Named once here so
/// every operation that resolves a benchmark-job reference words the same fact the same
/// way.
pub fn ai_benchmark_job_not_found(name: &str) -> String {
    ai_entity_not_found(AI_BENCHMARK_JOB_ENTITY, name)
}

/// `AIBenchmarkJobStatus`'s members this emulator moves a benchmark job through
/// (`smithy_query(query="shape",shape="AIBenchmarkJobStatus")` answers `InProgress`,
/// `Completed`, `Failed`, `Stopping` and `Stopped`; the `Completed` and stop states are
/// not reached by creating one). Named once here so no handler compares a bare string
/// literal.
pub const AI_BENCHMARK_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`AI_BENCHMARK_STATUS_IN_PROGRESS`].
pub const AI_BENCHMARK_STATUS_FAILED: &str = "Failed";

/// How long after `CreateAIBenchmarkJob` the clock owes the job's move out of
/// `InProgress` into its terminal status.
///
/// A span chosen by this emulator (AWS documents no duration for a benchmark run),
/// inside the 250–750 ms band the project fixes for a transition the clock owes. It is
/// never zero, so a caller that describes the job straight after creating it still
/// observes `InProgress`, and one that waits observes the terminal status, so a waiter
/// terminates.
pub const AI_BENCHMARK_JOB_ENDS_AFTER: Duration = Duration::from_millis(400);

/// `FailureReason` of a benchmark job whose run nothing could perform here.
///
/// A benchmark job drives real traffic at a live inference endpoint and measures its
/// latency, throughput and cost. This emulator serves no inference data plane and runs
/// no benchmark tool, and the engine contract (`vendor/engine/src/lib.rs`) forbids
/// reporting a status, duration or measurement for work nothing ran — so the run the
/// caller asked for ends in the modelled `Failed` state with this reason, and no
/// `StartTime`, `Completed` status or benchmark result is invented. Everything around
/// the run — accepting the job, resolving its workload configuration, minting its ARN,
/// replaying it, reporting its terminal status — is real.
pub const NO_BENCHMARK_RUNTIME_REASON: &str =
    "This emulator serves no inference data plane and runs no benchmark tool, so the \
     benchmark job's run was never started";

/// An AI benchmark job created by `CreateAIBenchmarkJob`, as `DescribeAIBenchmarkJob`
/// replays it.
///
/// `ends_at` is the instant the clock owes the move out of `InProgress`; it is stored,
/// never slept on, and materialised by whichever reporting call first runs at or after
/// it (see [`AIBenchmarkJob::settle_status`]).
#[derive(Debug, Clone)]
pub struct AIBenchmarkJob {
    pub name: String,
    pub arn: String,
    /// The `AIWorkloadConfigIdentifier` the request presented, kept VERBATIM (a bare
    /// name or the configuration's ARN) so the read replays what the caller sent rather
    /// than a form it did not use.
    pub workload_config_identifier: String,
    pub benchmark_target: crate::ops::types::AIBenchmarkTarget,
    /// What the request's `OutputConfig` carried. `DescribeAIBenchmarkJob` reports it as
    /// an `AIBenchmarkOutputResult`, which adds the log streams of a run — and nothing
    /// ran here, so that member stays absent.
    pub output_config: crate::ops::types::AIBenchmarkOutputConfig,
    pub network_config: Option<crate::ops::types::AIBenchmarkNetworkConfig>,
    pub role_arn: String,
    pub tags: Vec<Tag>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub ends_at: SystemTime,
}

impl AIBenchmarkJob {
    /// Materialise the move out of `InProgress` the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the move was due at
    /// `ends_at` whether or not anybody described the job, so the observer makes no
    /// difference to the result. `EndTime` takes the instant the move was due, not
    /// "now", so it is stable across later reads.
    ///
    /// The terminal status is [`AI_BENCHMARK_STATUS_FAILED`] with
    /// [`NO_BENCHMARK_RUNTIME_REASON`]: nothing benchmarked the target, so no
    /// `Completed` status and no `StartTime` is invented for it.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == AI_BENCHMARK_STATUS_IN_PROGRESS && now >= self.ends_at {
            self.status = AI_BENCHMARK_STATUS_FAILED.to_string();
            self.failure_reason = Some(NO_BENCHMARK_RUNTIME_REASON.to_string());
            // The instant the service gave up on the run, which is when the job ended.
            // No start time is recorded beside it: the run never started.
            self.end_time = Some(self.ends_at);
            return true;
        }
        false
    }
}

// ---------------------------------------------------------------------------
// The compute-allocation slice: the definitions `ListComputeQuotas` lists and
// `DescribeComputeQuota` replays. The pagination-token refusal those listings share
// with every other paginated listing here lives with the token vocabulary at the top
// of this module ([`STALE_PAGE_TOKEN`]).
//
// Kept together at the end of the file, rather than interleaved with the blocks
// above, so this slice is one block to read — and one block to merge.
// ---------------------------------------------------------------------------

/// How long after `CreateComputeQuota` the clock owes the `Creating` -> `Created`
/// transition, for the same reason [`ENDPOINT_IN_SERVICE_AFTER`] exists: a caller that
/// describes or lists the quota straight after creating it still observes `Creating`,
/// one that waits observes `Created`. Real SageMaker publishes no span for it; this one
/// is the emulator's own, compressed and never zero.
pub const COMPUTE_QUOTA_CREATED_AFTER: Duration = Duration::from_millis(300);

/// `SchedulerResourceStatus`'s members a compute quota reaches in this emulator: a
/// create is reported `Creating` until the clock owes `Created`. The modelled enum also
/// carries the update, rollback and delete states, which this slice does not reach.
/// Named separately from [`STATUS_CREATING`] (an `EndpointStatus` member that happens to
/// share the literal) so each handler compares against its own modelled enum.
pub const STATUS_QUOTA_CREATING: &str = "Creating";
/// See [`STATUS_QUOTA_CREATING`].
pub const STATUS_QUOTA_CREATED: &str = "Created";

/// `ActivationState`'s documented default ("Use to enable or disable compute
/// allocation. Default is `Enabled`.").
pub const DEFAULT_ACTIVATION_STATE: &str = "Enabled";

/// The version `CreateComputeQuota` stamps on a new compute allocation definition;
/// `UpdateComputeQuota` is the only operation that moves it, so every quota this slice
/// holds carries this one.
pub const FIRST_COMPUTE_QUOTA_VERSION: i32 = 1;

/// The length `ComputeQuotaId`'s modelled pattern `^[a-z0-9]{12}$` fixes.
const COMPUTE_QUOTA_ID_LEN: usize = 12;

/// How many draws [`ComAmazonawsSagemakerSagemakerState::mint_compute_quota_id`] takes
/// from [`mint_opaque_resource_id`] before falling back. One suffices unless its cut
/// collides with a stored id; the bound is what keeps the minting terminating.
const COMPUTE_QUOTA_ID_DRAWS: u32 = 16;

/// A compute allocation definition created by `CreateComputeQuota`, as
/// `DescribeComputeQuota` replays it and `ListComputeQuotas` summarises it.
///
/// `created_status_at` is the instant the clock owes the `Creating` -> `Created`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it (see [`ComputeQuota::settle_status`]).
///
/// The two configuration shapes are named by full path rather than through the module's
/// shared import list, the way the slices above do it: that list is the hottest line in
/// this file, and a slice that stays out of it stays out of its merge conflicts.
#[derive(Debug, Clone)]
pub struct ComputeQuota {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub version: i32,
    pub status: String,
    pub failure_reason: Option<String>,
    pub cluster_arn: String,
    pub config: crate::ops::types::ComputeQuotaConfig,
    pub target: crate::ops::types::ComputeQuotaTarget,
    pub activation_state: String,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_status_at: SystemTime,
}

impl ComputeQuota {
    /// Materialise the `Creating` -> `Created` transition the clock already owes, the
    /// way [`Endpoint::settle_status`] does: it was due at `created_status_at` whether
    /// or not anybody read the quota, so the observer makes no difference to the
    /// result, and `LastModifiedTime` moves to the instant it was due rather than to
    /// "now".
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == STATUS_QUOTA_CREATING && now >= self.created_status_at {
            self.status = STATUS_QUOTA_CREATED.to_string();
            self.last_modified_time = self.created_status_at;
            return true;
        }
        false
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// A `ComputeQuotaId` no record in this store holds. A second inherent `impl` block,
    /// beside this slice's own types rather than in the one above.
    ///
    /// The id is the server's, not derived from the request: `CreateComputeQuota` twice
    /// with the same input in two clusters mints two distinct quotas. It comes from this
    /// module's ONE id mint, [`mint_opaque_resource_id`] (32 lowercase hex digits), cut
    /// to the 12 characters `ComputeQuotaId`'s `^[a-z0-9]{12}$` fixes — hex is a subset
    /// of that class, so the id satisfies the pattern by construction — the way
    /// [`mint_cluster_id`] derives its own. The loop re-draws on the vanishing
    /// chance that the cut collides with a stored id, and is bounded: past
    /// [`COMPUTE_QUOTA_ID_DRAWS`] draws the fallback walks the 12-digit lowercase-hex
    /// numbers until one is free, which terminates because the store is finite.
    ///
    /// `CreateComputeQuota` is still a scaffold stub (its own issue owns it, together
    /// with the `ClusterArn`-existence precondition SageMaker applies before it stores
    /// anything), so the minting seam is exercised today by this crate's compute-quota
    /// tests, which seed the store the create will write.
    pub fn mint_compute_quota_id(&self) -> String {
        for _ in 0..COMPUTE_QUOTA_ID_DRAWS {
            let minted = mint_opaque_resource_id();
            // The mint yields 32 hex digits, so the cut is always `COMPUTE_QUOTA_ID_LEN`
            // long; only a collision with a stored id makes this draw unusable.
            let id: String = minted.chars().take(COMPUTE_QUOTA_ID_LEN).collect();
            if !self.compute_quotas.contains_key(&id) {
                return id;
            }
        }
        (0..=u64::try_from(self.compute_quotas.len()).unwrap_or(u64::MAX))
            .map(|sequence| format!("{sequence:012x}"))
            .find(|id| !self.compute_quotas.contains_key(id))
            .unwrap_or_else(|| "0".repeat(COMPUTE_QUOTA_ID_LEN))
    }
}

// ---------------------------------------------------------------------------
// The lineage-context slice: `CreateContext`, `DescribeContext`, `UpdateContext` and
// `DeleteContext`. Kept together at the end of this module, rather than spread through
// the blocks above, so this one resource kind reads as one slice (the same shape the
// AI-benchmarking slice above uses).
// ---------------------------------------------------------------------------

/// The ARN resource kind a lineage context carries, i.e. the `<kind>` in [`resource_arn`].
/// Fixed by `ContextArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:context/`), and named once here so
/// the create, the reader, the update and the delete mint and match the same ARN.
pub const CONTEXT_KIND: &str = "context";

/// See [`MODEL_QUOTA`]. Bounds the lineage-context store the same way, and is what
/// `CreateContext` answers `ResourceLimitExceeded` past; every record it holds is released
/// by `DeleteContext`.
pub const CONTEXT_QUOTA: usize = 1_000;

/// A lineage context created by `CreateContext`, as `DescribeContext` replays it and
/// `UpdateContext` edits it.
///
/// `properties` is the map the context HOLDS, empty while nothing has put an entry in it
/// (see this module's note on an entity's own collections). It is a `BTreeMap` so the map
/// serialises in one stable order whatever order the request arrived in. `created_by` /
/// `last_modified_by` are stored by the call that set them rather than rebuilt per read,
/// so every later read reports the identity that actually created or last edited the
/// context.
#[derive(Debug, Clone)]
pub struct Context {
    pub name: String,
    pub arn: String,
    /// The Region the ARN names, i.e. the one the creating request signed for. Held
    /// separately so a read does not have to parse the ARN to report the Region's lineage
    /// group.
    pub region: String,
    pub source: crate::ops::types::ContextSource,
    pub context_type: String,
    pub description: Option<String>,
    pub properties: BTreeMap<String, String>,
    pub created_by: UserContext,
    pub last_modified_by: UserContext,
    /// Stored so a later `ListTags` reports what the create carried; `DescribeContext`
    /// models no `Tags` member, so this is the one reader that surfaces it.
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

// ---------------------------------------------------------------------------
// SageMaker Partner AI Apps: the store `ListPartnerApps` pages. Kept together at the
// end of this module, the way the lineage-action and workload-configuration slices
// above are, so this resource kind reads as one slice — and merges as one block.
// ---------------------------------------------------------------------------

/// `PartnerAppStatus`'s members this emulator moves a partner AI app through (both are
/// members of `crate::ops::types::PARTNER_APP_STATUS_VALUES`; the modelled enum also
/// carries the update, failure and delete states, which this slice does not reach).
/// A separate pair from [`STATUS_CREATING`], which names `EndpointStatus`'s members.
pub const PARTNER_APP_STATUS_CREATING: &str = "Creating";
/// See [`PARTNER_APP_STATUS_CREATING`].
pub const PARTNER_APP_STATUS_AVAILABLE: &str = "Available";

/// `ListPartnerApps`' documented `MaxResults` default, applied when the caller names
/// none: "The default value for MaxResults is 10."
/// (`smithy_query(query="doc",shape="ListPartnerAppsRequest",member="MaxResults")`).
/// [`MAX_PAGE_SIZE`] still caps the page above it, as it does for every listing here.
pub const PARTNER_APP_DEFAULT_PAGE_SIZE: i32 = 10;

/// See [`MODEL_QUOTA`]. Bounds the partner-app store, and is what `CreatePartnerApp`
/// answers `ResourceLimitExceeded` past.
pub const PARTNER_APP_QUOTA: usize = 1_000;

/// How long after `CreatePartnerApp` the clock owes the `Creating` -> `Available`
/// transition of `PartnerAppStatus`.
///
/// Same contract as [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that lists or describes the
/// app straight after creating it still observes `Creating` (and the state refusals that
/// go with it), one that waits observes `Available`. Real SageMaker takes minutes to
/// provision a partner application; the span is compressed, never zero, and the due
/// instant is stored rather than slept on.
pub const PARTNER_APP_AVAILABLE_AFTER: Duration = ENDPOINT_IN_SERVICE_AFTER;

/// Where the partner app `app_id` is served in `region`: the host SageMaker gives one
/// partner AI app, which `DescribePartnerApp` reports as `BaseUrl` and
/// `CreatePartnerAppPresignedUrl` builds its URL on. The app's own id is what makes it
/// unique, so the host carries it.
pub fn partner_app_base_url(region: &str, app_id: &str) -> String {
    format!("https://{app_id}.{region}.partner-app.sagemaker.aws")
}

/// A fresh partner-app id: `app-` and the twelve upper-case characters the modelled
/// `PartnerAppArn` pattern admits after it (`partner-app\/app-[A-Z0-9]{12}`).
///
/// The caller never supplies it — a partner app is addressed by its ARN, so the id is the
/// server's to generate, and it is derived from nothing in the request. Same scheme as
/// every other id in this crate ([`mint_short_id`]), upper-cased to the alphabet this
/// pattern admits.
pub fn mint_partner_app_id() -> String {
    format!("app-{}", mint_short_id().to_ascii_uppercase())
}

/// How the partner-app operations state an app this account holds none of.
///
/// A FIXED sentence carrying NOTHING from the request: a partner AI app is addressed by
/// its ARN, and the service names no identifier here, so the refusal does not echo back
/// the ARN a caller probed with. Stated once here so the reader (`DescribePartnerApp`) and
/// the URL mint (`CreatePartnerAppPresignedUrl`) cannot word the same fact differently.
pub const PARTNER_APP_NOT_FOUND: &str = "App entity does not exist.";

/// How `CreatePartnerApp` refuses a `Type` this account holds no partner-application
/// subscription for: the type asked for, and what the caller has to do about it.
///
/// A partner AI app runs a third party's application, which is licensed to the account
/// through AWS Marketplace rather than through this API — so a type the account has not
/// subscribed to cannot be provisioned, and the refusal names the subscription that is
/// missing. See [`ComAmazonawsSagemakerSagemakerState::subscribed_to_partner_app`].
pub fn partner_app_not_subscribed(app_type: &str) -> String {
    format!(
        "This account does not have an active AWS Marketplace subscription for the \
         SageMaker Partner AI App type {app_type}. Subscribe to the partner application \
         in AWS Marketplace before creating the app."
    )
}

/// The configuration one partner AI app carries: everything `CreatePartnerApp` is given
/// about HOW the app runs, as opposed to the identity SageMaker mints for it.
///
/// Kept as one record so `DescribePartnerApp` reports exactly what the create stored, and
/// so `UpdatePartnerApp` — which takes the same members again — replaces the
/// configuration without touching the app's identity, status or grants.
#[derive(Debug, Clone, Default)]
pub struct PartnerAppSettings {
    /// `ExecutionRoleArn` — the IAM role the partner application uses.
    pub execution_role_arn: String,
    /// `Tier` — the instance type and size of the cluster attached to the app.
    pub tier: String,
    /// `AuthType` — the authorization type users access the app with.
    pub auth_type: String,
    /// `KmsKeyId`, when the request named a customer managed key.
    pub kms_key_id: Option<String>,
    pub maintenance_config: Option<crate::ops::types::PartnerAppMaintenanceConfig>,
    pub application_config: Option<crate::ops::types::PartnerAppConfig>,
    /// `IdcConfig` — the Identity Center instance an `IDC` app federates with.
    pub idc_config: Option<crate::ops::types::IdcConfigInput>,
    pub enable_iam_session_based_identity: Option<bool>,
    pub enable_auto_minor_version_upgrade: Option<bool>,
}

/// A SageMaker Partner AI App, as `ListPartnerApps` summarises it and
/// `DescribePartnerApp` reports it.
///
/// Keyed in state by [`PartnerApp::app_id`] (the ARN's `app-...` resource id), which AWS
/// mints and every later call addresses the app by. `CreatePartnerApp` fills the store and
/// `ListPartnerApps` pages whatever it holds; the updating and deleting handlers extend
/// [`ComAmazonawsSagemakerSagemakerState::partner_apps`] rather than adding a second map.
///
/// `available_at` is the instant the clock owes the `Creating` -> `Available` transition;
/// it is stored, never slept on, and materialised by whichever reporting call first runs
/// at or after it.
#[derive(Debug, Clone)]
pub struct PartnerApp {
    pub app_id: String,
    pub arn: String,
    pub name: String,
    pub app_type: String,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub available_at: SystemTime,
    /// Where the app itself is served; the presigned URL
    /// `CreatePartnerAppPresignedUrl` answers is this plus the grant it minted.
    pub base_url: String,
    /// How the app runs, as the create was given it. See [`PartnerAppSettings`].
    pub settings: PartnerAppSettings,
    /// The `ClientToken` the create that minted this app presented, if any, so a retried
    /// create answers the app it already made instead of a second one.
    pub client_token: Option<String>,
    /// The `Tags` the create carried, which the tag operations read and write.
    pub tags: Vec<Tag>,
    /// Every presigned-URL grant minted for this app that has not expired, oldest
    /// first. See [`PresignedUrlGrant`].
    pub grants: Vec<PresignedUrlGrant>,
}

impl PartnerApp {
    /// Materialise the `Creating` -> `Available` transition the clock already owes, the
    /// same way [`Endpoint::settle_status`] does for an endpoint: it was due at
    /// `available_at` whether or not anybody listed the apps, so the observer makes no
    /// difference to the result, and `LastModifiedTime` moves to the instant the
    /// transition was due rather than to "now".
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == PARTNER_APP_STATUS_CREATING && now >= self.available_at {
            self.status = PARTNER_APP_STATUS_AVAILABLE.to_string();
            self.last_modified_time = self.available_at;
            return true;
        }
        false
    }
}

// ---------------------------------------------------------------------------
// The SageMaker HyperPod cluster slice: everything `CreateCluster` writes and
// `ListClusters` reports, kept together at the end of this module the way the
// lineage-action and AI-workload slices above are, with its own quota, its own
// clock-owed span and its own inherent `impl` block on the state struct.
// ---------------------------------------------------------------------------

/// How long after `CreateCluster` the clock owes the `Creating` -> `InService`
/// transition of a HyperPod cluster. Same compressed, never-zero span as
/// [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that lists clusters straight after creating
/// one still observes `Creating`, one that waits observes `InService`. Real HyperPod
/// provisioning takes many minutes; the span is the emulator's own choice.
pub const CLUSTER_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// How long after `DeleteCluster` the clock owes the `Deleting` -> gone transition of a
/// HyperPod cluster. The same compressed, never-zero span as
/// [`CLUSTER_IN_SERVICE_AFTER`]: a caller that describes the cluster straight after
/// asking for its deletion still observes `Deleting`, one that waits observes the
/// cluster gone. Real HyperPod teardown takes many minutes; the span is the emulator's
/// own choice.
pub const CLUSTER_RELEASED_AFTER: Duration = CLUSTER_IN_SERVICE_AFTER;

/// `ClusterStatus`'s terminal failure member. Nothing in this emulator fails a cluster,
/// so no record carries it — it is named because a delete is admitted for a cluster in
/// it as well as for one `InService` ([`Cluster::accepts_deletion`]), and a handler must
/// not compare a bare string literal.
pub const CLUSTER_STATUS_FAILED: &str = "Failed";

/// See [`MODEL_QUOTA`]; bounds the cluster store, and is released by `DeleteCluster`.
pub const CLUSTER_QUOTA: usize = 1_000;

/// `ClusterSortBy`'s documented default ("The field by which to sort results. The
/// default value is `CREATION_TIME`." — `ListClusters`' member doc). `ListClusters` has
/// its own default and its own enum tokens, which is why it does not share
/// [`DEFAULT_SORT_BY`].
pub const CLUSTER_SORT_BY_CREATION_TIME: &str = "CREATION_TIME";
/// `ClusterSortBy`'s other member.
pub const CLUSTER_SORT_BY_NAME: &str = "NAME";
/// `SortOrder`'s documented default for `ListClusters` ("The sort order for results. The
/// default value is `Ascending`.") — the opposite of [`DEFAULT_SORT_ORDER`], which is
/// what `ListModels` documents. The descending token is [`ORDER_DESCENDING`].
pub const CLUSTER_DEFAULT_SORT_ORDER: &str = ORDER_ASCENDING;

/// A SageMaker HyperPod cluster created by `CreateCluster`, as `ListClusters`
/// summarises it and `DescribeCluster` replays it.
///
/// The whole creation spec is stored, not only the members `ListClusters` reports: the
/// record is the resource, so the readers of the rest (`DescribeCluster`,
/// `ListClusterNodes`) read it from here rather than re-deriving it.
///
/// `in_service_at` is the instant the clock owes the `Creating` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
#[derive(Debug, Clone)]
pub struct Cluster {
    pub name: String,
    /// The 12-character id the `ClusterArn` pattern carries
    /// (`…:cluster/[a-z0-9]{12}`), minted once at create time.
    pub id: String,
    pub arn: String,
    pub status: String,
    pub instance_groups: Vec<crate::ops::types::ClusterInstanceGroupSpecification>,
    pub restricted_instance_groups:
        Vec<crate::ops::types::ClusterRestrictedInstanceGroupSpecification>,
    pub restricted_instance_groups_config:
        Option<crate::ops::types::ClusterRestrictedInstanceGroupsConfig>,
    pub vpc_config: Option<VpcConfig>,
    pub orchestrator: Option<crate::ops::types::ClusterOrchestrator>,
    pub node_recovery: Option<String>,
    pub tiered_storage_config: Option<crate::ops::types::ClusterTieredStorageConfig>,
    pub node_provisioning_mode: Option<String>,
    pub cluster_role: Option<String>,
    pub auto_scaling: Option<crate::ops::types::ClusterAutoScalingConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub in_service_at: SystemTime,
    /// The instant the clock owes this cluster's deletion, set by
    /// [`Cluster::begin_deletion`]; `None` while no `DeleteCluster` has asked for one.
    /// Past it the cluster is gone for every caller ([`Cluster::is_released`]), whether
    /// or not a sweep has released the record yet.
    pub removal_due_at: Option<SystemTime>,
    /// The cluster's nodes, keyed by `NodeLogicalId` — the instances its groups were
    /// created asking for plus the ones `BatchAddClusterNodes` added, which
    /// `ListClusterNodes` reports, `DescribeClusterNode` replays and
    /// `AttachClusterNodeVolume` attaches storage inside. A cluster still `Creating`
    /// holds none: its groups' instances are minted when it reaches `InService`, by
    /// [`Cluster::settle_status`].
    pub nodes: BTreeMap<String, ClusterNode>,
    /// What a `BatchAddClusterNodes` `ClientToken` replays: the outcome of the first call
    /// carrying that token. Keyed by the token.
    pub node_additions: BTreeMap<String, NodeAdditionRecord>,
}

impl Cluster {
    /// Whether a delete may begin on this cluster: its status is a settled one.
    ///
    /// A cluster the service is already moving — provisioning it, updating it, rolling
    /// an update back, or tearing it down — cannot take a second change at the same
    /// time, which is the conflict `DeleteCluster` models `ConflictException` for. The
    /// settled statuses are `InService` and [`CLUSTER_STATUS_FAILED`].
    pub fn accepts_deletion(&self) -> bool {
        self.status == STATUS_IN_SERVICE || self.status == CLUSTER_STATUS_FAILED
    }

    /// Start the deletion `DeleteCluster` asked for: the cluster reports `Deleting`, and
    /// the clock owes its release [`CLUSTER_RELEASED_AFTER`] later.
    ///
    /// The record is NOT dropped here: SageMaker answers a cluster in `ClusterStatus`'s
    /// `Deleting` until the teardown finishes, so the transient status is observable and
    /// the removal is the clock's — stored as an instant and never slept on.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = STATUS_DELETING.to_string();
        self.removal_due_at = Some(now + CLUSTER_RELEASED_AFTER);
    }

    /// Whether a `DeleteCluster` has asked for this cluster's deletion, i.e. it is
    /// `Deleting` and on its way out.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the deletion the clock owed has completed, i.e. the cluster is gone.
    ///
    /// A lookup answers `None` for such a cluster and a listing leaves it out, so no
    /// caller sees a cluster whose teardown was already due — the record itself is
    /// dropped by [`ComAmazonawsSagemakerSagemakerState::release_deleted_clusters`],
    /// which frees the name and the quota slot it held.
    pub fn is_released(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due| now >= due)
    }

    /// Materialise the `Creating` -> `InService` transition the clock already owes.
    ///
    /// The one write a reporting operation may make: the transition was due at
    /// `in_service_at` whether or not anybody listed the cluster, so the observer makes
    /// no difference to the result. Answers whether it happened on this call.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == STATUS_CREATING && now >= self.in_service_at {
            self.status = STATUS_IN_SERVICE.to_string();
            self.provision_group_instances(now);
            return true;
        }
        false
    }

    /// Bring every instance group up to the number of instances it declares, minting the
    /// ones it is short of as `Running` nodes of that group.
    ///
    /// `InstanceCount` is "the number of instances to add to the instance group", so a
    /// group that declares N holds N of them once the cluster is provisioned — the same N
    /// that [`Cluster::instance_group_details`] reports as realised in `CurrentCount`, so
    /// the cluster's counts and its node list cannot disagree. They come up WITH the
    /// cluster, which is why this runs at the instant it reaches `InService` and never
    /// before: a caller listing nodes straight after the create sees none, and one that
    /// waits sees the instances the groups asked for.
    ///
    /// A group is short by however many nodes it does not already hold, so the nodes a
    /// call added to it (which raised its count by the same amount) are not minted twice.
    /// The whole cluster stays inside [`CLUSTER_NODE_QUOTA`], the bound every other node
    /// path honours.
    fn provision_group_instances(&mut self, now: SystemTime) {
        // The groups' settings are read off first, so the nodes can be written while
        // iterating them. A restricted group's instances are instances of this cluster
        // too, and `ListClusterNodes` reports "the list of instances ... in a SageMaker
        // HyperPod cluster".
        let declared: Vec<(String, i32, Option<String>, Option<i32>)> = self
            .instance_groups
            .iter()
            .map(|group| {
                (
                    group.instance_group_name.clone(),
                    group.instance_count,
                    instance_group_types(group).first().cloned(),
                    group.threads_per_core,
                )
            })
            .chain(self.restricted_instance_groups.iter().map(|group| {
                (
                    group.instance_group_name.clone(),
                    group.instance_count,
                    Some(group.instance_type.clone()),
                    group.threads_per_core,
                )
            }))
            .collect();

        for (group_name, instance_count, instance_type, threads_per_core) in declared {
            let held = self
                .nodes
                .values()
                .filter(|node| node.instance_group_name == group_name)
                .count();
            let declared_count = usize::try_from(instance_count).unwrap_or_default();
            for _ in held..declared_count {
                if self.nodes.len() >= CLUSTER_NODE_QUOTA {
                    return;
                }
                let logical_id = mint_node_logical_id();
                self.nodes.insert(
                    logical_id.clone(),
                    ClusterNode {
                        logical_id: logical_id.clone(),
                        // An instance that is up has its id, the same one the
                        // `Pending` -> `Running` transition mints for an added node.
                        instance_id: Some(mint_instance_id()),
                        instance_group_name: group_name.clone(),
                        instance_type: instance_type.clone(),
                        status: NODE_STATUS_RUNNING.to_string(),
                        // A group's zones follow from the subnets of its VPC
                        // configuration, which this emulator does not resolve.
                        availability_zone: None,
                        threads_per_core,
                        launch_time: now,
                        running_at: now,
                        // Nothing has asked for this node's hardware to be replaced.
                        replacement_due_at: None,
                        volume_attachments: BTreeMap::new(),
                    },
                );
            }
        }
    }

    /// The training plans this cluster's capacity is reserved against, in first-seen
    /// order with duplicates dropped.
    ///
    /// `ClusterSummary::TrainingPlanArns` is "a list of ARNs of the training plans
    /// associated with this cluster", and a cluster is associated with a plan through
    /// the `TrainingPlanArn` of an instance group it was created with — so the list is
    /// computed from the stored groups rather than stored a second time.
    pub fn training_plan_arns(&self) -> Vec<String> {
        let mut arns: Vec<String> = Vec::new();
        let from_groups = self
            .instance_groups
            .iter()
            .map(|group| group.training_plan_arn.as_ref())
            .chain(
                self.restricted_instance_groups
                    .iter()
                    .map(|group| group.training_plan_arn.as_ref()),
            );
        for arn in from_groups.flatten() {
            if !arns.iter().any(|seen| seen == arn) {
                arns.push(arn.clone());
            }
        }
        arns
    }
}

/// The instance types an instance group admits: the one its specification names, or the
/// list its `InstanceRequirements` names.
///
/// The model makes the two mutually exclusive ("The `InstanceType` and
/// `InstanceRequirements` properties are mutually exclusive" —
/// `ClusterInstanceRequirements`' own doc), and a group may name neither, which is why
/// this answers a list that can be empty: a group that names no type admits any.
pub fn instance_group_types(
    group: &crate::ops::types::ClusterInstanceGroupSpecification,
) -> Vec<String> {
    if let Some(instance_type) = &group.instance_type {
        return vec![instance_type.clone()];
    }
    group
        .instance_requirements
        .as_ref()
        .map(|requirements| requirements.instance_types.clone())
        .unwrap_or_default()
}

/// Mint the id a `ClusterArn` carries: the 12 lowercase hex digits
/// [`mint_short_id`] produces, which is exactly the `[a-z0-9]{12}` the
/// modelled `ClusterArn` pattern admits.
///
/// One minter for the service: the entropy, its bounds and the reasons it is not a
/// secret are stated once, on [`mint_opaque_resource_id`]. `CreateCluster` still
/// re-mints on a collision rather than assuming uniqueness.
pub fn mint_cluster_id() -> String {
    mint_short_id()
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another cluster fits, i.e. the store is below [`CLUSTER_QUOTA`]. A second
    /// inherent `impl` block, beside this slice's own types rather than in the one above.
    pub fn clusters_below_quota(&self) -> bool {
        self.clusters.len() < CLUSTER_QUOTA
    }

    /// Drop every cluster whose deletion the clock already completed, answering their
    /// ARNs in store order.
    ///
    /// Materialises only what was already due: the release was owed at the stored instant
    /// whether or not anybody called, and a lookup already answers `None` for such a
    /// cluster ([`Cluster::is_released`]), so this changes nothing a later caller sees —
    /// it frees the name and the quota slot the gone cluster still held.
    pub fn release_deleted_clusters(&mut self, now: SystemTime) -> Vec<String> {
        let released: Vec<(String, String)> = self
            .clusters
            .values()
            .filter(|cluster| cluster.is_released(now))
            .map(|cluster| (cluster.name.clone(), cluster.arn.clone()))
            .collect();
        for (name, _) in &released {
            self.clusters.remove(name);
        }
        released.into_iter().map(|(_, arn)| arn).collect()
    }

    /// The cluster whose `ClusterArn` is `arn`, if this account holds one.
    ///
    /// [`Self::clusters`] is keyed by `ClusterName` — the identifier `CreateCluster`
    /// takes — while another resource that BELONGS to a cluster names it by ARN
    /// (`CreateClusterSchedulerConfig$ClusterArn`), so the lookup is by the stored
    /// `Cluster::arn`. One place for it, so every operation resolving a `ClusterArn`
    /// resolves it against the same records `CreateCluster` writes and `ListClusters`
    /// reports, rather than against a second copy of the cluster.
    pub fn cluster_by_arn(&self, arn: &str) -> Option<&Cluster> {
        self.clusters
            .values()
            .find(|cluster| cluster.arn == arn)
            .filter(|cluster| !cluster.is_released(SystemTime::now()))
    }
}

// ---------------------------------------------------------------------------
// The lineage-group slice: the record `DescribeLineageGroup` reports, and the two
// accessors the reader and the lineage-entity creates reach it through. Kept at the end
// of the module, with its own inherent `impl` block, the way the lineage-action and
// workload-configuration slices above are.
// ---------------------------------------------------------------------------

/// A lineage group, as `DescribeLineageGroup` reports it.
///
/// SageMaker models no `CreateLineageGroup`: a lineage group comes into being with the
/// account's lineage entities (every artifact, action, context and trial component
/// belongs to one), so a record here is materialised through
/// [`ComAmazonawsSagemakerSagemakerState::ensure_lineage_group`] rather than by a create
/// handler — and a group nothing has materialised is simply absent from the store, which
/// is what the modelled `ResourceNotFound` reports.
#[derive(Debug, Clone)]
pub struct LineageGroup {
    pub name: String,
    pub arn: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub created_by: Option<UserContext>,
    pub last_modified_by: Option<UserContext>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// The resource-based policy document that gives another account access to this
    /// group, as `GetLineageGroupPolicy` reports it: an IAM policy document, and
    /// [`EMPTY_RESOURCE_POLICY`] while nothing has shared the group.
    ///
    /// Held on the group's own record rather than in a second map, so the policy a group
    /// carries lives beside the group a describe reports and cannot outlive it. The whole
    /// SageMaker surface models no operation that attaches one — a lineage group is shared
    /// with another account by a resource share, not by a call on this service — so a
    /// group this service materialises carries the empty document.
    pub resource_policy: String,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// The lineage group `region` holds under `name`, or `None` when no lineage entity
    /// has materialised it there.
    ///
    /// A lineage group is regional, so the Region is half the key: a group materialised
    /// by an entity created in one Region is not reported to a caller signing for
    /// another, which is what keeps a replayed `LineageGroupArn` naming the Region the
    /// reader signed for.
    pub fn lineage_group(&self, region: &str, name: &str) -> Option<&LineageGroup> {
        self.lineage_groups
            .get(&(region.to_string(), name.to_string()))
    }

    /// Every lineage group `region` holds, in `LineageGroupName` order.
    ///
    /// The Region is half the key (see [`Self::lineage_group`]), so a listing reports only
    /// the groups the caller's Region holds: a group an entity materialised elsewhere is
    /// not this caller's, exactly as the describe does not report it. One accessor for it,
    /// so `ListLineageGroups` and `DescribeLineageGroup` cannot disagree about which
    /// groups a Region has.
    pub fn lineage_groups_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a LineageGroup> + 'a {
        self.lineage_groups
            .iter()
            .filter(move |((group_region, _), _)| group_region.as_str() == region)
            .map(|(_, group)| group)
    }

    /// Materialise the lineage group `name` in `region` unless the store already holds
    /// it, and answer the ARN of the group this call CREATED.
    ///
    /// This is the ONLY way a lineage group enters the store, because SageMaker models no
    /// `CreateLineageGroup`: the service materialises a group when the account's lineage
    /// entities put something in it, so the lineage-entity creates call this with
    /// [`DEFAULT_LINEAGE_GROUP`] — `CreateArtifact`, `CreateAction`, `CreateContext` and
    /// `CreateTrialComponent` all do. Readers never call it: a reporting operation must
    /// not mint a resource.
    ///
    /// `DisplayName` is the group's own name: nothing in the API supplies one for a group
    /// the service materialises itself, and "if `DisplayName` isn't specified, the name is
    /// displayed" is the rule every other SageMaker entity's model doc states.
    /// `CreatedBy`/`LastModifiedBy` stay absent — there is no Studio user context behind a
    /// group the service created, and an invented one would be a value no request made.
    ///
    /// `None` means nothing changed: either that Region already held that group (so a
    /// second entity does not move its stored instants), or the store is at
    /// [`LINEAGE_GROUP_QUOTA`] and cannot take another, which bounds the growth a caller
    /// varying the Region or the name could otherwise cause. A caller that gets
    /// `Some(arn)` records the state change under that ARN, so a materialised group is
    /// auditable like any create. The at-the-bound case is WARNED about here rather than
    /// by each caller, so every lineage-entity create reports it the same way and none
    /// can drift.
    pub fn ensure_lineage_group(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<String> {
        // Read before the entry() borrow, so the bound is checked against the same store
        // this insert grows.
        let at_capacity = self.lineage_groups.len() >= LINEAGE_GROUP_QUOTA;
        match self
            .lineage_groups
            .entry((region.to_string(), name.to_string()))
        {
            ::std::collections::btree_map::Entry::Occupied(_) => None,
            ::std::collections::btree_map::Entry::Vacant(_) if at_capacity => {
                // Visible rather than silent: past the bound the entity this call creates
                // still reports its Region's lineage-group ARN, which
                // `DescribeLineageGroup` cannot answer until the store is back under it.
                tracing::warn!(
                    lineage_group = name,
                    region,
                    bound = LINEAGE_GROUP_QUOTA,
                    "the lineage-group store is at its bound, so this Region's lineage \
                     group was not materialised"
                );
                None
            }
            ::std::collections::btree_map::Entry::Vacant(slot) => {
                let arn = named_lineage_group_arn(region, name);
                slot.insert(LineageGroup {
                    name: name.to_string(),
                    arn: arn.clone(),
                    display_name: Some(name.to_string()),
                    description: None,
                    created_by: None,
                    last_modified_by: None,
                    creation_time: now,
                    last_modified_time: now,
                    // Nothing has shared a group the service has just materialised, so it
                    // carries the empty policy document.
                    resource_policy: EMPTY_RESOURCE_POLICY.to_string(),
                });
                Some(arn)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The MLflow tracking-server slice: `CreateMlflowTrackingServer` and
// `DescribeMlflowTrackingServer`. Kept together as its own slice, rather than spread
// through the blocks around it, so this one resource kind reads as one block.
// ---------------------------------------------------------------------------

/// The ARN resource kind an MLflow tracking server carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `TrackingServerArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:mlflow-tracking-server/`), and
/// named once here so the create and the describe mint and match the same ARN.
pub const TRACKING_SERVER_KIND: &str = "mlflow-tracking-server";

/// See [`MODEL_QUOTA`]. Bounds the MLflow tracking-server store the same way, and is
/// what `CreateMlflowTrackingServer` answers `ResourceLimitExceeded` past.
pub const MLFLOW_TRACKING_SERVER_QUOTA: usize = 1_000;

/// How long after `CreateMlflowTrackingServer` the clock owes the `Creating` ->
/// `Created` transition on a tracking server. Same shape as
/// [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the server straight after
/// creating it still observes `Creating`, one that waits observes `Created`. Real
/// SageMaker takes minutes; the span is compressed, never zero.
pub const TRACKING_SERVER_CREATED_AFTER: Duration = Duration::from_millis(300);

/// `TrackingServerStatus`'s members this emulator moves a tracking server through. The
/// modelled enum also carries the update, stop, start and maintenance states, which
/// `CreateMlflowTrackingServer` and `DescribeMlflowTrackingServer` do not reach. Named
/// here rather than shared with [`STATUS_CREATING`]: that one is an `EndpointStatus`
/// member, a different modelled enum that happens to spell one state the same way.
pub const TRACKING_SERVER_CREATING: &str = "Creating";
/// See [`TRACKING_SERVER_CREATING`].
pub const TRACKING_SERVER_CREATED: &str = "Created";

/// `IsTrackingServerActive`'s two members: a tracking server is reported `Active` once
/// it has finished being created, and `Inactive` while it is still coming up.
pub const TRACKING_SERVER_ACTIVE: &str = "Active";
/// See [`TRACKING_SERVER_ACTIVE`].
pub const TRACKING_SERVER_INACTIVE: &str = "Inactive";

/// The size a tracking server gets when `CreateMlflowTrackingServer` names none:
/// `TrackingServerSize`'s model doc says "The default MLflow Tracking Server
/// configuration size is \"Small\"".
pub const DEFAULT_TRACKING_SERVER_SIZE: &str = "Small";

/// An MLflow tracking server created by `CreateMlflowTrackingServer`, as
/// `DescribeMlflowTrackingServer` replays it.
///
/// `created_due_at` is the instant the clock owes the `Creating` -> `Created`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
///
/// `mlflow_version` and `weekly_maintenance_window_start` stay `None` when the create
/// named neither: the model documents no default for either, so the server reports no
/// value rather than one the request never asked for. `tags` is stored so a later
/// `ListTags` reports what the create carried; `DescribeMlflowTrackingServer` models no
/// tag member, so `ListTags` is the one reader that surfaces it.
#[derive(Debug, Clone)]
pub struct MlflowTrackingServer {
    pub name: String,
    pub arn: String,
    pub artifact_store_uri: String,
    pub size: String,
    pub mlflow_version: Option<String>,
    pub role_arn: String,
    pub automatic_model_registration: bool,
    pub weekly_maintenance_window_start: Option<String>,
    pub s3_bucket_owner_account_id: String,
    pub s3_bucket_owner_verification: bool,
    pub status: String,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_due_at: SystemTime,
}

impl MlflowTrackingServer {
    /// Materialise the `Creating` -> `Created` transition the clock already owes, the
    /// way [`Endpoint::settle_status`] does for an endpoint: it was due at
    /// `created_due_at` whether or not anybody described the server, so the observer
    /// makes no difference to the result, and `LastModifiedTime` moves to the instant
    /// the transition was due rather than to "now".
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == TRACKING_SERVER_CREATING && now >= self.created_due_at {
            self.status = TRACKING_SERVER_CREATED.to_string();
            self.last_modified_time = self.created_due_at;
            return true;
        }
        false
    }

    /// `IsActive` for the status the server is in, as one of
    /// `IsTrackingServerActive`'s two modelled members: a server is active once it is
    /// `Created`, and not before.
    pub fn is_active(&self) -> &'static str {
        if self.status == TRACKING_SERVER_CREATED {
            TRACKING_SERVER_ACTIVE
        } else {
            TRACKING_SERVER_INACTIVE
        }
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another MLflow tracking server fits, i.e. the store is below
    /// [`MLFLOW_TRACKING_SERVER_QUOTA`]. A second inherent `impl` block, beside this
    /// slice's own types rather than in the one above.
    pub fn mlflow_tracking_servers_below_quota(&self) -> bool {
        self.mlflow_tracking_servers.len() < MLFLOW_TRACKING_SERVER_QUOTA
    }
}

// ---------------------------------------------------------------------------
// The experiment-tracking slice: `CreateTrialComponent` and `DescribeTrialComponent`.
// Kept together at the end of this module, the way the AI-benchmarking slice above is,
// rather than spread through the blocks at the top.
// ---------------------------------------------------------------------------

/// The ARN resource kind a trial component carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `TrialComponentArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial-component/`), and
/// named once here so the create and the describe mint and match the same ARN.
pub const TRIAL_COMPONENT_KIND: &str = "experiment-trial-component";

/// See [`MODEL_QUOTA`]. Bounds the trial-component store the same way, and is what
/// `CreateTrialComponent` answers `ResourceLimitExceeded` past.
pub const TRIAL_COMPONENT_QUOTA: usize = 1_000;

/// See [`ASSOCIATION_QUOTA`], which bounds the lineage-edge store for the same reason and
/// at the same figure: a trial-component membership is an EDGE too — the index grows with
/// the PAIRS of trial and component a caller attaches, not with either resource — so it is
/// higher than the per-resource bounds, and it is what `AssociateTrialComponent` answers
/// `ResourceLimitExceeded` past.
pub const TRIAL_COMPONENT_TRIAL_QUOTA: usize = 10_000;

/// A trial component created by `CreateTrialComponent`, as `DescribeTrialComponent`
/// replays it.
///
/// `start_time` / `end_time` keep the modelled `Timestamp` the CALLER sent rather than a
/// `SystemTime`, so a describe replays the instant bit-for-bit however much precision the
/// request carried; `creation_time` / `last_modified_time` are the emulator's own clock
/// readings, so they are `SystemTime` like every other record here. `created_by` is stored
/// at creation rather than rebuilt per read, so every later read reports the identity that
/// actually created the component. `parameters`, `input_artifacts` and `output_artifacts`
/// are the collections the component HOLDS, empty while nothing has put an entry in them
/// (see this module's note on an entity's own collections).
#[derive(Debug, Clone)]
pub struct TrialComponent {
    pub name: String,
    pub arn: String,
    /// The Region the create signed for, kept beside the ARN the way [`Artifact`] and
    /// [`Action`] keep theirs: a trial component is a lineage entity, so a reader reports
    /// the lineage group of the Region the component was created in (see
    /// [`lineage_group_arn`]) rather than of the Region the READER happens to sign for.
    pub region: String,
    /// Never `None`: `DisplayName`'s model doc says "If DisplayName isn't specified,
    /// TrialComponentName is displayed", so a create without one stores the name.
    pub display_name: String,
    pub status: Option<crate::ops::types::TrialComponentStatus>,
    pub start_time: Option<::aws_smithy_types::DateTime>,
    pub end_time: Option<::aws_smithy_types::DateTime>,
    pub parameters: HashMap<String, crate::ops::types::TrialComponentParameterValue>,
    pub input_artifacts: HashMap<String, crate::ops::types::TrialComponentArtifact>,
    pub output_artifacts: HashMap<String, crate::ops::types::TrialComponentArtifact>,
    pub metadata_properties: Option<MetadataProperties>,
    pub created_by: UserContext,
    /// Who last modified the component: the creating identity until a call modifies it,
    /// when `UpdateTrialComponent` records the identity behind that call.
    pub last_modified_by: UserContext,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// The store key one lineage entity of `kind` is held under, and the ARN any resource of
/// `kind` addressed by a bare name OR a full ARN resolves to (the lineage members'
/// `ExperimentEntityNameOrArn` / `ContextNameOrArn`, a `ModelCardNameOrArn`, a
/// `VersionedArnOrName`).
///
/// Every lineage store in this module is keyed by the entity's ARN, which carries both the
/// Region the name is unique within and the name itself — SageMaker documents these names
/// as unique to an account IN A REGION. So a bare name is resolved against `region`, the
/// Region this request signed for, while an ARN is matched as a WHOLE: it addresses the one
/// entity that ARN names, and one differing in the entity kind, the account, the partition
/// or the Region addresses a different entity (and so, here, none). A name can never be
/// mistaken for an ARN: no lineage name pattern admits a `:`.
///
/// Shared by the action slice ([`ComAmazonawsSagemakerSagemakerState::action_key`]), the
/// context slice ([`context_key`]) and the refusals that NAME the resolved ARN
/// ([`model_card_not_found`], [`model_package_not_found`]), so the name-or-ARN split is
/// decided in ONE place: a name has to be resolved, because the caller sent no ARN to
/// report, while an ARN is echoed, because rewriting it would report an identifier the
/// caller never sent.
pub fn arn_from_name_or_arn(region: &str, kind: &str, identifier: &str) -> String {
    if identifier.starts_with(ARN_PREFIX) {
        identifier.to_string()
    } else {
        resource_arn(region, kind, identifier)
    }
}

/// The store key for the context a request addresses: that context's ARN, resolved by
/// [`arn_from_name_or_arn`] against the Region the request signed for.
///
/// `CreateContext`'s `ContextName` is documented unique to an account in a Region, so the
/// name alone does not identify a context — the ARN, which carries the Region and the
/// account beside the name, does. `DescribeContext`'s `ContextNameOrArn` is what lets a
/// caller spell either; `UpdateContext` and `DeleteContext` take a plain `ContextName`,
/// whose pattern admits no `:`, so for them only the name branch is ever reached.
pub fn context_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    name_or_arn: &str,
) -> String {
    arn_from_name_or_arn(&request_region(ctx), CONTEXT_KIND, name_or_arn)
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another context fits, i.e. the store is below [`CONTEXT_QUOTA`]. A second
    /// inherent `impl` block, beside this slice's own types rather than in the one above.
    pub fn contexts_below_quota(&self) -> bool {
        self.contexts.len() < CONTEXT_QUOTA
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another trial component fits, i.e. the store is below
    /// [`TRIAL_COMPONENT_QUOTA`]. A second inherent `impl` block, beside this slice's own
    /// types rather than in the one above.
    pub fn trial_components_below_quota(&self) -> bool {
        self.trial_components.len() < TRIAL_COMPONENT_QUOTA
    }

    /// Whether another trial-component membership fits, i.e. [`Self::trial_component_trials`]
    /// is below [`TRIAL_COMPONENT_TRIAL_QUOTA`]. What `AssociateTrialComponent` admits a
    /// NEW membership by.
    pub fn trial_component_trials_below_quota(&self) -> bool {
        self.trial_component_trials.len() < TRIAL_COMPONENT_TRIAL_QUOTA
    }

    /// The [`entity_key`]s of the trial components that are part of the trial `trial_name`
    /// names, read out of [`Self::trial_component_trials`].
    ///
    /// The name is folded the way the trial store folds it, so a filter spelling the trial
    /// in another case selects the same components.
    pub fn trial_component_keys_in_trial(&self, trial_name: &str) -> BTreeSet<String> {
        let trial = entity_key(trial_name);
        self.trial_component_trials
            .iter()
            .filter(|(member_of, _)| member_of == &trial)
            .map(|(_, component)| component.clone())
            .collect()
    }

    /// The [`entity_key`]s of the trials the trial component `component_key` ([`entity_key`]
    /// of its name) is part of, read out of [`Self::trial_component_trials`].
    ///
    /// The other direction of the same index as [`Self::trial_component_keys_in_trial`]:
    /// what `DeleteTrialComponent` tests the documented "must be disassociated from all
    /// trials" precondition by.
    pub fn trial_keys_holding_trial_component(&self, component_key: &str) -> BTreeSet<String> {
        self.trial_component_trials
            .iter()
            .filter(|(_, component)| component == component_key)
            .map(|(member_of, _)| member_of.clone())
            .collect()
    }

    /// The [`entity_key`]s of the trials that belong to the experiment `experiment_name`
    /// names, read out of [`Self::trials`].
    ///
    /// A trial names the experiment it belongs to ([`Trial::experiment_name`]), so the
    /// experiment's membership is read off the trial records rather than kept a second
    /// time beside them. Both names are folded the way the stores fold them, so a caller
    /// spelling the experiment in another case selects the same trials. What
    /// `DeleteExperiment` tests the documented "all trials associated with the experiment
    /// must be deleted first" precondition by, and the first step of
    /// [`Self::trial_component_keys_in_experiment`].
    pub fn trial_keys_in_experiment(&self, experiment_name: &str) -> BTreeSet<String> {
        let experiment = entity_key(experiment_name);
        self.trials
            .values()
            .filter(|trial| entity_key(&trial.experiment_name) == experiment)
            .map(|trial| entity_key(&trial.name))
            .collect()
    }

    /// The [`entity_key`]s of the trial components that are part of ANY trial of the
    /// experiment `experiment_name` names.
    ///
    /// A component is part of an experiment through a trial — a trial names the experiment
    /// it belongs to ([`Trial::experiment_name`]) and a component is attached to a trial —
    /// so this is the membership of the experiment's trials taken together.
    pub fn trial_component_keys_in_experiment(&self, experiment_name: &str) -> BTreeSet<String> {
        let trials = self.trial_keys_in_experiment(experiment_name);
        self.trial_component_trials
            .iter()
            .filter(|(member_of, _)| trials.contains(member_of))
            .map(|(_, component)| component.clone())
            .collect()
    }
}

/// The store key a `TrialComponentName` (or any other case-insensitive SageMaker name,
/// such as an `AutoMLJobName`) maps to.
///
/// The member's model doc says the name "must be unique in your Amazon Web Services
/// account and is not case-sensitive" (`AutoMLJobName`'s says "must be unique to your
/// account and is case insensitive"), so two names differing only in case are ONE
/// entity: the store is keyed by the case-folded name, while the record keeps the casing
/// the caller sent so a reader replays it. Every such name is ASCII (`ExperimentEntityName`
/// is `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`, `AutoMLJobName` the same shape bounded at 32),
/// so ASCII folding is exact for every name the boundary validator lets through.
pub fn entity_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// The entity NAME a member modelled `ExperimentEntityNameOrArn` denotes, or `None` when
/// the value is an ARN that denotes something else.
///
/// Those members accept either the bare name or the entity's ARN, so a reader resolves
/// both forms to one store key rather than keying the store by the string the caller
/// typed. An ARN denotes an entity of THIS service only when it is an ARN this service
/// mints: `arn:aws:sagemaker:<the request's region>:<the emulator's account>:<kind>/<name>`.
/// An ARN naming another service, account, region or resource kind names no entity here —
/// the caller gets the reader's not-found error rather than whatever entity happens to
/// share the ARN's trailing path segment. That validation is why this is stricter than
/// [`name_from_arn_or_name`], which serves the `ArnOrName` readers whose pattern admits
/// one resource kind only: `ExperimentEntityNameOrArn` also admits the ARNs of OTHER
/// entity kinds (experiment, trial, artifact, action, context), so the kind has to be
/// checked rather than assumed.
pub fn entity_name_from_name_or_arn<'v>(
    value: &'v str,
    region: &str,
    kind: &str,
) -> Option<&'v str> {
    if !value.starts_with("arn:") {
        return Some(value);
    }
    // The fixed segments are checked once, by `arn_resource`.
    let name = arn_resource(value, region)?
        .strip_prefix(kind)?
        .strip_prefix('/')?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

// ---------------------------------------------------------------------------
// The experiment-management slice: `CreateExperiment`, `DescribeExperiment` and the
// `DeleteExperiment` that releases one. Kept
// together as one span, the way the slices above are, so this one resource kind reads
// as one block. The store key is [`entity_key`], shared with the experiment-tracking
// slice: an `ExperimentEntityName` is unique per account and not case-sensitive.
// ---------------------------------------------------------------------------

/// The ARN resource kind an experiment carries, as the modelled `ExperimentArn` pattern
/// spells it (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment/`).
///
/// Named once, the way the lineage slices name theirs, so `CreateExperiment`'s ARN
/// minting and the ARN PARSING that resolves an `experiment/...` ARN back to a stored
/// record cannot drift apart.
pub const EXPERIMENT_KIND: &str = "experiment";

/// See [`MODEL_QUOTA`]. Bounds the experiment store the same way, and is what
/// `CreateExperiment` answers `ResourceLimitExceeded` past.
pub const EXPERIMENT_QUOTA: usize = 1_000;

/// An experiment created by `CreateExperiment`, as `DescribeExperiment` replays it.
///
/// `name` keeps the spelling the creating request used; the store's key is
/// [`entity_key`] of that name, so a later create or read that differs only in case
/// resolves to this same record. `created_by` is stored at creation rather than rebuilt
/// per read, so every later read reports the identity that actually created it, and
/// `last_modified_by` starts as that same identity — the creation IS the last
/// modification until `UpdateExperiment` records the identity behind its own call, the
/// way the context slice's does.
#[derive(Debug, Clone)]
pub struct Experiment {
    pub name: String,
    pub arn: String,
    /// What `DescribeExperiment` reports as `DisplayName`: the value the request
    /// supplied, or `ExperimentName` when it supplied none — the model documents that
    /// "if DisplayName isn't specified, ExperimentName is displayed".
    pub display_name: String,
    pub description: Option<String>,
    pub created_by: UserContext,
    /// Who last modified the experiment: the creating identity until a call modifies it.
    pub last_modified_by: UserContext,
    /// The tags the create carried, which `AddTags`/`DeleteTags`/`ListTags` and `Search`
    /// read once those operations are implemented (they are scaffolded stubs today).
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another experiment fits, i.e. the store is below [`EXPERIMENT_QUOTA`].
    pub fn experiments_below_quota(&self) -> bool {
        self.experiments.len() < EXPERIMENT_QUOTA
    }

    /// The experiment `name` identifies, whatever case it is written in — the name is
    /// unique per account and not case-sensitive, so the lookup folds case the way
    /// [`entity_key`] does.
    pub fn experiment(&self, name: &str) -> Option<&Experiment> {
        self.experiments.get(&entity_key(name))
    }
}

// ---------------------------------------------------------------------------
// The custom-image slice: `CreateImage` / `CreateImageVersion`, the `DeleteImage` that
// releases an image with every version of it, and the two
// `Describe*` readers. Kept together at the end of this module, the way the
// lineage-action, model-card and notebook-instance slices are, so one slice's
// records, constants and helpers read as one unit.
// ---------------------------------------------------------------------------

/// How long after `CreateImage` / `CreateImageVersion` the clock owes the
/// `CREATING` -> `CREATED` transition.
///
/// SageMaker AI returns a freshly created image and image version in `CREATING` and
/// moves it to `CREATED` once the registry path is resolved, so a caller that describes
/// straight after creating still observes `CREATING` and one that waits observes
/// `CREATED`. Real SageMaker takes seconds; the span is compressed, never zero.
pub const IMAGE_CREATED_AFTER: Duration = Duration::from_millis(300);

/// See [`MODEL_QUOTA`].
pub const IMAGE_QUOTA: usize = 1_000;

/// See [`MODEL_QUOTA`]: the bound is per image, since versions are numbered within one
/// image and `CreateImageVersion` is the only call that adds one.
pub const IMAGE_VERSION_QUOTA: usize = 1_000;

/// The most aliases one image version carries, after which `CreateImageVersion` is
/// refused with the modelled `ResourceLimitExceeded`.
///
/// `SageMakerImageVersionAliases` is modelled with no length, so nothing at the request
/// boundary bounds the list: without this the request would choose both how much is
/// stored per version and how much work the alias-collision check does. It is the
/// emulator's OWN bound rather than a claim about any account's limit, far above any
/// workflow an emulator serves (a version is normally tagged with a handful of aliases).
pub const MAX_ALIASES_PER_IMAGE_VERSION: usize = 50;

/// The `ImageStatus` / `ImageVersionStatus` members this emulator moves an image and an
/// image version through. Both modelled enums spell the creating and created states the
/// same way (`CREATING`, `CREATED`) and also carry the failure, update and delete states,
/// which this slice does not reach. Named once here so no handler compares a bare string
/// literal.
pub const IMAGE_STATUS_CREATING: &str = "CREATING";
/// See [`IMAGE_STATUS_CREATING`].
pub const IMAGE_STATUS_CREATED: &str = "CREATED";

/// `VendorGuidance`'s member for a version whose maintainer named no stability, which
/// the model documents as "The maintainers did not provide a status for image version
/// stability" — so it is what a version created without `VendorGuidance` reports.
pub const VENDOR_GUIDANCE_NOT_PROVIDED: &str = "NOT_PROVIDED";

/// The message a `ResourceNotFound` carries for an image no store holds: the image's ARN
/// inside the service's sentence, built from the request's `ImageName`.
///
/// Stated once so every refusal that reports a MISSING IMAGE words it the same way —
/// `DescribeImage`, `DeleteImage` and the `CreateImageVersion` / `DescribeImageVersion`
/// that can only resolve a version of an image that exists.
pub fn image_does_not_exist(image_arn: &str) -> String {
    format!("Image with ARN {image_arn} does not exist")
}

/// The message a `ResourceNotFound` carries for an image version the request identifies
/// by its number but no store holds: that version's own ARN inside the sentence this
/// service refuses a missing resource with, the way [`image_does_not_exist`] carries the
/// image's.
///
/// Stated once so every refusal that reports a MISSING IMAGE VERSION words it the same
/// way: the `DescribeImageVersion` reader and the `DeleteImageVersion` that resolves the
/// same `Version`.
pub fn image_version_does_not_exist(version_arn: &str) -> String {
    format!("ImageVersion with ARN {version_arn} does not exist")
}

/// The message a `ResourceNotFound` carries when the image IS stored and the request's
/// selection determines no version ARN — an alias attached to none of its versions, or a
/// request for the latest version of an image that holds none: what could not be resolved
/// beside the image's ARN.
///
/// Stated once so `DescribeImageVersion`, whose alias and latest-version selections this
/// serves, and the `DeleteImageVersion` that resolves the same alias word it the same way.
pub fn image_version_not_resolved(image_arn: &str, selector: &str) -> String {
    format!("Could not find {selector} of image \"{image_arn}\".")
}

/// The ARN a version of `image_name` numbered `version` carries in `region`.
///
/// The modelled `ImageVersionArn` pattern spells the image name in lowercase
/// (`image-version/[a-z0-9]([-.]?[a-z0-9])*/[0-9]+`) where the image ARN's keeps the
/// name's own case, so the name is lowercased for this ARN only. Stated once so the
/// create that mints a version and the readers and the delete that name one in a refusal
/// build the same ARN.
pub fn image_version_arn(region: &str, image_name: &str, version: i32) -> String {
    resource_arn(
        region,
        "image-version",
        &format!("{}/{version}", image_name.to_lowercase()),
    )
}

/// A custom SageMaker AI image created by `CreateImage`, as `DescribeImage` replays it
/// and `CreateImageVersion` reads it.
///
/// `created_at` is the instant the clock owes the `CREATING` -> `CREATED` transition;
/// it is stored, never slept on, and materialised by whichever reporting call first
/// runs at or after it. `next_version` is the number the NEXT version of this image
/// takes: it only ever increases, so a version number — which an image version's ARN
/// carries — is never handed to two different container images.
#[derive(Debug, Clone)]
pub struct Image {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub role_arn: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_at: SystemTime,
    pub next_version: i32,
}

/// A version of an image created by `CreateImageVersion`, as `DescribeImageVersion`
/// replays it.
///
/// `created_at` carries the same owed transition as [`Image::created_at`].
/// `client_token` is the request's `@idempotencyToken`, kept so a retried create
/// answers with the version the first call minted instead of minting a second one.
#[derive(Debug, Clone)]
pub struct ImageVersion {
    pub image_name: String,
    pub image_arn: String,
    pub version: i32,
    pub arn: String,
    pub base_image: String,
    pub client_token: String,
    pub aliases: Vec<String>,
    pub vendor_guidance: String,
    pub job_type: Option<String>,
    pub ml_framework: Option<String>,
    pub programming_lang: Option<String>,
    pub processor: Option<String>,
    pub horovod: Option<bool>,
    pub release_notes: Option<String>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_at: SystemTime,
}

/// Materialise the `CREATING` -> `CREATED` transition the clock already owes to the
/// record whose `status` is `*status` and whose transition was due at `created_at`.
///
/// This is the one write a reporting operation may make: the transition was due
/// whether or not anybody described the resource, so the observer makes no difference
/// to the result. `LastModifiedTime` moves to the instant the transition was due, not
/// to "now", so the value is stable across later reads.
///
/// Answers whether the transition happened on this call, so the caller can record it.
fn settle_created(
    status: &mut String,
    last_modified_time: &mut SystemTime,
    created_at: SystemTime,
    now: SystemTime,
) -> bool {
    if status == IMAGE_STATUS_CREATING && now >= created_at {
        *status = IMAGE_STATUS_CREATED.to_string();
        *last_modified_time = created_at;
        return true;
    }
    false
}

impl Image {
    /// See [`settle_created`].
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        settle_created(
            &mut self.status,
            &mut self.last_modified_time,
            self.created_at,
            now,
        )
    }
}

impl ImageVersion {
    /// See [`settle_created`].
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        settle_created(
            &mut self.status,
            &mut self.last_modified_time,
            self.created_at,
            now,
        )
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another image fits, i.e. the store is below [`IMAGE_QUOTA`].
    pub fn images_below_quota(&self) -> bool {
        self.images.len() < IMAGE_QUOTA
    }

    /// Every stored version of `image_name`, in ascending version order.
    pub fn versions_of<'store>(
        &'store self,
        image_name: &str,
    ) -> impl DoubleEndedIterator<Item = &'store ImageVersion> + 'store {
        self.image_versions
            .range((image_name.to_string(), i32::MIN)..=(image_name.to_string(), i32::MAX))
            .map(|(_, version)| version)
    }

    /// Whether another version of `image_name` fits, i.e. the image holds fewer than
    /// [`IMAGE_VERSION_QUOTA`] of them.
    pub fn image_versions_below_quota(&self, image_name: &str) -> bool {
        self.versions_of(image_name).count() < IMAGE_VERSION_QUOTA
    }

    /// The highest-numbered stored version of `image_name` — what
    /// `DescribeImageVersion` describes when the request names neither a version nor an
    /// alias.
    pub fn latest_version_of(&self, image_name: &str) -> Option<&ImageVersion> {
        self.versions_of(image_name).next_back()
    }

    /// Release every stored version of `image_name`, answering how many were released.
    ///
    /// `DeleteImage` deletes an image AND all versions of it (its own model doc), so the
    /// image and its versions leave the store in one call: a version left behind would be
    /// replayed by a later reader for an image no store holds any more, and a later
    /// `CreateImage` under the same name would inherit it.
    pub fn remove_versions_of(&mut self, image_name: &str) -> usize {
        let keys: Vec<(String, i32)> = self
            .versions_of(image_name)
            .map(|version| (version.image_name.clone(), version.version))
            .collect();
        let removed = keys.len();
        for key in keys {
            self.image_versions.remove(&key);
        }
        removed
    }

    /// The ARN of a Studio app launched on the image `image_name` names — its
    /// `ResourceSpec` names either the image itself or one of the image's versions — or
    /// `None` when no stored app does.
    ///
    /// An app keeps the spec it launched with (`CreateApp` stores it verbatim), so an app
    /// in the store is one still running on that image: releasing the image would leave
    /// the app naming an image nothing holds.
    pub fn app_running_on_image(&self, image_name: &str) -> Option<&str> {
        let image_arn = self.images.get(image_name).map(|image| image.arn.as_str())?;
        self.apps
            .values()
            .find_map(|app| {
                let spec = app.resource_spec.as_ref()?;
                (spec.sage_maker_image_arn.as_deref() == Some(image_arn))
                    .then_some(app.arn.as_str())
            })
            .or_else(|| {
                self.versions_of(image_name)
                    .find_map(|version| self.app_running_on_image_version(&version.arn))
            })
    }

    /// The ARN of a Studio app launched on the image VERSION whose ARN is `version_arn`,
    /// or `None` when no stored app names it.
    ///
    /// An app keeps the spec it launched with (`CreateApp` stores it verbatim), so an app
    /// in the store is one still running on that version: releasing the version would
    /// leave the app naming a version nothing holds. This is what `DeleteImageVersion`
    /// refuses on, and the version half of what [`Self::app_running_on_image`] answers.
    pub fn app_running_on_image_version(&self, version_arn: &str) -> Option<&str> {
        self.apps.values().find_map(|app| {
            let spec = app.resource_spec.as_ref()?;
            (spec.sage_maker_image_version_arn.as_deref() == Some(version_arn))
                .then_some(app.arn.as_str())
        })
    }
}

// ---------------------------------------------------------------------------
// The lineage-association slice: `AddAssociation` and the `DeleteAssociation` that
// releases one. Kept together at the end of this module, the way the lineage-action,
// model-card and custom-image slices are, so one slice's record, constants and helpers
// read as one unit.
// ---------------------------------------------------------------------------

/// The `<kind>` component of an `artifact/<id>` ARN, i.e. the kind `CreateArtifact`
/// mints and the one an `AssociationEntityArn` names an artifact by. Named here so the
/// create and the association slice mint and resolve the same kind.
pub const ARTIFACT_KIND: &str = "artifact";

/// The `<kind>` component of an `action/<name>` ARN. See [`ARTIFACT_KIND`].
pub const ACTION_KIND: &str = "action";

/// See [`MODEL_QUOTA`] for why the bound is the emulator's OWN rather than a claim about
/// any account's limit. It bounds the association store the same way, and is what
/// `AddAssociation` answers `ResourceLimitExceeded` past. It is higher than the
/// per-resource bounds because an association is an EDGE: the store grows with the PAIRS
/// of lineage entities a caller connects, not with the entities themselves, and every
/// entry it holds is released by `DeleteAssociation`.
pub const ASSOCIATION_QUOTA: usize = 10_000;

/// How SageMaker refuses the delete of a lineage entity that is still the SOURCE of an
/// association the account holds.
///
/// Entirely FIXED text: the recorded refusal interpolates nothing — no entity name, ARN or
/// edge count — so the sentence is stated once here and every lineage-entity delete words
/// the referential-integrity refusal the same way. The caller clears the condition itself
/// with `DeleteAssociation`, so this is the 400 `ValidationException` rather than an error
/// any of those deletes models.
pub const ENTITY_WITH_ASSOCIATIONS: &str = "Cannot delete entity with associations.";

/// One lineage association created by `AddAssociation`, as `DeleteAssociation` releases
/// it.
///
/// The `(source_arn, destination_arn)` pair is the association's identity — SageMaker
/// mints no id for an association and no operation addresses one by anything else — so
/// the pair is the store key and these two fields repeat it, the way every other record
/// here keeps the identifier it is filed under. `association_type` stays an `Option`
/// because `AssociationType` is an optional member: an association created without one
/// carries none, which is distinguishable on the wire from any of the modelled edge
/// types. `creation_time` is the instant the association was FIRST added, so a repeated
/// add does not pretend the edge is new, and `created_by` is stored beside it by the call
/// that first added the edge — as every other record here stores the identity that
/// created it — so a later read reports the identity that really created this one.
#[derive(Debug, Clone)]
pub struct Association {
    pub source_arn: String,
    pub destination_arn: String,
    pub association_type: Option<String>,
    pub created_by: UserContext,
    pub creation_time: SystemTime,
}

impl Association {
    /// This association's store key as ONE string, for the page token a listing mints.
    ///
    /// The identity is the `(source, destination)` pair, so the token has to name both; a
    /// space joins them because no ARN this service mints contains one (the resource
    /// segment is a lineage name or a minted id), so the pair is recoverable and two
    /// different pairs can never spell the same key. The same precedent as
    /// [`ModelPackage::key`], which spells a two-part identity as one key too.
    pub fn key(&self) -> String {
        format!("{} {}", self.source_arn, self.destination_arn)
    }

    /// Whether this edge belongs to `region`, i.e. both of its ends are ARNs this service
    /// mints there.
    ///
    /// The association store is account-wide, while every lineage entity is REGIONAL — its
    /// ARN names the Region it was created in — so a listing reports the edges of the
    /// Region the request signed for and never one between entities of another.
    pub fn in_region(&self, region: &str) -> bool {
        arn_resource(&self.source_arn, region).is_some()
            && arn_resource(&self.destination_arn, region).is_some()
    }
}

/// What a reader reports about ONE END of an association: the lineage entity's own name
/// and type, read out of the record the matching `Create*` stored.
///
/// `name` is an `Option` because an artifact need not have one (`ArtifactName` is
/// optional), and `entity_type` because only three of the five entity kinds an
/// `AssociationEntityArn` admits carry a type of their own — an artifact's `ArtifactType`,
/// an action's `ActionType`, a context's `ContextType`. An experiment and a trial
/// component take theirs from the `Source` of the job that emitted them, which
/// `CreateExperiment` and `CreateTrialComponent` do not accept, so one created directly
/// has no type to report and the member stays absent rather than being filled with an
/// invented value.
#[derive(Debug, Clone, Default)]
pub struct LineageEntityEnd {
    pub name: Option<String>,
    pub entity_type: Option<String>,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another association fits, i.e. the store is below [`ASSOCIATION_QUOTA`].
    pub fn associations_below_quota(&self) -> bool {
        self.associations.len() < ASSOCIATION_QUOTA
    }

    /// Whether the lineage entity `arn` names is still the SOURCE of an association the
    /// account holds, i.e. an edge the graph records starts at it.
    ///
    /// The referential-integrity question every lineage-entity delete asks before releasing
    /// its record ([`ENTITY_WITH_ASSOCIATIONS`]): an entity deleted out from under an edge
    /// that starts at it would leave `ListAssociations` reporting an origin no reader can
    /// describe. The association store is keyed by the `(SourceArn, DestinationArn)` pair,
    /// so the source side is the key's first component and this asks whether any stored key
    /// carries `arn` there — the one place the question is asked, so the deletes of the five
    /// entity kinds an `AssociationEntityArn` admits cannot answer it differently.
    pub fn is_association_source(&self, arn: &str) -> bool {
        self.associations
            .keys()
            .any(|(source_arn, _)| source_arn == arn)
    }

    /// Whether `arn` names a lineage entity this service holds, i.e. one an association
    /// may point at.
    ///
    /// `AssociationEntityArn`'s modelled pattern admits five entity kinds — experiment,
    /// experiment-trial-component, artifact, action and context. Each is resolved against
    /// the store the matching create writes to, through
    /// [`entity_name_from_name_or_arn`], so an ARN naming another partition, service,
    /// Region or account names no entity here. All five kinds now have an implemented
    /// create, so each is decided by whether its own store holds the ARN rather than by
    /// the kind being unreachable.
    pub fn lineage_entity_exists(&self, region: &str, arn: &str) -> bool {
        self.lineage_entity_end(region, arn).is_some()
    }

    /// The name and type of the lineage entity `arn` names, or `None` when it names none
    /// this service holds.
    ///
    /// The resolution side of [`Self::lineage_entity_exists`], which is this returning
    /// `Some`: one walk over the five entity kinds, so "is this an end an association may
    /// have" and "what does a reader report about that end" can never disagree.
    pub fn lineage_entity_end(&self, region: &str, arn: &str) -> Option<LineageEntityEnd> {
        // Every `AssociationEntityArn` IS an ARN (its pattern starts `^arn:`), so the
        // bare-name branch of `entity_name_from_name_or_arn` is unreachable from an
        // association request; guarded rather than assumed, so a bare name can never
        // resolve to an entity through this helper.
        if !arn.starts_with("arn:") {
            return None;
        }
        if entity_name_from_name_or_arn(arn, region, ARTIFACT_KIND).is_some() {
            // Both lineage stores are keyed by the WHOLE ARN the create minted, which
            // already carries this request's Region and the emulator's account.
            return self.artifacts.get(arn).map(|artifact| LineageEntityEnd {
                name: artifact.name.clone(),
                entity_type: Some(artifact.artifact_type.clone()),
            });
        }
        if entity_name_from_name_or_arn(arn, region, ACTION_KIND).is_some() {
            return self.actions.get(arn).map(|action| LineageEntityEnd {
                name: Some(action.name.clone()),
                entity_type: Some(action.action_type.clone()),
            });
        }
        if entity_name_from_name_or_arn(arn, region, CONTEXT_KIND).is_some() {
            // The context store is keyed by the whole ARN too (see [`context_key`]), so a
            // context `CreateContext` really stored is an end an association may point at.
            return self.contexts.get(arn).map(|context| LineageEntityEnd {
                name: Some(context.name.clone()),
                entity_type: Some(context.context_type.clone()),
            });
        }
        if let Some(name) = entity_name_from_name_or_arn(arn, region, TRIAL_COMPONENT_KIND) {
            // The trial-component store is keyed by the case-folded NAME (the name "is
            // not case-sensitive" per its model doc), so the lookup goes through the same
            // key `CreateTrialComponent` stored the record under.
            return self
                .trial_components
                .get(&entity_key(name))
                .map(|component| LineageEntityEnd {
                    name: Some(component.name.clone()),
                    entity_type: None,
                });
        }
        if let Some(name) = entity_name_from_name_or_arn(arn, region, EXPERIMENT_KIND) {
            // Same keying as the trial-component store, and for the same reason: an
            // `ExperimentEntityName` is unique per account and not case-sensitive, so
            // `CreateExperiment` filed the record under [`entity_key`] of the name.
            return self
                .experiments
                .get(&entity_key(name))
                .map(|experiment| LineageEntityEnd {
                    name: Some(experiment.name.clone()),
                    entity_type: None,
                });
        }
        None
    }
}

// ---------------------------------------------------------------------------
// The HyperPod scheduling, experiment-trial, model-card-export and partner-app
// control-plane slice's own lookups and store bounds. They sit in their OWN inherent
// `impl` block at the end of this module — the convention the slices above follow —
// rather than in the first one, so a slice added beside them edits its own block
// instead of the one every other slice also edits.
// ---------------------------------------------------------------------------

impl ComAmazonawsSagemakerSagemakerState {
    /// The cluster a `ClusterNameOrArn` member names: by ARN when the value is one, by
    /// name otherwise — the two forms the model accepts for the one resource.
    /// A cluster whose deletion the clock already completed answers `None` here, as it
    /// does from every other cluster lookup ([`Cluster::is_released`]).
    pub fn cluster_by_name_or_arn(&self, value: &str) -> Option<&Cluster> {
        let now = SystemTime::now();
        if value.starts_with(ARN_PREFIX) {
            return self
                .clusters
                .values()
                .find(|cluster| cluster.arn == value)
                .filter(|cluster| !cluster.is_released(now));
        }
        self.clusters
            .get(value)
            .filter(|cluster| !cluster.is_released(now))
    }

    /// [`Self::cluster_by_name_or_arn`], for a caller that writes.
    pub fn cluster_by_name_or_arn_mut(&mut self, value: &str) -> Option<&mut Cluster> {
        let now = SystemTime::now();
        if value.starts_with(ARN_PREFIX) {
            return self
                .clusters
                .values_mut()
                .find(|cluster| cluster.arn == value)
                .filter(|cluster| !cluster.is_released(now));
        }
        self.clusters
            .get_mut(value)
            .filter(|cluster| !cluster.is_released(now))
    }

    /// [`Self::cluster_by_arn`], for a caller that writes.
    pub fn cluster_by_arn_mut(&mut self, value: &str) -> Option<&mut Cluster> {
        let now = SystemTime::now();
        self.clusters
            .values_mut()
            .find(|cluster| cluster.arn == value)
            .filter(|cluster| !cluster.is_released(now))
    }

    /// The partner app whose ARN is `value`.
    pub fn partner_app_by_arn_mut(&mut self, value: &str) -> Option<&mut PartnerApp> {
        self.partner_apps
            .values_mut()
            .find(|app| app.arn == value)
    }

    /// Whether this account holds a partner-application subscription for `app_type`, which
    /// is what `CreatePartnerApp` admits a `Type` by.
    ///
    /// A partner AI app runs a third party's licensed application: the account subscribes
    /// to it in AWS Marketplace, which is a purchase outside SageMaker's API surface — no
    /// operation this service models adds to this set. So an account that has subscribed to
    /// nothing holds none, and every type is refused ([`partner_app_not_subscribed`]) until
    /// a subscription for it is held.
    pub fn subscribed_to_partner_app(&self, app_type: &str) -> bool {
        self.partner_app_subscriptions.contains(app_type)
    }

    /// Whether another partner app fits, i.e. the store is below [`PARTNER_APP_QUOTA`].
    pub fn partner_apps_below_quota(&self) -> bool {
        self.partner_apps.len() < PARTNER_APP_QUOTA
    }

    /// The partner app this account holds under `name`, the one identity a caller chooses
    /// for an app (its id and ARN are SageMaker's to mint).
    ///
    /// ACCOUNT-WIDE rather than per Region, which is the scope
    /// [`Self::partner_apps`] has: the store is keyed by the app id alone and
    /// `ListPartnerApps` pages all of it, so a name is unique across everything a caller
    /// of this emulator can see. Narrowing both to the Region of the app's ARN is one
    /// change covering the listing as well, not a property of the create alone.
    pub fn partner_app_named(&self, name: &str) -> Option<&PartnerApp> {
        self.partner_apps.values().find(|app| app.name == name)
    }

    /// The partner app a create presenting `token` already minted, so the retry of that
    /// create answers the app it made rather than a second one.
    pub fn partner_app_for_client_token(&self, token: &str) -> Option<&PartnerApp> {
        self.partner_apps
            .values()
            .find(|app| app.client_token.as_deref() == Some(token))
    }

    /// Whether another trial fits, i.e. the store is below [`TRIAL_QUOTA`].
    pub fn trials_below_quota(&self) -> bool {
        self.trials.len() < TRIAL_QUOTA
    }

    /// The trial `name` identifies, whatever case it is written in — `TrialName` is
    /// unique per account and not case-sensitive, so the lookup folds case the way
    /// [`Self::experiment`] does for an experiment.
    pub fn trial(&self, name: &str) -> Option<&Trial> {
        self.trials.get(&entity_key(name))
    }

    /// Whether another compute quota fits, i.e. the store is below [`COMPUTE_QUOTA_QUOTA`].
    pub fn compute_quotas_below_quota(&self) -> bool {
        self.compute_quotas.len() < COMPUTE_QUOTA_QUOTA
    }
}

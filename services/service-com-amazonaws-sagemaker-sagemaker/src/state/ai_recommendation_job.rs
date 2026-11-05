// SPDX-License-Identifier: CC0-1.0

//! The AI recommendation job slice: what `CreateAIRecommendationJob` writes,
//! `DescribeAIRecommendationJob` replays and `DeleteAIRecommendationJob` releases, beside
//! the AI workload configurations the jobs reference (kept in [`super`], with the rest of
//! the AI-benchmarking control plane).
//!
//! It lives in its own file rather than at the end of [`super`] so this one resource kind
//! reads as one unit: the record, the ARN kind and quota that identify and bound it, the
//! statuses it moves through, the span the clock owes that move at, and the reason a run
//! nothing performed ends with.

use std::time::{Duration, SystemTime};

use crate::ops::types::Tag;

use super::ComAmazonawsSagemakerSagemakerState;

/// The ARN resource kind an AI recommendation job carries, i.e. the `<kind>` in
/// [`super::resource_arn`]. Fixed by `AIRecommendationJobArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:ai-recommendation-job/...`), and
/// named once here so the create, the read and the delete mint and match the same ARN.
pub const AI_RECOMMENDATION_JOB_KIND: &str = "ai-recommendation-job";

/// See [`super::MODEL_QUOTA`]. Bounds the AI recommendation job store the same way, and is
/// what `CreateAIRecommendationJob` answers `ResourceLimitExceeded` past.
pub const AI_RECOMMENDATION_JOB_QUOTA: usize = 1_000;

/// `AIRecommendationJobStatus`'s members this emulator moves a recommendation job
/// through (`smithy_query(query="shape",shape="AIRecommendationJobStatus")` answers
/// `InProgress`, `Completed`, `Failed`, `Stopping` and `Stopped`; `Completed` is not
/// reached, because nothing here analyses a model). A separate set from
/// [`super::AI_BENCHMARK_STATUS_IN_PROGRESS`], which names `AIBenchmarkJobStatus`'s
/// members, so neither enum's members are compared against the other's.
pub const AI_RECOMMENDATION_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`AI_RECOMMENDATION_STATUS_IN_PROGRESS`].
pub const AI_RECOMMENDATION_STATUS_FAILED: &str = "Failed";
/// See [`AI_RECOMMENDATION_STATUS_IN_PROGRESS`]: the transient status
/// `StopAIRecommendationJob` moves a running job into.
pub const AI_RECOMMENDATION_STATUS_STOPPING: &str = "Stopping";
/// See [`AI_RECOMMENDATION_STATUS_IN_PROGRESS`]: the terminal status the clock owes a
/// job the caller asked to stop.
pub const AI_RECOMMENDATION_STATUS_STOPPED: &str = "Stopped";

/// The lifecycle state a job that has been accepted but whose run has not STARTED is in,
/// as the service that holds the job names it in [`ai_recommendation_job_not_deletable`].
///
/// It is not a member of `AIRecommendationJobStatus`
/// (`smithy_query(query="shape",shape="AIRecommendationJobStatus")` answers `InProgress`,
/// `Completed`, `Failed`, `Stopping` and `Stopped`), whose `InProgress` covers a job that
/// is queued as well as one that is running: the two vocabularies are distinct, so a
/// reported `AIRecommendationJobStatus` is never compared against this value and this
/// value is never reported as one.
pub const AI_RECOMMENDATION_STATE_PENDING: &str = "Pending";

/// How long after `CreateAIRecommendationJob` the clock owes the job's move out of
/// `InProgress` into its terminal status.
///
/// A span chosen by this emulator (AWS documents no duration for a recommendation run),
/// inside the 250–750 ms band the project fixes for a transition the clock owes. It is
/// never zero, so a caller that describes the job straight after creating it still
/// observes `InProgress`, and one that waits observes the terminal status, so a waiter
/// terminates.
pub const AI_RECOMMENDATION_JOB_ENDS_AFTER: Duration = Duration::from_millis(400);

/// How long after `StopAIRecommendationJob` the clock owes the job's `Stopping` ->
/// `Stopped` move.
///
/// Chosen on the same terms as [`AI_RECOMMENDATION_JOB_ENDS_AFTER`] (AWS documents no
/// duration for the stop either) and inside the same band: never zero, so a caller that
/// describes the job straight after asking it to stop still observes the transient
/// `Stopping`, and one that waits observes `Stopped`, so a waiter terminates.
pub const AI_RECOMMENDATION_JOB_STOPS_AFTER: Duration = Duration::from_millis(300);

/// What `OptimizeModel` means when a `CreateAIRecommendationJob` request omits it:
/// "Whether to allow model optimization techniques such as quantization, speculative
/// decoding, and kernel tuning. The default is true."
/// (`smithy_query(query="doc",shape="CreateAIRecommendationJobRequest",member="OptimizeModel")`).
/// The create stores the EFFECTIVE value, so `DescribeAIRecommendationJob`'s
/// "whether model optimization techniques were allowed" reports what the job actually
/// ran under rather than leaving the member absent.
pub const AI_RECOMMENDATION_OPTIMIZE_MODEL_DEFAULT: bool = true;

/// `FailureReason` of a recommendation job whose run nothing could perform here.
///
/// A recommendation job loads the model artifacts, runs the inference framework on each
/// candidate instance type and measures latency, throughput and cost before it can
/// recommend anything. This emulator serves no inference data plane and runs no
/// optimization or benchmarking tool, and the engine contract
/// (`vendor/engine/src/lib.rs`) forbids reporting a status, duration or measurement for
/// work nothing ran — so the run the caller asked for ends in the modelled `Failed`
/// state with this reason, and no `StartTime`, `Completed` status or `Recommendations`
/// list is invented. Everything around the run — accepting the job, resolving its
/// workload configuration, minting its ARN, replaying it, reporting its terminal status,
/// releasing it — is real.
pub const NO_RECOMMENDATION_RUNTIME_REASON: &str =
    "This emulator serves no inference data plane and runs no model optimization or \
     benchmarking tool, so the recommendation job's run was never started";

/// Why a reader or the stop could not resolve an AI recommendation job, named
/// once here so `DescribeAIRecommendationJob` and `StopAIRecommendationJob` refuse an
/// unknown name with the same sentence: the AI family's [`super::ai_entity_not_found`],
/// quoting the name the request presented. `DeleteAIRecommendationJob` relays the inner
/// service's rendering of it instead ([`super::ai_entity_not_found_relayed`]).
pub fn ai_recommendation_job_not_found(name: &str) -> String {
    super::ai_entity_not_found(AI_RECOMMENDATION_JOB_ENTITY, name)
}

/// The entity label the AI family's not-found sentence names a recommendation job by — the
/// bare kind, as the service words this one, where its sibling resources are named by their
/// API spelling.
pub const AI_RECOMMENDATION_JOB_ENTITY: &str = "Job";

/// Why a stop of a job that is no longer running is refused, naming the job and the
/// status it had already reached.
///
/// `StopAIRecommendationJob` stops "a running AI recommendation job"
/// (`smithy_query(query="doc",shape="StopAIRecommendationJob")`), so a job that has
/// already reached a status nothing leaves on its own has no run to stop. The operation
/// models only `ResourceNotFound`, and the job IS found, so the refusal is a 400
/// `ValidationException` carrying this sentence.
pub fn ai_recommendation_job_not_running(name: &str, status: &str) -> String {
    format!(
        "The AI recommendation job \"{name}\" is {status} and is not running, so there is \
         nothing to stop."
    )
}

/// Why a delete of a job that is still running is refused, naming the state that stands
/// in the way.
///
/// `DeleteAIRecommendationJob` releases a job, and a job whose run has not finished is
/// still holding the work the caller asked for: SageMaker refuses the delete, names the
/// state the job is in and points at the stop that ends it, rather than destroying a
/// running job. The state is the one the service that holds the job names — see
/// [`AIRecommendationJob::deletion_state`] — not a member of the reported
/// `AIRecommendationJobStatus`. The operation models only `ResourceNotFound`, and the job
/// IS found, so
/// the refusal is a 400 `ValidationException` carrying this sentence.
pub fn ai_recommendation_job_not_deletable(status: &str) -> String {
    format!("Cannot delete job in '{status}' state. Stop the job first.")
}

/// An AI recommendation job created by `CreateAIRecommendationJob`, as
/// `DescribeAIRecommendationJob` replays it.
///
/// `ends_at` is the instant the clock owes the move out of `InProgress`; it is stored,
/// never slept on, and materialised by whichever reporting call first runs at or after
/// it (see [`AIRecommendationJob::settle_status`]).
#[derive(Debug, Clone)]
pub struct AIRecommendationJob {
    pub name: String,
    pub arn: String,
    /// The `AIWorkloadConfigIdentifier` the request presented, kept VERBATIM (a bare
    /// name or the configuration's ARN) so the read replays what the caller sent rather
    /// than a form it did not use.
    pub workload_config_identifier: String,
    pub model_source: crate::ops::types::AIModelSource,
    /// What the request's `OutputConfig` carried. `DescribeAIRecommendationJob` reports
    /// it as an `AIRecommendationOutputResult`, whose `S3OutputLocation` is required
    /// while the create's is optional — and `S3Uri`'s modelled pattern admits no empty
    /// value, so `CreateAIRecommendationJob` REFUSES a request that names no output
    /// location (a 400 `ValidationException`) rather than storing a job it could only
    /// report with a value the model forbids. Every stored job therefore carries one.
    pub output_config: crate::ops::types::AIRecommendationOutputConfig,
    pub performance_target: crate::ops::types::AIRecommendationPerformanceTarget,
    pub inference_specification: Option<crate::ops::types::AIRecommendationInferenceSpecification>,
    pub compute_spec: Option<crate::ops::types::AIRecommendationComputeSpec>,
    pub adapter_source: Option<crate::ops::types::AIAdapterSource>,
    /// The EFFECTIVE `OptimizeModel`, i.e. the request's value or
    /// [`AI_RECOMMENDATION_OPTIMIZE_MODEL_DEFAULT`] when it named none.
    pub optimize_model: bool,
    pub role_arn: String,
    pub tags: Vec<Tag>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub ends_at: SystemTime,
}

impl AIRecommendationJob {
    /// Materialise the move out of a non-terminal status the clock already owes.
    ///
    /// The one write a reporting operation may make, exactly as
    /// [`super::AIBenchmarkJob::settle_status`] does it: the move was due at `ends_at`
    /// whether or not anybody described the job, so the observer makes no difference to
    /// the result, and `EndTime` takes the instant the move was due rather than "now", so
    /// it is stable across later reads.
    ///
    /// A job left to run ends in [`AI_RECOMMENDATION_STATUS_FAILED`] with
    /// [`NO_RECOMMENDATION_RUNTIME_REASON`]: nothing analysed the model, so no
    /// `Completed` status, no `StartTime` and no recommendation is invented for it. A job
    /// the caller asked to stop ends in [`AI_RECOMMENDATION_STATUS_STOPPED`] with no
    /// failure reason instead: stopping is what the caller asked for, not a failure.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if now < self.ends_at {
            return false;
        }
        match self.status.as_str() {
            AI_RECOMMENDATION_STATUS_IN_PROGRESS => {
                self.status = AI_RECOMMENDATION_STATUS_FAILED.to_string();
                self.failure_reason = Some(NO_RECOMMENDATION_RUNTIME_REASON.to_string());
            }
            AI_RECOMMENDATION_STATUS_STOPPING => {
                self.status = AI_RECOMMENDATION_STATUS_STOPPED.to_string();
            }
            // Already terminal: nothing is owed and nothing moves.
            _ => return false,
        }
        // The instant the run ended, which is when the move was due rather than "now", so
        // it is stable across later reads. No start time is recorded beside it: the run
        // never started.
        self.end_time = Some(self.ends_at);
        true
    }

    /// Whether the job has reached a status nothing leaves on its own, i.e. one of
    /// `AIRecommendationJobStatus`'s terminal members.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            AI_RECOMMENDATION_STATUS_FAILED
                | AI_RECOMMENDATION_STATUS_STOPPED
                | super::STATUS_COMPLETED
        )
    }

    /// Whether a stop the caller already asked for is still in flight, i.e. the job sits
    /// in the transient `Stopping` status.
    pub fn is_stopping(&self) -> bool {
        self.status == AI_RECOMMENDATION_STATUS_STOPPING
    }

    /// The lifecycle state [`ai_recommendation_job_not_deletable`] names this job by.
    ///
    /// A stop in flight is the state it names ([`AI_RECOMMENDATION_STATUS_STOPPING`]);
    /// otherwise the job is QUEUED — nothing here analyses a model, so no run of a job in
    /// this store is ever started (which is why it reports no `StartTime` and ends with
    /// [`NO_RECOMMENDATION_RUNTIME_REASON`]), and the state of a job whose run has not
    /// started is [`AI_RECOMMENDATION_STATE_PENDING`].
    pub fn deletion_state(&self) -> &'static str {
        if self.is_stopping() {
            AI_RECOMMENDATION_STATUS_STOPPING
        } else {
            AI_RECOMMENDATION_STATE_PENDING
        }
    }

    /// Record that the caller asked this job to stop: it moves to the transient
    /// `Stopping` now, and the clock owes it `Stopped` at
    /// [`AI_RECOMMENDATION_JOB_STOPS_AFTER`]. The span is never zero, so the transient
    /// status is observable.
    pub fn request_stop(&mut self, now: SystemTime) {
        self.status = AI_RECOMMENDATION_STATUS_STOPPING.to_string();
        // The run is being abandoned on request, not failing, so any reason a previous
        // status carried is cleared rather than reported beside `Stopped`.
        self.failure_reason = None;
        self.end_time = None;
        self.ends_at = now + AI_RECOMMENDATION_JOB_STOPS_AFTER;
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another AI recommendation job fits, i.e. the store is below
    /// [`AI_RECOMMENDATION_JOB_QUOTA`].
    pub fn ai_recommendation_jobs_below_quota(&self) -> bool {
        self.ai_recommendation_jobs.len() < AI_RECOMMENDATION_JOB_QUOTA
    }
}

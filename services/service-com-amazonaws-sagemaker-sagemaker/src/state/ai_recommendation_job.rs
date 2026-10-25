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
/// `InProgress`, `Completed`, `Failed`, `Stopping` and `Stopped`; the `Completed` and
/// stop states are not reached by creating one). A separate pair from
/// [`super::AI_BENCHMARK_STATUS_IN_PROGRESS`], which names `AIBenchmarkJobStatus`'s
/// members, so neither enum's members are compared against the other's.
pub const AI_RECOMMENDATION_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`AI_RECOMMENDATION_STATUS_IN_PROGRESS`].
pub const AI_RECOMMENDATION_STATUS_FAILED: &str = "Failed";

/// How long after `CreateAIRecommendationJob` the clock owes the job's move out of
/// `InProgress` into its terminal status.
///
/// A span chosen by this emulator (AWS documents no duration for a recommendation run),
/// inside the 250–750 ms band the project fixes for a transition the clock owes. It is
/// never zero, so a caller that describes the job straight after creating it still
/// observes `InProgress`, and one that waits observes the terminal status, so a waiter
/// terminates.
pub const AI_RECOMMENDATION_JOB_ENDS_AFTER: Duration = Duration::from_millis(400);

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

/// Why a reader or the delete could not resolve an AI recommendation job, named once here
/// so `DescribeAIRecommendationJob` and `DeleteAIRecommendationJob` refuse an unknown
/// name with the same sentence, naming the ARN they could not resolve.
pub fn ai_recommendation_job_not_found(arn: &str) -> String {
    format!("Could not find AI recommendation job \"{arn}\".")
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
    /// Materialise the move out of `InProgress` the clock already owes.
    ///
    /// The one write a reporting operation may make, exactly as
    /// [`super::AIBenchmarkJob::settle_status`] does it: the move was due at `ends_at`
    /// whether or not anybody described the job, so the observer makes no difference to
    /// the result, and `EndTime` takes the instant the move was due rather than "now", so
    /// it is stable across later reads.
    ///
    /// The terminal status is [`AI_RECOMMENDATION_STATUS_FAILED`] with
    /// [`NO_RECOMMENDATION_RUNTIME_REASON`]: nothing analysed the model, so no
    /// `Completed` status, no `StartTime` and no recommendation is invented for it.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == AI_RECOMMENDATION_STATUS_IN_PROGRESS && now >= self.ends_at {
            self.status = AI_RECOMMENDATION_STATUS_FAILED.to_string();
            self.failure_reason = Some(NO_RECOMMENDATION_RUNTIME_REASON.to_string());
            // The instant the service gave up on the run, which is when the job ended.
            // No start time is recorded beside it: the run never started.
            self.end_time = Some(self.ends_at);
            return true;
        }
        false
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another AI recommendation job fits, i.e. the store is below
    /// [`AI_RECOMMENDATION_JOB_QUOTA`].
    pub fn ai_recommendation_jobs_below_quota(&self) -> bool {
        self.ai_recommendation_jobs.len() < AI_RECOMMENDATION_JOB_QUOTA
    }
}

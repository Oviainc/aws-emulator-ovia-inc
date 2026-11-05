// SPDX-License-Identifier: CC0-1.0

//! The AI-benchmark-job slice of this service's state: the [`AIBenchmarkJob`] record,
//! its bound, its statuses and the clock-owed end every reader materialises.

use super::*;

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

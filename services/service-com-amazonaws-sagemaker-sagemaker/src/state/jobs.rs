// SPDX-License-Identifier: CC0-1.0

//! The JOB slice of this service's state: the records a training, hyper-parameter
//! tuning, trial, processing or inference-recommendations job keeps, with the clock
//! spans and statuses that belong to them.
//!
//! Moved out of [`super`] unchanged, so the parent module stays inside the project's
//! file-length limit and each job record reads beside the clock that moves it. Every
//! name is re-exported from [`super`], so a handler keeps naming `crate::state::…`.

use super::*;

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

// SPDX-License-Identifier: CC0-1.0

//! The AutoML job V2 slice of this service's state: what `CreateAutoMLJobV2` persists
//! and `DescribeAutoMLJobV2` replays.
//!
//! A child module of [`crate::state`] rather than another map beside it: the record,
//! the constants that bound and word it and the derived `AutoMLProblemTypeConfigName`
//! live here, while the ONE state struct keeps the single
//! [`auto_ml_jobs_v2`](super::ComAmazonawsSagemakerSagemakerState::auto_ml_jobs_v2) map
//! every AutoML operation reads, and [`crate::state`] re-exports this module's items so
//! a handler names them as `crate::state::…` like the rest of the slice.
//!
//! # The job's status is moved by the CLOCK, never by a read
//!
//! AWS returns an accepted AutoML job in the non-terminal `InProgress` status, so
//! leaving it is owed by the clock: the due instant is stored at create time and
//! [`AutoMLJobV2::settle_status`] only materialises what is already due, so a read never
//! changes what a later caller sees and the transient status is observable because the
//! span is never zero.

use std::time::{Duration, SystemTime};

// The modelled shapes this slice keeps, reached through the parent module's import list
// rather than imported again here: `crate::state`'s `use crate::ops::types::{…}` is the
// list every concurrent change to this service edits, so naming these through `super`
// leaves it exactly as it was.
use super::{
    AutoMLComputeConfig, AutoMLDataSplitConfig, AutoMLJobChannel, AutoMLJobObjective,
    AutoMLOutputDataConfig, AutoMLProblemTypeConfig, AutoMLSecurityConfig, ModelDeployConfig, Tag,
};

/// See [`super::MODEL_QUOTA`]. AutoML jobs are not deleted through the API, so this is
/// what bounds the job store: a caller varying `AutoMLJobName` cannot grow it without
/// end.
pub const AUTO_ML_JOB_QUOTA: usize = 1_000;

/// How long after `CreateAutoMLJobV2` the clock owes the job's terminal transition.
///
/// AWS returns an accepted AutoML job in the non-terminal `InProgress` status, so
/// leaving it is owed by the clock rather than by any call. Real Autopilot jobs take
/// tens of minutes; the span is compressed, never zero, so a caller that describes the
/// job straight after creating it still observes `InProgress`/`Starting`. Nothing
/// sleeps: the due instant is stored and materialised by the next call that reads the
/// record. The span sits at the top of the band this emulator gives a clock-owed
/// transition, so a caller's create-then-describe pair observes the transient status
/// even on a loaded host.
pub const AUTO_ML_JOB_TERMINAL_AFTER: Duration = Duration::from_millis(700);

/// `AutoMLJobStatus`'s members an AutoML job V2 created here passes through
/// (`smithy_query` shape=AutoMLJobStatus: Completed | InProgress | Failed | Stopped |
/// Stopping — the stop states belong to `StopAutoMLJob`, still a stub).
pub const AUTO_ML_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`AUTO_ML_STATUS_IN_PROGRESS`].
pub const AUTO_ML_STATUS_FAILED: &str = "Failed";
/// `AutoMLJobSecondaryStatus`'s members an AutoML job V2 created here passes through
/// (`smithy_query` shape=AutoMLJobSecondaryStatus). The emulator claims only the two
/// that need no training to have happened: the job has started, and the job failed.
pub const AUTO_ML_SECONDARY_STATUS_STARTING: &str = "Starting";
/// See [`AUTO_ML_SECONDARY_STATUS_STARTING`].
pub const AUTO_ML_SECONDARY_STATUS_FAILED: &str = "Failed";

/// The kind label [`super::could_not_find`] names an AutoML job V2 by.
pub const AUTO_ML_JOB_KIND_LABEL: &str = "AutoML job";

/// Why an AutoML job V2 this emulator accepted ends in `Failed`.
///
/// Autopilot's work is model training: it analyses the input dataset, trains and
/// evaluates candidates, writes candidate notebooks and model artifacts to the output
/// S3 path and reports a best candidate. This emulator runs no training, so reporting
/// `Completed` — or a `BestCandidate`, `AutoMLJobArtifacts` or `ResolvedAttributes`
/// derived from it — would claim results that do not exist. The job ends in the
/// modelled failure status with this reason instead.
pub const AUTO_ML_NOT_TRAINED_FAILURE_REASON: &str = "This emulator does not run Autopilot \
model training: no candidate was trained or evaluated for this AutoML job, so no model \
artifact, candidate notebook or data-exploration notebook was written to the output S3 \
location.";

/// An AutoML job V2 created by `CreateAutoMLJobV2`, as `DescribeAutoMLJobV2` replays it.
///
/// Every request member the job was created with is kept, so the describe replays what
/// was stored rather than a reconstruction, and `terminal_at` is the instant the clock
/// owes the `InProgress` -> `Failed` transition (see
/// [`AUTO_ML_NOT_TRAINED_FAILURE_REASON`]).
#[derive(Debug, Clone)]
pub struct AutoMLJobV2 {
    /// The name as the caller spelled it. The lookup key is
    /// [`super::entity_key`]'s case-folded form of it.
    pub name: String,
    pub arn: String,
    pub input_data_config: Vec<AutoMLJobChannel>,
    pub output_data_config: AutoMLOutputDataConfig,
    pub problem_type_config: AutoMLProblemTypeConfig,
    pub role_arn: String,
    pub objective: Option<AutoMLJobObjective>,
    pub model_deploy_config: Option<ModelDeployConfig>,
    pub data_split_config: Option<AutoMLDataSplitConfig>,
    pub security_config: Option<AutoMLSecurityConfig>,
    pub compute_config: Option<AutoMLComputeConfig>,
    pub tags: Vec<Tag>,
    /// An `AutoMLJobStatus` value.
    pub status: String,
    /// An `AutoMLJobSecondaryStatus` value.
    pub secondary_status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub terminal_at: SystemTime,
}

impl AutoMLJobV2 {
    /// Materialise the `InProgress` -> `Failed` transition the clock already owes.
    ///
    /// The one write a reporting operation may make: the transition was due at
    /// `terminal_at` whether or not anybody described the job, so the observer makes no
    /// difference to the result. `LastModifiedTime` and `EndTime` move to the instant
    /// the transition was due, not to "now", so both stay stable across later reads.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == AUTO_ML_STATUS_IN_PROGRESS && now >= self.terminal_at {
            self.status = AUTO_ML_STATUS_FAILED.to_string();
            self.secondary_status = AUTO_ML_SECONDARY_STATUS_FAILED.to_string();
            self.failure_reason = Some(AUTO_ML_NOT_TRAINED_FAILURE_REASON.to_string());
            self.last_modified_time = self.terminal_at;
            self.end_time = Some(self.terminal_at);
            return true;
        }
        false
    }
}

impl super::ComAmazonawsSagemakerSagemakerState {
    /// Whether another AutoML job V2 fits, i.e. the store is below
    /// [`AUTO_ML_JOB_QUOTA`]. Beside the state struct's other `*_below_quota` readers.
    pub fn auto_ml_jobs_v2_below_quota(&self) -> bool {
        self.auto_ml_jobs_v2.len() < AUTO_ML_JOB_QUOTA
    }
}

/// The `AutoMLProblemTypeConfigName` that names which `AutoMLProblemTypeConfig` union
/// member a job was created with (`smithy_query` shape=AutoMLProblemTypeConfigName:
/// ImageClassification | TextClassification | TimeSeriesForecasting | Tabular |
/// TextGeneration). Derived from the stored config, so the two can never disagree.
pub fn auto_ml_problem_type_config_name(config: &AutoMLProblemTypeConfig) -> &'static str {
    match config {
        AutoMLProblemTypeConfig::ImageClassificationJobConfig(_) => "ImageClassification",
        AutoMLProblemTypeConfig::TextClassificationJobConfig(_) => "TextClassification",
        AutoMLProblemTypeConfig::TimeSeriesForecastingJobConfig(_) => "TimeSeriesForecasting",
        AutoMLProblemTypeConfig::TabularJobConfig(_) => "Tabular",
        AutoMLProblemTypeConfig::TextGenerationJobConfig(_) => "TextGeneration",
    }
}

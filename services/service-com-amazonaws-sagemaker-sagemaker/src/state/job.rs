// SPDX-License-Identifier: CC0-1.0

//! The model-customization JOB slice of this service's state: what `CreateJob` persists
//! and `DescribeJob` replays.
//!
//! A child module of [`crate::state`] rather than another map beside it: the record, the
//! constants that bound and word it and the lookup every job operation performs live
//! here, while the ONE state struct keeps the single
//! [`jobs`](super::ComAmazonawsSagemakerSagemakerState::jobs) map, and [`crate::state`]
//! re-exports this module's items so a handler names them as `crate::state::…` like the
//! rest of the slice.
//!
//! # The job is also a resource of another service
//!
//! A job of one of the reinforcement-fine-tuning categories is the control-plane half of
//! a two-service resource: the job runtime data plane serves its rollouts, addressing it
//! by the ARN this slice mints and carrying no job definition of its own. So a create
//! records the job in the SHARED directory
//! ([`::service_common::agentic_rft_jobs::JobDirectory`]) beside this store, which is the
//! only place that data plane can resolve a job ARN against.

use std::time::SystemTime;

// The modelled shapes this slice keeps, reached through the parent module's import list
// rather than imported again here.
use super::Tag;

/// See [`super::MODEL_QUOTA`]. A job is released by `DeleteJob` (still a scaffold stub),
/// so until that lands this is what bounds the job store: a caller varying `JobName`
/// cannot grow it without end.
pub const JOB_QUOTA: usize = 1_000;

/// `JobStatus`'s member a job this emulator accepted is reported in
/// (`smithy_query` shape=JobStatus: InProgress | Completed | Failed | Stopping | Stopped |
/// Deleting | DeleteFailed — the stop and delete states belong to `StopJob` and
/// `DeleteJob`, both still stubs).
///
/// A job AWS has accepted is `InProgress`, and it is what this emulator reports for as
/// long as the job is held: the job's WORK is the rollouts its own harness drives through
/// the job runtime data plane, so nothing here can decide that the customization
/// finished. Reporting `Completed` or `Failed` would claim an outcome for training
/// nothing ran.
pub const JOB_STATUS_IN_PROGRESS: &str = "InProgress";

/// `JobSecondaryStatus`'s member a job created here carries
/// (`smithy_query` shape=JobSecondaryStatus). `Starting` is the one secondary status that
/// claims no compute of its own: the job has been accepted and its model session is
/// addressable by the data plane. Every other member of that enum (`Training`,
/// `Evaluating`, `Uploading`, …) would name work this emulator does not run.
pub const JOB_SECONDARY_STATUS_STARTING: &str = "Starting";

/// The `<kind>` segment a job's ARN carries, from `JobArn`'s own pattern
/// (`^arn:(aws[a-z\-]*):sagemaker:[a-z0-9\-]+:[0-9]{12}:job/[a-zA-Z0-9]+/…`). The segment
/// after it is the job's `JobCategory`, and the last one its `JobName`.
pub const JOB_ARN_KIND: &str = "job";

/// The kind label [`super::could_not_find`] names a job by.
pub const JOB_KIND_LABEL: &str = "job";

/// A job created by `CreateJob`, as `DescribeJob` replays it.
///
/// Every request member the job was created with is kept, so the describe replays what
/// was stored rather than a reconstruction.
#[derive(Debug, Clone)]
pub struct Job {
    /// The `JobName` the job was created with, which is also its key in the store.
    pub name: String,
    /// The `JobArn` this service minted for it.
    pub arn: String,
    /// A `JobCategory` value: which workload the job runs.
    pub category: String,
    /// The `RoleArn` SageMaker assumes to perform the job.
    pub role_arn: String,
    /// The `JobConfigSchemaVersion` the configuration document was declared against.
    pub config_schema_version: String,
    /// The `JobConfigDocument` the job was created with, kept verbatim.
    pub config_document: String,
    /// The tags the create carried.
    pub tags: Vec<Tag>,
    /// A `JobStatus` value.
    pub status: String,
    /// A `JobSecondaryStatus` value.
    pub secondary_status: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl super::ComAmazonawsSagemakerSagemakerState {
    /// Whether another job fits, i.e. the store is below [`JOB_QUOTA`]. Beside the state
    /// struct's other `*_below_quota` readers.
    pub fn jobs_below_quota(&self) -> bool {
        self.jobs.len() < JOB_QUOTA
    }

    /// The job `name` names when it was created under `category`, or `None` when the
    /// account holds no such job.
    ///
    /// `JobName` is documented unique "within your account and Amazon Web Services
    /// Region", so the NAME is the store's key; every job operation takes the category
    /// beside the name, and a pair naming a job of another category names no job at all.
    pub fn job(&self, category: &str, name: &str) -> Option<&Job> {
        self.jobs
            .get(name)
            .filter(|held| held.category == category)
    }
}

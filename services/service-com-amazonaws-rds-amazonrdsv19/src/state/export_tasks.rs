// SPDX-License-Identifier: CC0-1.0

//! The snapshot- and cluster-export records `DescribeExportTasks` reports and
//! `CancelExportTask` moves.
//!
//! An export task is the record of one export of a DB snapshot, DB cluster snapshot or DB
//! cluster to Amazon S3: which source was exported, where it is being written, under which
//! IAM role and KMS key, and how far it has got. The record carries the whole of the
//! modelled `ExportTask` shape, because that one shape is what every export-task operation
//! answers with ([`ExportTaskRecord::to_wire`]), so no two of them can disagree about a
//! task.
//!
//! `StartExportTask` — the operation that inserts here — is still scaffolded in this crate
//! and is its own stub issue, so this record is written by the cancel path and seeded by
//! this crate's tests exactly as that create will write it; the reporting operations run
//! against these records rather than against a canned answer, so they report exactly what
//! is stored.

use std::time::Instant;

use super::{now_as_wire_timestamp, PendingChange, PendingStatus};
use crate::ops::types::CancelExportTaskOutput;

/// `ExportTask$Status` — "The progress status of the export task": the export has been
/// accepted and is being set up.
pub const EXPORT_TASK_STATUS_STARTING: &str = "STARTING";

/// `ExportTask$Status` — the export is writing to the bucket.
pub const EXPORT_TASK_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `ExportTask$Status` — a cancellation has been accepted and the export is winding down.
pub const EXPORT_TASK_STATUS_CANCELING: &str = "CANCELING";

/// `ExportTask$Status` — the export was cancelled. Terminal.
pub const EXPORT_TASK_STATUS_CANCELED: &str = "CANCELED";

/// `ExportTask$Status` — the export finished writing. Terminal.
pub const EXPORT_TASK_STATUS_COMPLETE: &str = "COMPLETE";

/// `ExportTask$Status` — the export stopped on an error. Terminal. The record then also
/// carries the `FailureCause` that stopped it.
pub const EXPORT_TASK_STATUS_FAILED: &str = "FAILED";

/// Every status an export task reports, as `ExportTask$Status` lists them: "CANCELED
/// CANCELING COMPLETE FAILED IN_PROGRESS STARTING". The lowercase of each is also what
/// `DescribeExportTasks`'s `status` filter accepts ("Must be lowercase").
pub const EXPORT_TASK_STATUS_VALUES: [&str; 6] = [
    EXPORT_TASK_STATUS_CANCELED,
    EXPORT_TASK_STATUS_CANCELING,
    EXPORT_TASK_STATUS_COMPLETE,
    EXPORT_TASK_STATUS_FAILED,
    EXPORT_TASK_STATUS_IN_PROGRESS,
    EXPORT_TASK_STATUS_STARTING,
];

/// The statuses an export task does not leave on its own: "Any data that has already been
/// written to the S3 bucket isn't removed", and nothing moves a finished export again.
const TERMINAL_STATUSES: [&str; 3] = [
    EXPORT_TASK_STATUS_CANCELED,
    EXPORT_TASK_STATUS_COMPLETE,
    EXPORT_TASK_STATUS_FAILED,
];

/// A persisted snapshot or cluster export task.
///
/// Every member of the modelled `ExportTask` shape is stored rather than recomputed per
/// read: an export task has no identifier-derived fields (no ARN of its own), and the
/// progress fields belong to the export that is writing the objects, so a reader reports
/// exactly what the writer last left.
#[derive(Debug, Clone)]
pub struct ExportTaskRecord {
    /// `ExportTaskIdentifier`, which is half this record's key.
    pub identifier: String,
    /// The region whose credential scope the request that started the export was signed
    /// with; the scope a report of it is visible in.
    pub region: String,
    /// `SourceArn`: the snapshot or cluster being exported.
    pub source_arn: String,
    /// `SourceType`, one of [`crate::ops::types::EXPORT_SOURCE_TYPE_VALUES`].
    pub source_type: String,
    /// `ExportOnly`: the databases, schemas or tables the export is limited to, empty when
    /// the whole source is exported.
    pub export_only: Vec<String>,
    /// `SnapshotTime`: when the exported snapshot was created.
    pub snapshot_time: ::aws_smithy_types::DateTime,
    /// `TaskStartTime`.
    pub task_start_time: ::aws_smithy_types::DateTime,
    /// `TaskEndTime`, once the task has reached a terminal status.
    pub task_end_time: Option<::aws_smithy_types::DateTime>,
    /// `S3Bucket`.
    pub s3_bucket: String,
    /// `S3Prefix`.
    pub s3_prefix: Option<String>,
    /// `IamRoleArn`.
    pub iam_role_arn: String,
    /// `KmsKeyId`.
    pub kms_key_id: String,
    /// `Status`, one of [`EXPORT_TASK_STATUS_VALUES`].
    pub status: String,
    /// The status change the clock owes this record, if any.
    pub pending_status: Option<PendingStatus>,
    /// `PercentProgress`: how much of the source the export has written.
    pub percent_progress: i32,
    /// `TotalExtractedDataInGB`.
    pub total_extracted_data_in_gb: i32,
    /// `FailureCause`, for a task that failed.
    pub failure_cause: Option<String>,
    /// `WarningMessage`.
    pub warning_message: Option<String>,
}

impl ExportTaskRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, exactly as every other record's `settle` does.
    ///
    /// A task that reaches a terminal status has ENDED, so this is also where
    /// `TaskEndTime` is taken: the instant the clock made the terminal status due is the
    /// instant the task stopped, whether or not anybody was reading.
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        if self
            .pending_status
            .as_ref()
            .is_none_or(|pending| now < pending.due_at)
        {
            return false;
        }
        let pending = self.pending_status.take().expect("checked just above");
        match pending.change {
            PendingChange::Status(status) => {
                self.status = status;
                if self.is_terminal() && self.task_end_time.is_none() {
                    self.task_end_time = Some(now_as_wire_timestamp());
                }
            }
            PendingChange::Removal => return true,
        }
        false
    }

    /// Whether this task has stopped for good — see [`TERMINAL_STATUSES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_STATUSES.contains(&self.status.as_str())
    }

    /// Whether `CancelExportTask` can still act on this task: "You can't cancel an export
    /// task that has completed", and a cancellation already accepted is not accepted
    /// twice.
    pub fn is_cancelable(&self) -> bool {
        !self.is_terminal() && self.status != EXPORT_TASK_STATUS_CANCELING
    }

    /// Accept a cancellation: the task reports `CANCELING` and the clock owes it
    /// `CANCELED`, so a caller reading straight after the cancel still sees the transient
    /// status, exactly as it would against the real service.
    pub fn begin_cancellation(&mut self) {
        self.status = EXPORT_TASK_STATUS_CANCELING.to_string();
        self.pending_status = Some(PendingStatus::in_a_moment(EXPORT_TASK_STATUS_CANCELED));
    }

    /// Project the stored record onto the modelled `ExportTask` shape — the output of
    /// `CancelExportTask` and `StartExportTask`, and one entry of
    /// `DescribeExportTasks$ExportTasks`.
    ///
    /// `ExportOnly` is reported as the stored list, EMPTY where the whole source is
    /// exported, because that is what the member documents: "The data exported from the
    /// snapshot or cluster", which is all of it when the request named nothing.
    pub fn to_wire(&self) -> CancelExportTaskOutput {
        CancelExportTaskOutput {
            export_task_identifier: Some(self.identifier.clone()),
            source_arn: Some(self.source_arn.clone()),
            export_only: Some(self.export_only.clone()),
            snapshot_time: Some(self.snapshot_time),
            task_start_time: Some(self.task_start_time),
            task_end_time: self.task_end_time,
            s3_bucket: Some(self.s3_bucket.clone()),
            s3_prefix: self.s3_prefix.clone(),
            iam_role_arn: Some(self.iam_role_arn.clone()),
            kms_key_id: Some(self.kms_key_id.clone()),
            status: Some(self.status.clone()),
            percent_progress: Some(self.percent_progress),
            total_extracted_data_in_gb: Some(self.total_extracted_data_in_gb),
            failure_cause: self.failure_cause.clone(),
            warning_message: self.warning_message.clone(),
            source_type: Some(self.source_type.clone()),
        }
    }
}

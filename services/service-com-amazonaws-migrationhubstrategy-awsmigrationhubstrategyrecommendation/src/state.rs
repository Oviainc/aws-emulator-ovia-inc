// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation service.
//!
//! Strategy Recommendations has two primary resources behind its import-then-assess
//! loop, and both live here:
//!
//! * an **import file task**, minted by `StartImportFileTask` and reported by
//!   `GetImportFileTask` (and later by `ListImportFileTask`, still a stub);
//! * an **assessment**, minted by `StartAssessment`, reported by `GetAssessment` and
//!   `GetLatestAssessmentId`, and moved to a terminal status by `StopAssessment`.
//!
//! Every one of those operations computes its answer from the record below — there is
//! exactly one copy of each resource, and the `DataCollectionDetails` an assessment
//! reports is derived from the counters it stores, never canned.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AssessmentTarget, DataCollectionDetails, GetImportFileTaskOutput, Group,
    ResourceNotFoundException, ValidationException,
};

/// `ImportFileTaskStatus`'s non-terminal import status: the status a task carries from
/// the instant `StartImportFileTask` returns until the clock owes it a terminal one.
pub const IMPORT_IN_PROGRESS: &str = "ImportInProgress";

/// The terminal `ImportFileTaskStatus` an import file task reaches.
///
/// This emulator serves no S3 data plane, so there is no import file to read: the task
/// completes having imported no records, and [`ImportFileTask::number_of_records_success`]
/// / [`ImportFileTask::number_of_records_failed`] report exactly that — zero — rather
/// than a count nothing produced.
pub const IMPORT_SUCCESS: &str = "ImportSuccess";

/// `AssessmentStatus`'s non-terminal value: an assessment is collecting data.
pub const ASSESSMENT_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `AssessmentStatus` an assessment that ran to the end reaches.
pub const ASSESSMENT_COMPLETE: &str = "COMPLETE";

/// The `AssessmentStatus` `StopAssessment` moves a running assessment to.
pub const ASSESSMENT_STOPPED: &str = "STOPPED";

/// How long an import file task stays `ImportInProgress`, and an assessment
/// `IN_PROGRESS`, before the clock owes it a terminal status.
///
/// Neither AWS nor the model publishes a duration for either, so the emulator chooses
/// one span for both. It is deliberately NOT zero, so a caller acting straight after the
/// start call still observes the non-terminal status (and `StopAssessment` still has a
/// running assessment to stop). The due instant is stored on the record and materialised
/// by whichever later call reads it — nothing sleeps.
pub const COMPLETION_SPAN: Duration = Duration::from_millis(500);

/// How many import file tasks the store holds before `StartImportFileTask` answers its
/// modelled `ServiceQuotaExceededException` ("The AWS account has reached its quota of
/// imports.").
///
/// AWS publishes an adjustable quota this emulator does not model, so this is the
/// emulator's OWN bound on the store — far above any workflow it serves, and what stops
/// the state growing without end.
pub const IMPORT_TASK_QUOTA: usize = 1_000;

/// The same bound for assessments, refused with `StartAssessment`'s own modelled
/// `ServiceQuotaExceededException`.
pub const ASSESSMENT_QUOTA: usize = 1_000;

/// One import file task, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ImportFileTask {
    /// Minted by [`ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState::mint_import_file_task_id`].
    pub id: String,
    /// `StartImportFileTaskRequest$name`, reported as `importName`.
    pub import_name: String,
    /// `StartImportFileTaskRequest$S3Bucket`, reported as `inputS3Bucket`.
    pub input_s3_bucket: String,
    /// `StartImportFileTaskRequest$s3key`, reported as `inputS3Key`.
    pub input_s3_key: String,
    /// `StartImportFileTaskRequest$dataSourceType`: where the servers come from.
    pub data_source_type: Option<String>,
    /// `StartImportFileTaskRequest$groupId`: the groups this import's resources join.
    pub group_id: Vec<Group>,
    /// `StartImportFileTaskRequest$s3bucketForReportData` — the bucket the import
    /// results would be uploaded to.
    pub status_report_s3_bucket: Option<String>,
    /// The key of the status report object. Unset: this emulator writes no report
    /// object, so there is no key to name.
    pub status_report_s3_key: Option<String>,
    /// One of `ImportFileTaskStatus`'s values.
    pub status: String,
    pub start_time: SystemTime,
    pub completion_time: Option<SystemTime>,
    pub number_of_records_success: i32,
    pub number_of_records_failed: i32,
    /// The instant the clock owes this task its terminal status, while it has one.
    pub completion_due: Option<SystemTime>,
}

impl ImportFileTask {
    /// Materialise the completion the clock owes this task, if `now` is past its due
    /// instant.
    ///
    /// `completion_time` is the DUE instant rather than `now`, so a task reports the
    /// same completion time however late it is first read.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.completion_due else {
            return;
        };
        if now < due {
            return;
        }
        self.status = IMPORT_SUCCESS.to_string();
        self.completion_time = Some(due);
        self.completion_due = None;
    }

    /// True once this task reached a terminal `ImportFileTaskStatus`.
    pub fn is_complete(&self) -> bool {
        self.completion_due.is_none()
    }

    /// The `GetImportFileTask` answer for this task.
    ///
    /// The record's own values, with the three completion members reported only once
    /// the task completed: a running import has no completion time and no record counts
    /// yet.
    pub fn to_output(&self) -> GetImportFileTaskOutput {
        let complete = self.is_complete();
        GetImportFileTaskOutput {
            id: Some(self.id.clone()),
            status: Some(self.status.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            input_s3_bucket: Some(self.input_s3_bucket.clone()),
            input_s3_key: Some(self.input_s3_key.clone()),
            status_report_s3_bucket: self.status_report_s3_bucket.clone(),
            status_report_s3_key: self.status_report_s3_key.clone(),
            completion_time: self
                .completion_time
                .map(::aws_smithy_types::DateTime::from),
            number_of_records_success: complete.then_some(self.number_of_records_success),
            number_of_records_failed: complete.then_some(self.number_of_records_failed),
            import_name: Some(self.import_name.clone()),
        }
    }
}

/// One assessment of an on-premises environment, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Assessment {
    /// An `AsyncTaskId` minted by
    /// [`ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState::mint_assessment_id`].
    pub id: String,
    /// One of `AssessmentStatus`'s values.
    pub status: String,
    /// `StartAssessmentRequest$assessmentTargets`, reported back verbatim.
    pub assessment_targets: Vec<AssessmentTarget>,
    pub assessment_data_source_type: Option<String>,
    pub s3bucket_for_analysis_data: Option<String>,
    pub s3bucket_for_report_data: Option<String>,
    pub start_time: SystemTime,
    pub completion_time: Option<SystemTime>,
    pub status_message: Option<String>,
    /// How many servers this assessment covers: the servers the service's data source
    /// held when the assessment started (see
    /// [`ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState::imported_server_count`]).
    pub servers: i32,
    pub success: i32,
    pub failed: i32,
    pub in_progress: i32,
    /// The instant the clock owes this assessment `COMPLETE`, while it is running.
    pub completion_due: Option<SystemTime>,
}

impl Assessment {
    /// Materialise the completion the clock owes this assessment, if `now` is past its
    /// due instant: every server still in progress has finished collecting by then.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.completion_due else {
            return;
        };
        if now < due {
            return;
        }
        self.status = ASSESSMENT_COMPLETE.to_string();
        self.completion_time = Some(due);
        self.success = self.success.saturating_add(self.in_progress);
        self.in_progress = 0;
        self.completion_due = None;
        self.status_message = Some(format!(
            "Data collection completed for {} of {} servers.",
            self.success, self.servers
        ));
    }

    /// True once this assessment reached a terminal `AssessmentStatus` — nothing is
    /// collecting any more, so there is nothing left to stop.
    pub fn is_terminal(&self) -> bool {
        self.completion_due.is_none()
    }

    /// Stop a running assessment: `STOPPED` is terminal, so the completion the clock
    /// owed it is cancelled and the servers that had not finished collecting are
    /// neither a success nor a failure.
    ///
    /// Answers `false` for an assessment that already reached a terminal status, whose
    /// stored status is left exactly as it was.
    pub fn stop(&mut self, now: SystemTime) -> bool {
        if self.is_terminal() {
            return false;
        }
        self.status = ASSESSMENT_STOPPED.to_string();
        self.completion_time = Some(now);
        self.status_message = Some(format!(
            "Data collection stopped with {} of {} servers collected.",
            self.success, self.servers
        ));
        self.in_progress = 0;
        self.completion_due = None;
        true
    }

    /// The `DataCollectionDetails` `GetAssessment` reports: this record's status, its
    /// counters and its timestamps, computed from nothing else.
    pub fn to_data_collection_details(&self) -> DataCollectionDetails {
        DataCollectionDetails {
            status: Some(self.status.clone()),
            servers: Some(self.servers),
            failed: Some(self.failed),
            success: Some(self.success),
            in_progress: Some(self.in_progress),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            completion_time: self
                .completion_time
                .map(::aws_smithy_types::DateTime::from),
            status_message: self.status_message.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState {
    /// Every import file task, keyed by its id. A `BTreeMap` is what will give
    /// `ListImportFileTask` a stable total order to paginate over.
    pub import_tasks: BTreeMap<String, ImportFileTask>,
    /// Every assessment, keyed by its id.
    pub assessments: BTreeMap<String, Assessment>,
    /// The id of the most recently started assessment, which is what
    /// `GetLatestAssessmentId` answers. `None` until one is started.
    pub latest_assessment_id: Option<String>,
    /// The counter both kinds of id are minted from, so no two resources of this
    /// service ever share a suffix.
    pub next_id: u64,
}

impl ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState {
    /// The next value of the shared id counter.
    fn next_sequence(&mut self) -> u64 {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }

    /// Mint an unused import file task id.
    ///
    /// `StartImportFileTaskResponse$id` is a `String` bounded to 1024 with pattern
    /// `.*\S.*`, so the shape of the id is the emulator's choice; the loop skips an id
    /// already taken, which is what makes the counter's wrap safe.
    pub fn mint_import_file_task_id(&mut self) -> String {
        loop {
            let id = format!("import-file-task-{:012x}", self.next_sequence());
            if !self.import_tasks.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused assessment id matching `AsyncTaskId`'s `[0-9a-z-:]+`, within its
    /// 52-character bound.
    pub fn mint_assessment_id(&mut self) -> String {
        loop {
            let id = format!("assessment-{:012x}", self.next_sequence());
            if !self.assessments.contains_key(&id) {
                return id;
            }
        }
    }

    /// The import file task `id` names, with any completion the clock owes it already
    /// materialised.
    pub fn settled_import_task_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut ImportFileTask> {
        let task = self.import_tasks.get_mut(id)?;
        task.settle(now);
        Some(task)
    }

    /// The assessment `id` names, with any completion the clock owes it already
    /// materialised.
    pub fn settled_assessment_mut(&mut self, id: &str, now: SystemTime) -> Option<&mut Assessment> {
        let assessment = self.assessments.get_mut(id)?;
        assessment.settle(now);
        Some(assessment)
    }

    /// Materialise every completion the clock owes, for a caller about to read across
    /// all of the records.
    pub fn settle_all(&mut self, now: SystemTime) {
        for task in self.import_tasks.values_mut() {
            task.settle(now);
        }
        for assessment in self.assessments.values_mut() {
            assessment.settle(now);
        }
    }

    /// How many servers an assessment started now would cover.
    ///
    /// An import file task is the only way a server enters this service's state — the
    /// Application Discovery Service and collector data sources are not emulated — so
    /// the answer is every record the completed imports brought in. Call
    /// [`Self::settle_all`] first so a task whose completion is already due is counted.
    pub fn imported_server_count(&self) -> i32 {
        self.import_tasks
            .values()
            .filter(|task| task.is_complete())
            .fold(0i32, |total, task| {
                total.saturating_add(task.number_of_records_success)
            })
    }
}

/// This service's message for a request refused because a value it carries does not
/// satisfy a rule the model declares.
///
/// Strategy Recommendations states that refusal as ONE service-wide sentence — the
/// `ValidationException` the model documents as "The request body isn't valid." — and
/// names neither the member that failed nor the value it rejected. It is a constant of
/// the service, not a rendering of the request, so it is stated once here and used for
/// every operation and every constraint kind (see
/// `EmulatedService::override_refusal`).
pub const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// This service's message for a request whose parameters it cannot resolve: an id that
/// names no resource of this account.
///
/// Like [`INVALID_REQUEST_BODY`] it is a constant of the service and carries nothing
/// from the request — in particular not the id that failed to resolve, which a caller
/// already holds.
pub const INVALID_OR_MISSING_PARAMETERS: &str =
    "Looks like you provided invalid or missing parameters.";

/// The `ResourceNotFoundException` payload for an import file task id that names no
/// task.
///
/// Takes no id: the message is this service's constant for an unresolvable parameter,
/// so there is nothing from the request to put in it.
pub fn import_task_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(INVALID_OR_MISSING_PARAMETERS.to_string()),
    }
}

/// The `ResourceNotFoundException` payload for an assessment id that names no
/// assessment. The same constant, for the same reason.
pub fn assessment_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(INVALID_OR_MISSING_PARAMETERS.to_string()),
    }
}

/// The `ValidationException` payload for an assessment id that names no assessment, on
/// an operation whose model declares no `ResourceNotFoundException`.
///
/// Same family, same constant: an id that resolves to nothing is an unresolvable
/// parameter whichever error shape the operation declares for it.
pub fn invalid_assessment_id() -> ValidationException {
    ValidationException {
        message: Some(INVALID_OR_MISSING_PARAMETERS.to_string()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>)
}

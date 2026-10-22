// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation service.
//!
//! Strategy Recommendations has two primary resources behind its import-then-assess
//! loop, and both live here:
//!
//! * an **import file task**, minted by `StartImportFileTask` and reported by
//!   `GetImportFileTask` one at a time and by `ListImportFileTask` a page at a time;
//! * an **assessment**, minted by `StartAssessment`, reported by `GetAssessment` and
//!   `GetLatestAssessmentId`, and moved to a terminal status by `StopAssessment`.
//!
//! Every one of those operations computes its answer from the record below — there is
//! exactly one copy of each resource, and the `DataCollectionDetails` an assessment
//! reports is derived from the counters it stores, never canned.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AssessmentTarget, DataCollectionDetails, GetImportFileTaskOutput, Group,
    ImportFileTaskInformation, ResourceNotFoundException, ValidationException,
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

    /// The `ListImportFileTask` entry for this task.
    ///
    /// The same record the single-task read reports, in the list shape: both answers
    /// come from these fields, so a task cannot read one way through one operation and
    /// another way through the other.
    pub fn to_task_information(&self) -> ImportFileTaskInformation {
        let reported = self.to_output();
        ImportFileTaskInformation {
            id: reported.id,
            status: reported.status,
            start_time: reported.start_time,
            input_s3_bucket: reported.input_s3_bucket,
            input_s3_key: reported.input_s3_key,
            status_report_s3_bucket: reported.status_report_s3_bucket,
            status_report_s3_key: reported.status_report_s3_key,
            completion_time: reported.completion_time,
            number_of_records_success: reported.number_of_records_success,
            number_of_records_failed: reported.number_of_records_failed,
            import_name: reported.import_name,
        }
    }
}

/// One page of import file tasks, as `ListImportFileTask` answers it.
#[derive(Debug, Default)]
pub struct ImportTaskPage {
    /// The entries of this page, in the store's total order.
    pub task_infos: Vec<ImportFileTaskInformation>,
    /// The cursor the next call resumes from, set only when another task follows this
    /// page.
    pub next_token: Option<String>,
}

/// The largest page `ListImportFileTask` serves, and the page size it uses when the
/// caller names none.
///
/// `ListImportFileTaskRequest$maxResults` documents "The total number of items to
/// return. The maximum value is 100." The member carries no `@range`, so the boundary
/// validator passes any integer and the bound is enforced where the page is taken.
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// The marker every pagination token this service mints carries, so a token minted
/// elsewhere (or invented by a caller) is recognised as foreign rather than resolved
/// against this store.
const PAGE_TOKEN_MARKER: &str = "migrationhubstrategy:import-file-task:";

/// Encode the id a page stopped after as an opaque `nextToken`.
///
/// Hex round-trips exactly and is non-empty for a non-empty key, so the token satisfies
/// the member's `@length(0, 1024)` and `.*\S.*`, and the next call resolves it against
/// the same map — no server-side cursor table.
pub fn encode_page_token(key: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_MARKER}{key}").as_bytes())
}

/// The id an [`encode_page_token`] token names; `None` for anything this service did not
/// mint, including well-formed hex carrying no [`PAGE_TOKEN_MARKER`].
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_MARKER).map(str::to_string)
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
    /// Every import file task, keyed by its id. A `BTreeMap` is what gives
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

    /// One page of this store's import file tasks, continuing after the task a
    /// `nextToken` names.
    ///
    /// The order is the store's own total order over the minted ids, which is the order
    /// the tasks were started in: `BTreeMap` iterates its keys sorted and an id's
    /// zero-padded counter suffix sorts the same way it counts. A token is emitted only
    /// when another task actually follows the page, so a caller following tokens never
    /// makes a call that returns nothing.
    ///
    /// Refused with the service's `ValidationException` when `max_results` is outside
    /// the documented 1..=[`MAX_LIST_PAGE_SIZE`], or when the token names no task of this
    /// store. Each task in the page gets the completion the clock already owed it first —
    /// owed at this same instant even if nobody had called.
    pub fn page_of_import_tasks(
        &mut self,
        next_token: Option<&str>,
        max_results: Option<i32>,
        now: SystemTime,
    ) -> Result<ImportTaskPage, ValidationException> {
        let limit = match max_results {
            None => MAX_LIST_PAGE_SIZE,
            Some(requested) if (1..=MAX_LIST_PAGE_SIZE).contains(&requested) => requested,
            Some(_) => return Err(invalid_request_body()),
        };
        let limit = usize::try_from(limit).unwrap_or(1);

        let after = match next_token {
            Some(token) => {
                let resume_after = decode_page_token(token).ok_or_else(page_token_not_accepted)?;
                if !self.import_tasks.contains_key(&resume_after) {
                    return Err(page_token_not_accepted());
                }
                Bound::Excluded(resume_after)
            }
            None => Bound::Unbounded,
        };

        let mut rest = self.import_tasks.range_mut((after, Bound::Unbounded));
        let mut task_infos = Vec::new();
        let mut last: Option<String> = None;
        for (id, task) in rest.by_ref().take(limit) {
            task.settle(now);
            task_infos.push(task.to_task_information());
            last = Some(id.clone());
        }
        let next_token = match (rest.next(), last) {
            (Some(_), Some(key)) => Some(encode_page_token(&key)),
            _ => None,
        };

        Ok(ImportTaskPage {
            task_infos,
            next_token,
        })
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

/// The `ValidationException` (HTTP 400) this service answers for a request carrying a
/// value outside a rule it enforces but the model does not express — a documented bound
/// the boundary validator cannot refuse.
///
/// Worded with [`INVALID_REQUEST_BODY`], the same one sentence `EmulatedService::
/// override_refusal` gives a modelled constraint violation, so a caller sees one refusal
/// for one logical failure whichever side of the boundary decided it.
pub fn invalid_request_body() -> ValidationException {
    ValidationException {
        message: Some(INVALID_REQUEST_BODY.to_string()),
    }
}

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

/// The refusal of a `nextToken` this service cannot resolve: one it never minted, or one
/// naming a task the store no longer holds.
///
/// A pagination token is an unresolvable parameter, so it earns the same
/// `ValidationException` (HTTP 400) and the same constant sentence
/// ([`INVALID_OR_MISSING_PARAMETERS`]) as any other — the token is NOT echoed back.
pub fn page_token_not_accepted() -> ValidationException {
    ValidationException {
        message: Some(INVALID_OR_MISSING_PARAMETERS.to_string()),
    }
}

/// This service's message for an assessment id that resolves to no assessment, on the
/// operation that reports it as a missing resource.
///
/// `StopAssessment`'s recorded 404 carries this sentence rather than
/// [`INVALID_OR_MISSING_PARAMETERS`]: the service words "I cannot resolve what you sent"
/// and "that resource is not here" differently. Like the other two it is a constant of
/// the service — the recorded response for an unknown id does not interpolate the id, so
/// neither does this.
pub const INVALID_RESOURCE_ID: &str = "Invalid resourceId: ";

/// This service's message for a report it cannot produce because the resource it would
/// name does not exist yet, as `GetLatestAssessmentId` answers it in an account where no
/// assessment has ever been started.
pub const NOTHING_TO_REPORT: &str = "Something went wrong, please try again later.";

/// The prefix every S3 bucket name this service can use has to begin with.
///
/// Documented on each bucket member the service takes — `StartAssessmentRequest`'s
/// `s3bucketForAnalysisData` and `s3bucketForReportData` ("The bucket name must begin
/// with `migrationhub-strategy-`") and `StartImportFileTaskRequest`'s `S3Bucket` and
/// `s3bucketForReportData` ("The bucket name is required to begin with
/// `migrationhub-strategy-`"). The members' own `@pattern`
/// (`[0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+`) and `@length(0,63)` cannot express a prefix, so
/// the boundary validator passes a non-conforming name and the handler enforces it.
pub const BUCKET_NAME_PREFIX: &str = "migrationhub-strategy-";

/// Refuse a bucket naming one this service cannot use.
///
/// `Ok` for an absent member (nothing was named) and for a name carrying
/// [`BUCKET_NAME_PREFIX`]; otherwise the `ValidationException` (HTTP 400) the service
/// answers for a bucket it cannot reach, naming the bucket the way it reports any such
/// bucket.
///
/// The prefix is the whole of the check. This emulator serves no S3 data plane, so it
/// cannot look a bucket up; the documented prefix is the one property of a name it can
/// decide, and a conforming name is taken to name a bucket the service can use rather
/// than refusing every start call and leaving the resource unreachable.
pub fn check_bucket_usable(value: Option<&str>) -> Result<(), ValidationException> {
    match value {
        Some(bucket) if !bucket.starts_with(BUCKET_NAME_PREFIX) => Err(ValidationException {
            message: Some(format!(
                "S3 bucket \"{bucket}\" does not exist or is not accessible [S3BucketNotFound]"
            )),
        }),
        _ => Ok(()),
    }
}

/// Refuse an operation that was given no bucket for a member it cannot run without.
///
/// For a member the model leaves optional but the service needs: the refusal names the
/// MEMBER and the prefix a bucket for it has to carry, which is what the caller has to
/// fix. `action` is what the operation does with the bucket, so the message reads for the
/// operation it came from. A member the model marks `@required` never reaches here — the
/// boundary validator refuses its absence first.
pub fn require_bucket(
    member: &str,
    action: &str,
    value: Option<&str>,
) -> Result<(), ValidationException> {
    if value.is_some() {
        return Ok(());
    }
    Err(ValidationException {
        message: Some(format!(
            "S3 bucket {member} is required for {action}. Please specify s3 bucket \
             with \"{BUCKET_NAME_PREFIX}\" prefix"
        )),
    })
}

/// `GetAssessment`'s refusal of an `id` that names no assessment of this account.
///
/// Strategy Recommendations answers that read with its invalid-parameter refusal —
/// `ValidationException`, HTTP 400 — rather than a not-found, which is what the recorded
/// wire behaviour of `GET /get-assessment/{id}` shows for a pattern- and length-legal id
/// the account was never issued. The status and the error code are the requirement; the
/// message is this service's one sentence for an unresolvable parameter
/// ([`INVALID_OR_MISSING_PARAMETERS`]), which carries nothing from the request.
pub fn assessment_id_not_accepted() -> ValidationException {
    ValidationException {
        message: Some(INVALID_OR_MISSING_PARAMETERS.to_string()),
    }
}

/// `StopAssessment`'s refusal of an `assessmentId` that names no assessment: the
/// service's `ResourceNotFoundException` (HTTP 404, "The specified ID in the request is
/// not found."), worded with [`INVALID_RESOURCE_ID`].
pub fn assessment_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(INVALID_RESOURCE_ID.to_string()),
    }
}

/// `GetLatestAssessmentId`'s answer in an account where no assessment has ever been
/// started: there is no assessment to name, so the absent resource is reported with
/// `ResourceNotFoundException`, HTTP 404.
///
/// The operation takes no input, so an invalid-request refusal is not a reachable
/// outcome — there is nothing a caller could have sent wrongly.
pub fn no_latest_assessment() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(NOTHING_TO_REPORT.to_string()),
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

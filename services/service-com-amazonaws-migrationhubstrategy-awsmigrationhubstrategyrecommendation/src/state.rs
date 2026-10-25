// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation service.
//!
//! Strategy Recommendations has two primary resources behind its import-then-assess
//! loop, and both live here:
//!
//! * an **import file task**, minted by `StartImportFileTask` and reported by
//!   `GetImportFileTask` one at a time and by `ListImportFileTask` a page at a time —
//!   including the status report object that import wrote into the report bucket the
//!   request named, which is what its `statusReportS3Key` names
//!   ([`import_status_report_key`]);
//! * an **assessment**, minted by `StartAssessment`, reported by `GetAssessment` and
//!   `GetLatestAssessmentId`, and moved to a terminal status by `StopAssessment`.
//!
//! Every one of those operations computes its answer from the record below — there is
//! exactly one copy of each resource, and the `DataCollectionDetails` an assessment
//! reports is derived from the counters it stores, never canned.
//!
//! Beside them sits the account's **server inventory** ([`ServerRecord`]), which
//! `ListServers` reports: the servers an import file task ingested, each carrying the
//! import groups it came in with, the recommendation the service holds for it and the
//! strategy option the account prefers for it — what `UpdateServerConfig` writes and
//! `GetServerStrategies` reports. A server a collector fetched also carries the discovery
//! data that fetch brought with it ([`ServerDiscovery`]), which is what
//! `ListAnalyzableServers` reports.
//!
//! Beside the servers sit the **data collectors** installed in the account's environment
//! ([`CollectorRecord`]), which `ListCollectors` reports a page at a time: each one is
//! reported exactly as its registration recorded it, so that read derives nothing from the
//! request that makes it.
//!
//! Beside the servers, the account's **application components**
//! ([`ApplicationComponentRecord`]) — the processes and databases the assessment found
//! running on those servers — which `GetApplicationComponentDetails` reports one at a
//! time and `ListApplicationComponents` a filtered page at a time: every member of the
//! `ApplicationComponentDetail` it answers comes from that
//! record, and the association counts it reports are counted off the record's own lists.
//! `UpdateApplicationComponentConfig` writes that same record — the component's app type,
//! inclusion status, database credentials, source code configuration and the strategy
//! option the account prefers for it — and `GetApplicationComponentStrategies` reports
//! that preference beside the service's own recommendation, so each read answers what the
//! update stored.
//!
//! Strategy Recommendations is a REGIONAL service, so none of those resources is held
//! once for the account: the store holds one [`RegionState`] per Region, and a resource
//! created through one Region's endpoint is reported only by reads signed for that same
//! Region. Every handler resolves its request's Region with [`request_region`] and then
//! reaches that Region's portfolio alone.
//!
//! And beside those, the account's one **portfolio preferences** record
//! ([`PortfolioPreferences`]): what `PutPortfolioPreferences` saves and
//! `GetPortfolioPreferences` reports, member for member.
//!
//! Last, the account's **recommendation report generation tasks**
//! ([`RecommendationReport`]): what `StartRecommendationReportGeneration` mints — one at
//! a time, a start call made while one is running coalescing onto it — and
//! `GetRecommendationReportDetails` reports.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AnalyzableServerSummary, AppUnitError, ApplicationComponentDetail, ApplicationComponentStrategy,
    ApplicationPreferences,
    AssessmentTarget, AssociatedApplication, Collector, ConfigurationSummary, ConflictException,
    DataCollectionDetails, DatabaseConfigDetail,
    DatabasePreferences, GetImportFileTaskOutput, GetPortfolioPreferencesOutput, Group, ImportFileTaskInformation,
    OSInfo, PrioritizeBusinessGoals, RecommendationReportDetails, RecommendationSet,
    ResourceNotFoundException, ServerDetail, ServerError, ServerStrategy, SourceCodeRepository,
    StrategyOption, SystemInfo, TransformationTool, ValidationException, GROUP_NAME_VALUES,
};

/// The Region partitioning of this store: the per-service state struct itself, the Region
/// a request is served in, and the ceiling on how many Regions the store keeps a
/// portfolio for.
pub mod regions;

pub use regions::{
    request_region, ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState,
    DEFAULT_REGION, MAX_REGIONS,
};

/// `ImportFileTaskStatus`'s non-terminal import status: the status a task carries from
/// the instant `StartImportFileTask` returns until the clock owes it a terminal one.
pub const IMPORT_IN_PROGRESS: &str = "ImportInProgress";

/// The terminal `ImportFileTaskStatus` an import file task reaches.
///
/// The contents of the import file are not read, so the task completes having imported
/// no records, and [`ImportFileTask::number_of_records_success`]
/// / [`ImportFileTask::number_of_records_failed`] report exactly that — zero — rather
/// than a count nothing produced.
pub const IMPORT_SUCCESS: &str = "ImportSuccess";

/// `AssessmentStatus`'s non-terminal value: an assessment is collecting data.
pub const ASSESSMENT_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `AssessmentStatus` an assessment that ran to the end reaches.
pub const ASSESSMENT_COMPLETE: &str = "COMPLETE";

/// The `AssessmentStatus` `StopAssessment` moves a running assessment to.
pub const ASSESSMENT_STOPPED: &str = "STOPPED";

/// The `RunTimeAssessmentStatus` a server of the portfolio carries while the assessment
/// covering it is collecting data from it.
///
/// `ServerDetail$dataCollectionStatus` is documented "The status of assessment for the
/// server", so it is the per-server face of the assessment's own `AssessmentStatus`: an
/// assessment reported `IN_PROGRESS` is collecting, and the collection task on each
/// server it covers has started.
pub const COLLECTION_STARTED: &str = "dataCollectionTaskStarted";

/// The `RunTimeAssessmentStatus` a server still collecting carries once a caller stopped
/// the assessment — the per-server face of [`ASSESSMENT_STOPPED`].
pub const COLLECTION_STOPPED: &str = "dataCollectionTaskStopped";

/// The `RunTimeAssessmentStatus` a server still collecting carries once the assessment
/// ran to the end — the per-server face of [`ASSESSMENT_COMPLETE`].
pub const COLLECTION_SUCCESS: &str = "dataCollectionTaskSuccess";

/// The `RunTimeAssessmentStatus` values that mean a collection task is RUNNING on the
/// server, and so the ones a terminal assessment status moves off that server.
///
/// `dataCollectionTaskToBeScheduled` is not one of them: nothing is collecting for a
/// server whose task was never scheduled, so a stop or a completion leaves it where it
/// was, as it does a server that already reached a terminal status of its own.
pub const COLLECTION_RUNNING: &[&str] = &["dataCollectionTaskScheduled", COLLECTION_STARTED];

/// `RecommendationReportStatus`'s non-terminal value: the status a report generation
/// task carries from the instant `StartRecommendationReportGeneration` returns until the
/// clock owes it a terminal one.
pub const REPORT_IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal `RecommendationReportStatus` a report generation task that ran to the
/// end reaches.
///
/// It is the status the service's recorded behaviour shows a started task settling on (a
/// task is `IN_PROGRESS` and then `SUCCESS`), so a task of this emulator settles there
/// too. What this emulator cannot do is DELIVER the artifact: a recommendation report is
/// an Excel or JSON FILE the service uploads to the account's S3 report bucket, and there
/// is no S3 data plane here to upload it to. That is reported rather than papered over —
/// the task's [`RecommendationReport::status_message`] says no report file was written,
/// and `s3Bucket`/`s3Keys` stay absent, so nothing names a file nothing wrote.
pub const REPORT_SUCCESS: &str = "SUCCESS";

/// The `OutputFormat` a report generation task takes when the request names none:
/// `StartRecommendationReportGenerationRequest$outputFormat` is documented "The output
/// format for the recommendation report file. The default format is Microsoft Excel",
/// and `Excel` is that format's value in `OutputFormat`.
pub const DEFAULT_OUTPUT_FORMAT: &str = "Excel";

/// How long an import file task stays `ImportInProgress`, an assessment `IN_PROGRESS`,
/// and a recommendation report generation task `IN_PROGRESS`, before the clock owes it a
/// terminal status.
///
/// Neither AWS nor the model publishes a duration for any of them, so the emulator
/// chooses one span for all three. It is deliberately NOT zero, so a caller acting
/// straight after the start call still observes the non-terminal status (and
/// `StopAssessment` still has a running assessment to stop). The due instant is stored on
/// the record and materialised by whichever later call reads it — nothing sleeps.
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

/// How many recommendation report generation tasks the store holds before
/// `StartRecommendationReportGeneration` refuses to mint another.
///
/// The same emulator-owned bound as [`IMPORT_TASK_QUOTA`] and [`ASSESSMENT_QUOTA`], for
/// the same reason: it is what stops the store growing without end. The operation models
/// no quota error, so the refusal is its modelled `ThrottlingException` (HTTP 429) — the
/// request is denied because too many have been made.
pub const REPORT_QUOTA: usize = 1_000;

/// How many `groupIdFilter` entries `StartRecommendationReportGeneration` accepts.
///
/// `GroupIds` declares no `@length`, so the boundary validator admits a list of any size;
/// this is the emulator's OWN bound on the work one request can ask for, and a filter
/// past it is refused with the service's invalid-request-body sentence rather than
/// silently truncated — the same treatment a `maxResults` past its documented maximum
/// gets ([`MAX_LIST_PAGE_SIZE`]).
pub const MAX_GROUP_FILTER_ENTRIES: usize = 100;

/// One import file task, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ImportFileTask {
    /// Minted by [`ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState::mint_import_file_task_id`],
    /// unique across every Region's portfolio.
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
    /// results are uploaded to.
    pub status_report_s3_bucket: Option<String>,
    /// The key of the status report object this import wrote inside
    /// [`Self::status_report_s3_bucket`], set by the start call once the object is
    /// there. `None` when the request named no report bucket, or when the object could
    /// not be written: a key is only ever held for an object a caller can read.
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
            // The status report is the artifact of a FINISHED import, so its key is
            // reported with the other completion members: a task still importing has
            // no final report for a caller to read per-record outcomes from.
            status_report_s3_key: complete
                .then(|| self.status_report_s3_key.clone())
                .flatten(),
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

/// The largest page a list operation of this service serves, and the page size it uses
/// when the caller names none.
///
/// Every `maxResults` member the service takes documents the same bound —
/// `ListImportFileTaskRequest$maxResults` "The total number of items to return. The
/// maximum value is 100.", `ListServersRequest$maxResults` "The maximum number of items
/// to include in the response. The maximum value is 100." The members carry no `@range`,
/// so the boundary validator passes any integer and the bound is enforced where the page
/// is taken.
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// The marker a `ListImportFileTask` cursor carries, so a token minted elsewhere (or
/// invented by a caller) is recognised as foreign rather than resolved against this
/// store.
pub const IMPORT_TASK_PAGE_MARKER: &str = "migrationhubstrategy:import-file-task:";

/// The marker a `ListServers` cursor carries.
///
/// A token names a position in ONE list, so each list has its own marker and a token
/// issued for another list is foreign here — the two never resolve against each other's
/// records.
pub const SERVER_PAGE_MARKER: &str = "migrationhubstrategy:servers:";

/// The marker a `GetServerDetails` cursor carries.
///
/// That operation pages over ONE server's associated applications rather than over the
/// inventory, so it has its own marker: a `ListServers` token handed to it (or one of its
/// own handed to `ListServers`) is foreign and refused instead of resolving against the
/// other list's records.
pub const SERVER_DETAIL_PAGE_MARKER: &str = "migrationhubstrategy:server-applications:";

/// The marker a `ListAnalyzableServers` cursor carries.
///
/// That list holds the servers a collector fetched, which is a SUBSET of the inventory in
/// an order of its own, so it gets its own marker: a `ListServers` token handed to it (or
/// one of its own handed to `ListServers`) is foreign and refused instead of resuming at
/// whatever position its server happens to have in the other list.
pub const ANALYZABLE_SERVER_PAGE_MARKER: &str = "migrationhubstrategy:analyzable-servers:";

/// The marker a `ListCollectors` cursor carries.
///
/// That list pages over the installed collectors rather than over any list of servers, so
/// it gets its own marker: a token issued by one of this service's other lists is foreign
/// here (and one of these is foreign there) and refused instead of resolving against
/// another list's records.
pub const COLLECTOR_PAGE_MARKER: &str = "migrationhubstrategy:collectors:";

/// The marker a `ListApplicationComponents` cursor carries.
///
/// That list pages over the account's application components rather than over any list of
/// servers, so it gets its own marker: a token one of this service's other lists issued is
/// foreign here (and one of these is foreign there) and refused instead of resolving
/// against another list's records.
pub const APPLICATION_COMPONENT_PAGE_MARKER: &str =
    "migrationhubstrategy:application-components:";

/// Encode the key a page stopped at as an opaque `nextToken` carrying `marker`.
///
/// Hex round-trips exactly and is non-empty for a non-empty key, so the token satisfies
/// the `nextToken` members' `@length` and `.*\S.*`, and the next call resolves it against
/// the same map — no server-side cursor table. One codec for every list of this service,
/// so the token shape cannot drift between them; the marker is what tells the lists
/// apart.
pub fn encode_page_token(marker: &str, key: &str) -> String {
    hex::encode(format!("{marker}{key}").as_bytes())
}

/// The key an [`encode_page_token`] token carrying `marker` names; `None` for anything
/// this service did not mint, including well-formed hex carrying another list's marker.
pub fn decode_page_token(marker: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(marker).map(str::to_string)
}

/// What a `nextToken` of a FILTERED list of this service carries, beside the marker: the
/// list the page was taken from, and the record the next page starts at.
///
/// A token names a position in ONE list, so it carries a fingerprint of the SELECTION the
/// list was taken under ([`selection_fingerprint`]) — the criteria and value, the order and
/// the groups. Handed back with a different selection it is refused, rather than resuming
/// at whatever position that record happens to have in some other list. The page SIZE is
/// deliberately not part of it: `maxResults` is documented per call ("if a previous call to
/// this action returned 100 items, but you set maxResults to 10"), so changing it between
/// pages is the caller's business.
///
/// One cursor for every selected list of this service — the lists differ in what they hold
/// and not in how a page of them resumes — so the token shape cannot drift between them;
/// the marker is what tells the lists apart.
#[derive(Debug, Clone)]
pub struct ListCursor {
    /// The fingerprint of the selection the list was taken under.
    pub selection: String,
    /// The key of the first record the previous page did not report.
    pub resume_at: String,
}

impl ListCursor {
    /// This cursor as the opaque `nextToken` the caller hands back, through the service's
    /// one page-token codec under `marker`.
    pub fn to_token(&self, marker: &str) -> String {
        encode_page_token(marker, &format!("{}:{}", self.selection, self.resume_at))
    }

    /// The cursor a token carries, or `None` for anything the list owning `marker` did not
    /// mint.
    pub fn from_token(marker: &str, token: &str) -> Option<Self> {
        let payload = decode_page_token(marker, token)?;
        let (selection, resume_at) = payload.split_once(':')?;
        Some(Self {
            selection: selection.to_string(),
            resume_at: resume_at.to_string(),
        })
    }
}

/// A fingerprint of the selection a list was taken under: which records it holds and in
/// which order, independent of how they are paged.
///
/// A digest rather than the members themselves, so a token stays well inside `NextToken`'s
/// `@length(0, 2048)` however many groups the filter names — and so it carries nothing a
/// caller could read its own filter back out of.
pub fn selection_fingerprint(
    filter: Option<(&str, &str)>,
    descending: bool,
    groups: &[Group],
) -> String {
    use ::sha2::{Digest, Sha256};

    // The group filter is a SET: the same groups in another order select the same records,
    // so the fingerprint must not depend on the order they were sent in.
    let mut named: Vec<String> = groups
        .iter()
        .map(|group| {
            format!(
                "{:?}={:?}",
                group.name.as_deref().unwrap_or_default(),
                group.value.as_deref().unwrap_or_default()
            )
        })
        .collect();
    named.sort();
    let (criteria, value) = filter.unwrap_or((CRITERIA_NOT_DEFINED, ""));
    let order = if descending {
        SORT_DESCENDING
    } else {
        SORT_ASCENDING
    };
    let digest = <Sha256 as Digest>::digest(
        format!("{criteria:?}/{value:?}/{order}/{}", named.join(",")).as_bytes(),
    );
    // Half the digest: the fingerprint only has to tell two selections of one account's
    // portfolio apart, and a shorter token is a shorter `nextToken`.
    ::hex::encode(&digest[..16])
}

/// The page a list request asks for: how many records it takes, and the key it resumes
/// from.
#[derive(Debug, Clone)]
pub struct ListPage {
    /// How many records this page holds at most.
    pub size: usize,
    /// The key the page starts at, or `None` for a request that starts a list.
    pub resume_at: Option<String>,
}

/// Hold `maxResults` to the documented maximum and `nextToken` to a token the list owning
/// `marker` issued for THIS selection.
///
/// Every `maxResults` of this service is documented "The maximum number of items to include
/// in the response. The maximum value is 100" ([`MAX_LIST_PAGE_SIZE`]) and its target shape
/// carries no `@range`, so a request past that bound (or one asking for a page of nothing)
/// is refused here rather than silently clamped: a caller that asked for more than the
/// service gives has to learn it cannot have it. An absent `maxResults` takes the documented
/// maximum.
///
/// The refusals are this service's two `ValidationException` sentences — an unusable value
/// ([`invalid_request_body`]) and an unresolvable one ([`page_token_not_accepted`]) — which
/// each operation lifts into its own error channel.
pub fn list_page(
    max_results: Option<i32>,
    next_token: Option<String>,
    marker: &str,
    selection: &str,
) -> Result<ListPage, ValidationException> {
    let size = match max_results {
        None => MAX_LIST_PAGE_SIZE,
        Some(requested) if (1..=MAX_LIST_PAGE_SIZE).contains(&requested) => requested,
        Some(_) => return Err(invalid_request_body()),
    };
    let resume_at = match next_token {
        None => None,
        Some(token) => {
            let cursor =
                ListCursor::from_token(marker, &token).ok_or_else(page_token_not_accepted)?;
            if cursor.selection != selection {
                return Err(page_token_not_accepted());
            }
            Some(cursor.resume_at)
        }
    };
    Ok(ListPage {
        size: usize::try_from(size).unwrap_or(0),
        resume_at,
    })
}

/// The (criteria, value) pair a list request filters on, or `None` when it filters nothing.
///
/// A criteria other than [`CRITERIA_NOT_DEFINED`] names the member to compare, so it needs
/// a `filterValue` to compare against and is refused without one; `NOT_DEFINED` (and an
/// absent criteria) names no member, so it filters nothing and needs no value. Membership
/// in `declared` — the criteria enumeration's own generated value list — is checked here
/// because the boundary validator does not enforce it: a value the enumeration does not
/// declare is refused rather than quietly matching no record.
///
/// One rule for every criteria-filtered list of this service: each criteria enumeration
/// declares the same `NOT_DEFINED` member and each `filterValue` is documented the same way
/// ("Specify the value based on the … criteria type"), so the lists differ only in which
/// catalog they pass in and which member each value names.
pub fn filter_pair<'a>(
    criteria: Option<&'a str>,
    filter_value: Option<&'a str>,
    declared: &[&str],
) -> Result<Option<(&'a str, &'a str)>, ValidationException> {
    let criteria = criteria.unwrap_or(CRITERIA_NOT_DEFINED);
    if !declared.contains(&criteria) {
        return Err(invalid_request_body());
    }
    if criteria == CRITERIA_NOT_DEFINED {
        return Ok(None);
    }
    let value = filter_value.ok_or_else(invalid_request_body)?;
    Ok(Some((criteria, value)))
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
    /// [`RegionState::imported_server_count`]).
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
    ///
    /// Answers whether THIS call materialised it, which is what tells its caller the
    /// servers the assessment covers have to be moved off their collecting status too
    /// ([`RegionState::finish_server_collection`]).
    pub fn settle(&mut self, now: SystemTime) -> bool {
        let Some(due) = self.completion_due else {
            return false;
        };
        if now < due {
            return false;
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
        true
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

/// One recommendation report generation task, as this emulator persists it.
///
/// `StartRecommendationReportGeneration` mints it and `GetRecommendationReportDetails`
/// reports it; every member of the `RecommendationReportDetails` the read answers with is
/// computed from this record ([`Self::to_details`]).
#[derive(Debug, Clone)]
pub struct RecommendationReport {
    /// A `RecommendationTaskId` minted by
    /// [`ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState::mint_recommendation_report_id`].
    pub id: String,
    /// One of `RecommendationReportStatus`'s values.
    pub status: String,
    /// `RecommendationReportDetails$statusMessage`, set when the task reaches its
    /// terminal status.
    pub status_message: Option<String>,
    /// The `OutputFormat` the report was asked for, [`DEFAULT_OUTPUT_FORMAT`] when the
    /// request named none.
    pub output_format: String,
    /// How many servers of the portfolio the request's `groupIdFilter` selected when this
    /// task started — counted from the store, never from the request.
    ///
    /// The filter ITSELF is not kept: no member of `RecommendationReportDetails` reports
    /// it, so the record holds the count it produced rather than a caller-sized list for
    /// the life of the task.
    pub servers_covered: i32,
    pub start_time: SystemTime,
    pub completion_time: Option<SystemTime>,
    /// The instant the clock owes this task its terminal status, while it has one. It is
    /// also what makes the task "in progress": a start call made while one is owed
    /// coalesces onto this task rather than minting a second one.
    pub completion_due: Option<SystemTime>,
}

impl RecommendationReport {
    /// Materialise the terminal status the clock owes this task, if `now` is past its due
    /// instant.
    ///
    /// `completion_time` is the DUE instant rather than `now`, so a task reports the same
    /// completion time however late it is first read. The status is [`REPORT_SUCCESS`]:
    /// see that constant for what the accompanying message reports.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.completion_due else {
            return;
        };
        if now < due {
            return;
        }
        self.status = REPORT_SUCCESS.to_string();
        self.completion_time = Some(due);
        self.completion_due = None;
        self.status_message = Some(format!(
            "{} server(s) of the portfolio were in scope. No {} recommendation report file was \
             written: this emulator serves no Amazon S3 data plane to upload one to.",
            self.servers_covered, self.output_format
        ));
    }

    /// True while this task has not reached a terminal `RecommendationReportStatus`, so a
    /// new start call coalesces onto it.
    pub fn is_in_progress(&self) -> bool {
        self.completion_due.is_some()
    }

    /// The `RecommendationReportDetails` `GetRecommendationReportDetails` reports: this
    /// record's status, message and timestamps, and nothing derived from the request that
    /// reads it.
    ///
    /// `s3Bucket` and `s3Keys` are left unset — they name where the report FILE is, and
    /// no file was written (see [`REPORT_SUCCESS`]), so there is no bucket holding one and
    /// no key naming one.
    pub fn to_details(&self) -> RecommendationReportDetails {
        RecommendationReportDetails {
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            completion_time: self
                .completion_time
                .map(::aws_smithy_types::DateTime::from),
            s3_bucket: None,
            s3_keys: None,
        }
    }
}

/// The criteria value that names no member to filter on, so a list request carrying it (or
/// no criteria at all) filters nothing.
///
/// Each criteria enumeration of this service declares it — `ServerCriteria`
/// (`SERVER_CRITERIA_VALUES`) and `ApplicationComponentCriteria`
/// (`APPLICATION_COMPONENT_CRITERIA_VALUES`) — and in both every OTHER value names a member
/// of the record the list reports, so the one that names none is stated once here.
pub const CRITERIA_NOT_DEFINED: &str = "NOT_DEFINED";

/// The `ServerCriteria` value that names no member to filter on, so a request carrying it
/// (or no `serverCriteria` at all) filters nothing — the service-wide
/// [`CRITERIA_NOT_DEFINED`] under the name the server inventory reads it by.
pub const SERVER_CRITERIA_NOT_DEFINED: &str = CRITERIA_NOT_DEFINED;

/// `SortOrder`'s ascending value, which is the order a list is reported in when the
/// request names no `sort`.
pub const SORT_ASCENDING: &str = "ASC";

/// `SortOrder`'s descending value.
pub const SORT_DESCENDING: &str = "DESC";

/// `StrategyRecommendation`'s value for a strategy the service itself recommends: the
/// status the recommendation set stored on a server is reported with.
///
/// The other three values this enumeration declares (`viableOption`, `notRecommended`,
/// `potential`) rank a strategy some analysis considered and did not recommend. Nothing in
/// this emulator ranks strategies, so no strategy is reported with one of them, and a
/// strategy nothing recommended is reported with no status at all rather than with a rank
/// nothing computed.
pub const STRATEGY_RECOMMENDED: &str = "recommended";

/// `SrcCodeOrDbAnalysisStatus`'s value for an application component whose source code
/// and/or database credentials are configured and for which no analysis was asked for.
///
/// `UpdateApplicationComponentConfigRequest$configureOnly` is documented "If it is set to
/// true, the source code and/or database credentials are updated. If it is set to false,
/// the source code and/or database credentials are updated and an analysis is initiated",
/// so a configure-only update leaves the component configured and unanalysed.
pub const ANALYSIS_CONFIGURED: &str = "CONFIGURED";

/// `SrcCodeOrDbAnalysisStatus`'s value for an application component whose analysis has
/// been asked for and not yet scheduled.
///
/// It is where an update that asked for an analysis (the same documented sentence, with
/// `configureOnly` false or absent) leaves the component, and where it STAYS: nothing in
/// this emulator analyses source code or a database, so no analyzer picks the queued
/// analysis up and no result, success or failure is ever reported for work nothing ran.
pub const ANALYSIS_TO_BE_SCHEDULED: &str = "ANALYSIS_TO_BE_SCHEDULED";

/// The `RecommendationSet` a stored strategy and destination amount to, or `None` when
/// neither is held.
///
/// A recommendation of this emulator is a strategy and/or a destination and carries no
/// transformation tool, because nothing here picks one. Both the server inventory and the
/// application components hold those two members, so both report their recommendation
/// through this one function.
fn recommendation_of(
    strategy: &Option<String>,
    target_destination: &Option<String>,
) -> Option<RecommendationSet> {
    (strategy.is_some() || target_destination.is_some()).then(|| RecommendationSet {
        transformation_tool: None,
        target_destination: target_destination.clone(),
        strategy: strategy.clone(),
    })
}

/// The `RecommendationSet` a stored preference names.
///
/// `StrategyOption` and `RecommendationSet` carry the same three things under different
/// names — a strategy, a destination and a tool — so the preference is reported as the
/// recommendation it selects, its `toolName` becoming the `TransformationTool` the option
/// named and nothing more (no description and no installation link: the caller named a
/// tool, not a tool catalog entry).
fn recommendation_of_option(option: &StrategyOption) -> RecommendationSet {
    RecommendationSet {
        transformation_tool: option.tool_name.clone().map(|name| TransformationTool {
            name: Some(name),
            description: None,
            tranformation_tool_installation_link: None,
        }),
        target_destination: option.target_destination.clone(),
        strategy: option.strategy.clone(),
    }
}

/// One entry of the strategy list a read reports for a resource of this service.
///
/// `ServerStrategy` and `ApplicationComponentStrategy` declare the same three members (the
/// server's one also counts the application components running the strategy), so the list
/// is decided once here and each read maps it into its own shape.
struct StrategyListing {
    recommendation: RecommendationSet,
    status: Option<String>,
    is_preferred: bool,
}

/// The strategies a read reports for a resource holding the recommendation `analyzed` and
/// the preference `preferred`: the option the account prefers first, then the service's
/// own recommendation when it is a different one.
///
/// Both come from the resource's own record, so one nothing recommended and nobody chose a
/// preference for reports NO strategy rather than a canned list. `isPreferred` is "set to
/// true if the recommendation is set as preferred", so it is true exactly for the option an
/// update stored (the flag that call carried when it carried one) and false for a
/// recommendation nobody chose. A preference naming the same strategy and destination as
/// the service's recommendation is ONE strategy, not two, and is reported once — carrying
/// the recommended status, since it is also what the service recommends.
fn strategy_listings(
    analyzed: Option<RecommendationSet>,
    preferred: Option<&StrategyOption>,
) -> Vec<StrategyListing> {
    let preferred_set = preferred.map(recommendation_of_option);
    let same_strategy = |left: &RecommendationSet, right: &RecommendationSet| {
        left.strategy == right.strategy && left.target_destination == right.target_destination
    };
    let also_recommended = match (&analyzed, &preferred_set) {
        (Some(analyzed), Some(preferred)) => same_strategy(analyzed, preferred),
        _ => false,
    };
    let mut listings: Vec<StrategyListing> = Vec::new();
    if let Some(recommendation) = preferred_set {
        listings.push(StrategyListing {
            recommendation,
            status: also_recommended.then(|| STRATEGY_RECOMMENDED.to_string()),
            is_preferred: preferred
                .and_then(|option| option.is_preferred)
                .unwrap_or(true),
        });
    }
    if let Some(recommendation) = analyzed.filter(|_| !also_recommended) {
        listings.push(StrategyListing {
            recommendation,
            status: Some(STRATEGY_RECOMMENDED.to_string()),
            is_preferred: false,
        });
    }
    listings
}

/// What a data collector fetched about one server when it discovered it — the
/// `AnalyzableServerSummary` members, held on the server they describe.
///
/// `ListAnalyzableServers` is documented "Retrieves a list of all the servers fetched from
/// customer vCenter using Strategy Recommendation Collector", so an analyzable server is a
/// server the service holds a collector's discovery data for. That data is held HERE,
/// beside the rest of the server's record ([`ServerRecord::discovery`]), rather than in a
/// second collection of servers: a discovered server and a portfolio server are one
/// machine, and `AnalyzableServerSummary` carries no id of its own that could key a
/// separate one.
///
/// Every member is what the discovery stored, so a read derives nothing from the request
/// that reads it, and a member the discovery did not carry stays `None` rather than being
/// filled with a value nothing fetched. None of these four members is a member of
/// `ServerDetail`, which is why `ListServers` reports none of them: the model gives the
/// server's addresses to `SystemInfo$networkInfoList`, whose `NetworkInfo` requires an
/// interface name, a MAC address and a subnet mask that no part of this emulator holds.
#[derive(Debug, Clone, Default)]
pub struct ServerDiscovery {
    /// `AnalyzableServerSummary$hostname` — "The host name of the analyzable server."
    pub hostname: Option<String>,
    /// `AnalyzableServerSummary$ipAddress` — "The ip address of the analyzable server."
    pub ip_address: Option<String>,
    /// `AnalyzableServerSummary$source` — "The data source of the analyzable server."
    ///
    /// A free-text string in the model (it targets `String`, not `DataSourceType`), so it
    /// is held as the discovery recorded it.
    pub source: Option<String>,
    /// `AnalyzableServerSummary$vmId` — "The virtual machine id of the analyzable server."
    pub vm_id: Option<String>,
}

/// One server of this account's portfolio, as this emulator persists it.
///
/// A server enters the portfolio when an import file task ingests it — that ingestion
/// (and the record counts behind it) is filed separately as the import-loop issue, which
/// is why a freshly booted emulator holds no server at all. This record is what the read
/// path computes from: `ListServers` reports it as a [`ServerDetail`], filters it by the
/// member a `ServerCriteria` names, and scopes it by the import groups it came in with.
///
/// Every member is what the import stored, so nothing here is derived from the request
/// that reads it.
#[derive(Debug, Clone, Default)]
pub struct ServerRecord {
    /// `ServerDetail$id` — a `ResourceId` (`^[0-9a-b]+`, up to 44 characters). It is also
    /// this server's key in [`RegionState::servers`],
    /// which is what gives a list a stable total order to sort and paginate over.
    pub id: String,
    /// `ServerDetail$name`.
    pub name: Option<String>,
    /// `ServerDetail$serverType`.
    pub server_type: Option<String>,
    /// `SystemInfo$osInfo$type` — one of `OSType`'s values.
    pub os_type: Option<String>,
    /// `SystemInfo$osInfo$version` — the free-text operating system the server reports.
    pub os_version: Option<String>,
    /// `SystemInfo$fileSystemType`.
    pub file_system_type: Option<String>,
    /// `SystemInfo$cpuArchitecture`.
    pub cpu_architecture: Option<String>,
    /// `ServerDetail$dataCollectionStatus` — one of `RunTimeAssessmentStatus`'s values.
    ///
    /// Documented "The status of assessment for the server", so it is the per-server face
    /// of the assessment covering this server: `StartAssessment` puts it in
    /// [`COLLECTION_STARTED`] ([`RegionState::begin_server_collection`]) and the terminal
    /// status the assessment reaches moves it to [`COLLECTION_STOPPED`] or
    /// [`COLLECTION_SUCCESS`] ([`RegionState::finish_server_collection`]). `None` for a
    /// server no assessment ever covered.
    pub data_collection_status: Option<String>,
    /// `ServerDetail$statusMessage`.
    pub status_message: Option<String>,
    /// `RecommendationSet$strategy` — one of `Strategy`'s values, once an analysis
    /// recommended one.
    pub strategy: Option<String>,
    /// `RecommendationSet$targetDestination` — one of `TargetDestination`'s values.
    pub target_destination: Option<String>,
    /// `ServerError$serverErrorCategory` — one of `ServerErrorCategory`'s values, for a
    /// server whose analysis failed.
    pub error_category: Option<String>,
    /// `ServerDetail$lastAnalyzedTimestamp`.
    pub last_analyzed: Option<SystemTime>,
    /// The import groups this server came in with — `StartImportFileTaskRequest$groupId`
    /// of the import that brought it in. Not a member of `ServerDetail`: it is what
    /// `ListServers`'s `groupIdFilter` scopes a list by.
    pub group_id: Vec<Group>,
    /// The strategy option the account PREFERS for this server, as `UpdateServerConfig`
    /// stored it — member for member, so nothing here is derived from the request that
    /// reads it. `None` until a caller sets one.
    ///
    /// It records the CUSTOMER's choice rather than any analysis, so it is held beside the
    /// recommendation members above instead of overwriting them: `GetServerStrategies`
    /// reports both, and `ServerStrategy$isPreferred` is the member the model gives the
    /// distinction to.
    pub preferred_strategy: Option<StrategyOption>,
    /// The applications this server belongs to — `AssociatedApplication` is "Object
    /// containing details about applications as defined in Application Discovery Service"
    /// — as the import that brought the server in stored them, in the order it stored
    /// them.
    ///
    /// Not a member of `ServerDetail`: it is the list `GetServerDetails` reports BESIDE
    /// the detail, and the one that operation pages over. Empty for a server no
    /// application was recorded for, so that read answers an empty list rather than an
    /// application nothing holds.
    pub associated_applications: Vec<AssociatedApplication>,
    /// The discovery data a collector fetched for this server ([`ServerDiscovery`]), or
    /// `None` for a server no collector fetched — a server that came in some other way
    /// (an import file, Application Discovery Service) and that the service therefore
    /// holds no vCenter discovery data for.
    ///
    /// It is this member that makes a server ANALYZABLE: `ListAnalyzableServers` reports
    /// the servers that carry it ([`Self::to_analyzable_summary`]) and no others, so a
    /// server nothing discovered is not reported as an empty summary.
    pub discovery: Option<ServerDiscovery>,
}

impl ServerRecord {
    /// The `ServerDetail` a read reports for this server: this record's own members, and
    /// nothing for the ones no part of this emulator produces.
    ///
    /// `listAntipatternSeveritySummary`, `antipatternReport*` and
    /// `applicationComponentStrategySummary` are left unset rather than filled with a
    /// shape nothing computed — no anti-pattern report is generated and application
    /// components are a resource of their own — and the nested structures are reported
    /// only when the record holds something to put in them, so a server the import knew
    /// nothing about does not come back as a tree of empty objects.
    pub fn to_server_detail(&self) -> ServerDetail {
        let os_info = (self.os_type.is_some() || self.os_version.is_some()).then(|| OSInfo {
            r#type: self.os_type.clone(),
            version: self.os_version.clone(),
        });
        let system_info = (os_info.is_some()
            || self.file_system_type.is_some()
            || self.cpu_architecture.is_some())
        .then(|| SystemInfo {
            os_info,
            file_system_type: self.file_system_type.clone(),
            // The import brings no interface, address or mask with it, so there is no
            // network information to report.
            network_info_list: None,
            cpu_architecture: self.cpu_architecture.clone(),
        });
        let recommendation_set = self.analyzed_recommendation();
        ServerDetail {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            recommendation_set,
            data_collection_status: self.data_collection_status.clone(),
            status_message: self.status_message.clone(),
            list_antipattern_severity_summary: None,
            system_info,
            application_component_strategy_summary: None,
            antipattern_report_s3_object: None,
            antipattern_report_status: None,
            antipattern_report_status_message: None,
            server_type: self.server_type.clone(),
            last_analyzed_timestamp: self.last_analyzed.map(::aws_smithy_types::DateTime::from),
            server_error: self.error_category.clone().map(|category| ServerError {
                server_error_category: Some(category),
            }),
        }
    }

    /// True when this server satisfies `criteria` = `value`.
    ///
    /// Each `ServerCriteria` value names ONE member of the server record and the value is
    /// compared to it for equality — `filterValue` is documented as "the filter value,
    /// which is based on the type of server criteria … if `serverCriteria` is `OS_NAME`,
    /// and the `filterValue` is equal to `WindowsServer`, then `ListServers` returns all
    /// of the servers matching the OS name `WindowsServer`". `OS_NAME` is the one criteria
    /// whose member the model splits in two — `SystemInfo$osInfo` carries an `OSType` and
    /// a free-text version — so a value equal to either selects the server.
    ///
    /// [`SERVER_CRITERIA_NOT_DEFINED`] names no member, so it selects every server. A
    /// value `ServerCriteria` does not declare selects none; a caller never sees that,
    /// because the handler refuses an undeclared criteria before it gets here.
    pub fn matches_criteria(&self, criteria: &str, value: &str) -> bool {
        match criteria {
            SERVER_CRITERIA_NOT_DEFINED => true,
            "SERVER_ID" => self.id == value,
            "OS_NAME" => {
                self.os_type.as_deref() == Some(value) || self.os_version.as_deref() == Some(value)
            }
            "STRATEGY" => self.strategy.as_deref() == Some(value),
            "DESTINATION" => self.target_destination.as_deref() == Some(value),
            "ANALYSIS_STATUS" => self.data_collection_status.as_deref() == Some(value),
            "ERROR_CATEGORY" => self.error_category.as_deref() == Some(value),
            _ => false,
        }
    }

    /// The `RecommendationSet` the service's own recommendation for this server amounts
    /// to, or `None` when it holds none.
    ///
    /// The same two members `to_server_detail` reports: a recommendation is a strategy
    /// and/or a destination, and no transformation tool is part of it, because nothing in
    /// this emulator picks one.
    fn analyzed_recommendation(&self) -> Option<RecommendationSet> {
        recommendation_of(&self.strategy, &self.target_destination)
    }

    /// The strategies a read reports for this server — [`strategy_listings`] as
    /// `ServerStrategy`.
    ///
    /// `numberOfApplicationComponents` is left unset: the component records this service
    /// holds carry the servers they run on, but nothing here attributes a component to a
    /// strategy of a server, so a count would be a number nothing computed.
    pub fn to_server_strategies(&self) -> Vec<ServerStrategy> {
        strategy_listings(
            self.analyzed_recommendation(),
            self.preferred_strategy.as_ref(),
        )
        .into_iter()
        .map(|listing| ServerStrategy {
            recommendation: Some(listing.recommendation),
            status: listing.status,
            number_of_application_components: None,
            is_preferred: Some(listing.is_preferred),
        })
        .collect()
    }

    /// The `AnalyzableServerSummary` a read reports for this server, or `None` when no
    /// collector discovered it and the service therefore holds nothing to summarise.
    ///
    /// The four members of the stored [`ServerDiscovery`], member for member and nothing
    /// else: `AnalyzableServerSummary` declares exactly those four, so a summary carries
    /// what the discovery fetched and leaves absent what it did not.
    pub fn to_analyzable_summary(&self) -> Option<AnalyzableServerSummary> {
        let discovery = self.discovery.as_ref()?;
        Some(AnalyzableServerSummary {
            hostname: discovery.hostname.clone(),
            ip_address: discovery.ip_address.clone(),
            source: discovery.source.clone(),
            vm_id: discovery.vm_id.clone(),
        })
    }

    /// True when this server came in with one of `groups` — the service's one
    /// [`in_any_group`], over the groups this record holds.
    pub fn in_any_group(&self, groups: &[Group]) -> bool {
        in_any_group(&self.group_id, groups)
    }
}

/// True when a resource that came in with `held` belongs to one of the groups `wanted`
/// names.
///
/// A `Group` is a (`name`, `value`) pair — "The object containing information about
/// distinct imports or groups" — so a resource is in a filter's group only when both halves
/// agree, and several groups in a filter widen the selection rather than narrowing it: a
/// `groupIdFilter` names the groups whose resources to report.
///
/// An EMPTY `wanted` answers `false`, which is why a caller that named no group at all must
/// not reach here: naming no group scopes nothing, so a handler applies this only to a
/// non-empty filter. One rule for every list of this service that takes a `groupIdFilter`,
/// so a server and an application component are scoped by a filter the same way.
fn in_any_group(held: &[Group], wanted: &[Group]) -> bool {
    wanted.iter().any(|wanted| {
        held.iter()
            .any(|held| held.name == wanted.name && held.value == wanted.value)
    })
}

/// One application component of this account's portfolio, as this emulator persists it.
///
/// An application component is "the process or database running on a server" that the
/// portfolio holds a recommendation for, so it is a resource of its own beside
/// [`ServerRecord`] rather than a member of it. Like a server it enters this service
/// through the data the account brings in (an import file task's ingestion, filed
/// separately as the import-loop issue), which is why a freshly booted emulator holds no
/// application component at all.
///
/// Every member below is what that ingestion stored, so nothing in a read is derived from
/// the request that reads it: `GetApplicationComponentDetails` reports this record as the
/// `ApplicationComponentDetail` the model declares ([`Self::to_detail`]) beside the
/// associations it holds.
#[derive(Debug, Clone, Default)]
pub struct ApplicationComponentRecord {
    /// `ApplicationComponentDetail$id` — a `ResourceId` (`^[0-9a-b]+`, up to 44
    /// characters). It is also this component's key in
    /// [`RegionState::application_components`],
    /// which is what gives a list over them a stable total order.
    pub id: String,
    /// `ApplicationComponentDetail$name`.
    pub name: Option<String>,
    /// `ApplicationComponentDetail$appType` — one of `AppType`'s values
    /// ([`APP_TYPE_VALUES`](crate::ops::types::APP_TYPE_VALUES)).
    pub app_type: Option<String>,
    /// `ApplicationComponentDetail$resourceSubType` — one of `ResourceSubType`'s values.
    pub resource_sub_type: Option<String>,
    /// `ApplicationComponentDetail$inclusionStatus` — one of `InclusionStatus`'s values:
    /// whether the component is included in server recommendation.
    pub inclusion_status: Option<String>,
    /// `ApplicationComponentDetail$analysisStatus` — one of
    /// `SrcCodeOrDbAnalysisStatus`'s values, for a component with source code or a
    /// database attached.
    pub analysis_status: Option<String>,
    /// `ApplicationComponentDetail$statusMessage`.
    pub status_message: Option<String>,
    /// `RecommendationSet$strategy` — one of `Strategy`'s values, once an analysis
    /// recommended one for this component.
    pub strategy: Option<String>,
    /// `RecommendationSet$targetDestination` — one of `TargetDestination`'s values.
    pub target_destination: Option<String>,
    /// `ApplicationComponentDetail$osVersion`.
    pub os_version: Option<String>,
    /// `ApplicationComponentDetail$osDriver`.
    pub os_driver: Option<String>,
    /// `ApplicationComponentDetail$lastAnalyzedTimestamp`.
    pub last_analyzed: Option<SystemTime>,
    /// `DatabaseConfigDetail$secretName` — the Secrets Manager secret the component's
    /// database is reached with, for a component that has one.
    pub database_secret_name: Option<String>,
    /// `ApplicationComponentDetail$sourceCodeRepositories`, in the order they were
    /// stored. Empty for a component no repository was recorded for.
    pub source_code_repositories: Vec<SourceCodeRepository>,
    /// `ApplicationComponentDetail$runtimeStatus` — one of `RuntimeAnalysisStatus`'s
    /// values.
    pub runtime_status: Option<String>,
    /// `ApplicationComponentDetail$runtimeStatusMessage`.
    pub runtime_status_message: Option<String>,
    /// `AppUnitError$appUnitErrorCategory` — one of `AppUnitErrorCategory`'s values, for
    /// a component whose source-code or database analysis failed.
    pub app_unit_error_category: Option<String>,
    /// The ids of the servers this component runs on, in the order they were stored —
    /// `GetApplicationComponentDetailsResponse$associatedServerIds`, "a list of the IDs of
    /// the servers on which the application component is running".
    ///
    /// The detail's own `associatedServerId` ("the ID of the server that the application
    /// component is running on") is the first of them and
    /// `moreServerAssociationExists` is whether there is more than one, so the single
    /// member and the list can never disagree. Empty for a component no server
    /// association was recorded for.
    pub associated_server_ids: Vec<String>,
    /// The application groups this component belongs to, as defined in Application
    /// Discovery Service, in the order they were stored —
    /// `GetApplicationComponentDetailsResponse$associatedApplications`.
    ///
    /// `moreApplicationResource` ("set to true if the application component belongs to
    /// more than one application group") is whether there is more than one, counted off
    /// this list.
    pub associated_applications: Vec<AssociatedApplication>,
    /// The strategy option the account PREFERS for this component, as
    /// `UpdateApplicationComponentConfig` stored it — member for member, so nothing here
    /// is derived from the request that reads it. `None` until a caller sets one.
    ///
    /// Like the server inventory's preference ([`ServerRecord::preferred_strategy`]) it
    /// records the CUSTOMER's choice rather than any analysis, so it is held beside the
    /// recommendation members above instead of overwriting them:
    /// `GetApplicationComponentStrategies` reports both, and
    /// `ApplicationComponentStrategy$isPreferred` is the member the model gives the
    /// distinction to.
    pub preferred_strategy: Option<StrategyOption>,
    /// The import groups this component came in with —
    /// `StartImportFileTaskRequest$groupId` of the import that brought it in, whose
    /// documentation is "This ID can be as filter in ListApplicationComponents and
    /// ListServers".
    ///
    /// Not a member of `ApplicationComponentDetail`: like the server inventory's own groups
    /// ([`ServerRecord::group_id`]) it is what a list's `groupIdFilter` scopes the answer
    /// by. Empty for a component that came in with no group.
    pub group_id: Vec<Group>,
}

impl ApplicationComponentRecord {
    /// The `ApplicationComponentDetail` a read reports for this component: this record's
    /// own members, and nothing for the ones no part of this emulator produces.
    ///
    /// `listAntipatternSeveritySummary` and `antipatternReport*` are left unset rather
    /// than filled with a shape nothing computed — this emulator generates no
    /// anti-pattern report — and so is `resultList`, the list of analysis results, since
    /// nothing here analyses source code or a database. The nested structures are
    /// reported only when the record holds something to put in them, so a component the
    /// ingestion knew little about does not come back as a tree of empty objects.
    pub fn to_detail(&self) -> ApplicationComponentDetail {
        let recommendation_set = self.analyzed_recommendation();
        ApplicationComponentDetail {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            recommendation_set,
            analysis_status: self.analysis_status.clone(),
            status_message: self.status_message.clone(),
            list_antipattern_severity_summary: None,
            database_config_detail: self.database_secret_name.clone().map(|secret_name| {
                DatabaseConfigDetail {
                    secret_name: Some(secret_name),
                }
            }),
            source_code_repositories: (!self.source_code_repositories.is_empty())
                .then(|| self.source_code_repositories.clone()),
            app_type: self.app_type.clone(),
            resource_sub_type: self.resource_sub_type.clone(),
            inclusion_status: self.inclusion_status.clone(),
            antipattern_report_s3_object: None,
            antipattern_report_status: None,
            antipattern_report_status_message: None,
            os_version: self.os_version.clone(),
            os_driver: self.os_driver.clone(),
            last_analyzed_timestamp: self.last_analyzed.map(::aws_smithy_types::DateTime::from),
            associated_server_id: self.associated_server_ids.first().cloned(),
            more_server_association_exists: Some(self.associated_server_ids.len() > 1),
            runtime_status: self.runtime_status.clone(),
            runtime_status_message: self.runtime_status_message.clone(),
            app_unit_error: self
                .app_unit_error_category
                .clone()
                .map(|category| AppUnitError {
                    app_unit_error_category: Some(category),
                }),
            result_list: None,
        }
    }

    /// Whether this component belongs to more than one application group —
    /// `GetApplicationComponentDetailsResponse$moreApplicationResource`.
    ///
    /// Counted off [`Self::associated_applications`], the very list the same answer
    /// reports, so the flag and the list cannot drift apart.
    pub fn more_application_resource(&self) -> bool {
        self.associated_applications.len() > 1
    }

    /// The `RecommendationSet` the service's own recommendation for this component amounts
    /// to, or `None` when it holds none — the same two members [`Self::to_detail`]
    /// reports.
    fn analyzed_recommendation(&self) -> Option<RecommendationSet> {
        recommendation_of(&self.strategy, &self.target_destination)
    }

    /// The strategies a read reports for this component — [`strategy_listings`] as
    /// `ApplicationComponentStrategy`: the option the account prefers first, then the
    /// service's own recommendation when it is a different one.
    pub fn to_component_strategies(&self) -> Vec<ApplicationComponentStrategy> {
        strategy_listings(
            self.analyzed_recommendation(),
            self.preferred_strategy.as_ref(),
        )
        .into_iter()
        .map(|listing| ApplicationComponentStrategy {
            recommendation: Some(listing.recommendation),
            status: listing.status,
            is_preferred: Some(listing.is_preferred),
        })
        .collect()
    }

    /// True when this component satisfies `criteria` = `value`.
    ///
    /// Each `ApplicationComponentCriteria` value names ONE member of the component record
    /// and the value is compared to it for equality — `filterValue` is documented "Specify
    /// the value based on the application component criteria type. For example, if
    /// `applicationComponentCriteria` is set to `SERVER_ID` and `filterValue` is set to
    /// `server1`, then `ListApplicationComponents` returns all the application components
    /// running on `server1`". `SERVER_ID` is the one criteria whose member is a LIST (a
    /// component runs on one or more servers), so a value equal to any of the servers the
    /// component runs on selects it.
    ///
    /// [`CRITERIA_NOT_DEFINED`] names no member, so it selects every component. A value
    /// `ApplicationComponentCriteria` does not declare selects none; a caller never sees
    /// that, because the handler refuses an undeclared criteria before it gets here.
    pub fn matches_criteria(&self, criteria: &str, value: &str) -> bool {
        match criteria {
            CRITERIA_NOT_DEFINED => true,
            "APP_NAME" => self.name.as_deref() == Some(value),
            "APP_TYPE" => self.app_type.as_deref() == Some(value),
            "SERVER_ID" => self
                .associated_server_ids
                .iter()
                .any(|server| server == value),
            "STRATEGY" => self.strategy.as_deref() == Some(value),
            "DESTINATION" => self.target_destination.as_deref() == Some(value),
            "ANALYSIS_STATUS" => self.analysis_status.as_deref() == Some(value),
            "ERROR_CATEGORY" => self.app_unit_error_category.as_deref() == Some(value),
            _ => false,
        }
    }

    /// True when this component came in with one of `groups` — the service's one
    /// [`in_any_group`], over the groups this record holds.
    pub fn in_any_group(&self, groups: &[Group]) -> bool {
        in_any_group(&self.group_id, groups)
    }
}

/// One data collector installed in the account's environment, as this emulator persists
/// it.
///
/// A collector is NOT created through this API: `Collector` is documented "Process data
/// collector that runs in the environment that you specify" and `registeredTimeStamp` is
/// "Time when the collector registered with the service", so a collector enters the store
/// when the appliance installed on-premises registers itself — a path outside the modelled
/// surface, which is why a freshly booted emulator holds none and `ListCollectors` answers
/// the empty list it honestly holds.
///
/// Every member is what that registration recorded, so a read derives nothing from the
/// request that reads it and a member the registration did not carry stays `None` rather
/// than being filled with a value nothing reported. The two time stamps are held as
/// strings because the model types them as `String` rather than as a `timestamp`
/// (`Collector$registeredTimeStamp`, `Collector$lastActivityTimeStamp`): the record keeps
/// exactly the text the collector reported.
#[derive(Debug, Clone, Default)]
pub struct CollectorRecord {
    /// `Collector$collectorId` — a `String` bounded to 1024 with pattern `.*\S.*`. It is
    /// also this collector's key in [`RegionState::collectors`], which is what gives the
    /// list a stable total order to paginate over.
    pub id: String,
    /// `Collector$ipAddress` — "IP address of the server that is hosting the collector."
    pub ip_address: Option<String>,
    /// `Collector$hostName` — "Hostname of the server that is hosting the collector."
    pub host_name: Option<String>,
    /// `Collector$collectorHealth`, one of `CollectorHealth`'s values
    /// ([`COLLECTOR_HEALTH_VALUES`](crate::ops::types::COLLECTOR_HEALTH_VALUES)).
    pub health: Option<String>,
    /// `Collector$collectorVersion` — the version of the collector running in the
    /// environment.
    pub version: Option<String>,
    /// `Collector$registeredTimeStamp` — when the collector registered with the service.
    pub registered_time_stamp: Option<String>,
    /// `Collector$lastActivityTimeStamp` — when the collector last pinged the service.
    pub last_activity_time_stamp: Option<String>,
    /// `Collector$configurationSummary` — the remote sources this collector is configured
    /// to analyse, as the collector reported them. `None` for a collector that reported no
    /// configuration, so the read answers nothing rather than a tree of empty lists.
    pub configuration_summary: Option<ConfigurationSummary>,
}

impl CollectorRecord {
    /// The `Collector` a read reports for this record: its own members, and nothing for
    /// the ones the registration did not carry.
    pub fn to_collector(&self) -> Collector {
        Collector {
            collector_id: Some(self.id.clone()),
            ip_address: self.ip_address.clone(),
            host_name: self.host_name.clone(),
            collector_health: self.health.clone(),
            collector_version: self.version.clone(),
            registered_time_stamp: self.registered_time_stamp.clone(),
            last_activity_time_stamp: self.last_activity_time_stamp.clone(),
            configuration_summary: self.configuration_summary.clone(),
        }
    }
}

/// One page of collectors, as `ListCollectors` answers it.
#[derive(Debug, Default)]
pub struct CollectorPage {
    /// The collectors of this page, in the store's total order.
    pub collectors: Vec<Collector>,
    /// The cursor the next call resumes from, set only when another collector follows
    /// this page.
    pub next_token: Option<String>,
}

/// The account's migration and modernization preferences, as `PutPortfolioPreferences`
/// saved them and `GetPortfolioPreferences` reports them.
///
/// One record per account — the API names no id and takes no input on the read, so there
/// is exactly one of these in the store. `PutPortfolioPreferences` *saves* the
/// preferences, so the record it writes is the whole of the request: a member the request
/// leaves out is not set afterwards, however an earlier call left it. Every member is
/// optional in both directions, so a preference nobody expressed comes back absent rather
/// than as a default nothing chose.
#[derive(Debug, Clone, Default)]
pub struct PortfolioPreferences {
    /// `PutPortfolioPreferencesRequest$prioritizeBusinessGoals`.
    pub prioritize_business_goals: Option<PrioritizeBusinessGoals>,
    /// `PutPortfolioPreferencesRequest$applicationPreferences`.
    pub application_preferences: Option<ApplicationPreferences>,
    /// `PutPortfolioPreferencesRequest$databasePreferences`.
    pub database_preferences: Option<DatabasePreferences>,
    /// `PutPortfolioPreferencesRequest$applicationMode`, one of `ApplicationMode`'s
    /// values ([`APPLICATION_MODE_VALUES`](crate::ops::types::APPLICATION_MODE_VALUES)).
    pub application_mode: Option<String>,
}

impl PortfolioPreferences {
    /// The `GetPortfolioPreferences` answer: this record, member for member.
    ///
    /// The read reports what the save stored and derives nothing, so the two operations
    /// cannot drift apart.
    pub fn to_output(&self) -> GetPortfolioPreferencesOutput {
        GetPortfolioPreferencesOutput {
            prioritize_business_goals: self.prioritize_business_goals.clone(),
            application_preferences: self.application_preferences.clone(),
            database_preferences: self.database_preferences.clone(),
            application_mode: self.application_mode.clone(),
        }
    }
}

/// One Region's portfolio: the resources of this service a request signed for that
/// Region can observe, and the only ones it can.
#[derive(Debug, Default)]
pub struct RegionState {
    /// Every import file task, keyed by its id. A `BTreeMap` is what gives
    /// `ListImportFileTask` a stable total order to paginate over.
    pub import_tasks: BTreeMap<String, ImportFileTask>,
    /// Every assessment, keyed by its id.
    pub assessments: BTreeMap<String, Assessment>,
    /// Every server of this account's portfolio, keyed by its id — the inventory
    /// `ListServers` reports. A `BTreeMap` is what gives that list a stable total order
    /// to sort and paginate over, and it is empty until an import ingests a server.
    pub servers: BTreeMap<String, ServerRecord>,
    /// Every application component of this account's portfolio, keyed by its id — the
    /// processes and databases running on the servers above, which
    /// `GetApplicationComponentDetails` reports one at a time. A `BTreeMap` is what gives
    /// a list over them a stable total order, and it is empty until the account's data is
    /// brought in.
    pub application_components: BTreeMap<String, ApplicationComponentRecord>,
    /// Every recommendation report generation task, keyed by its id: what
    /// `StartRecommendationReportGeneration` mints and `GetRecommendationReportDetails`
    /// reports.
    pub recommendation_reports: BTreeMap<String, RecommendationReport>,
    /// Every data collector installed in this account's environment, keyed by its id —
    /// the collectors `ListCollectors` reports. A `BTreeMap` is what gives that list a
    /// stable total order to paginate over, and it is empty until a collector registers
    /// itself (see [`CollectorRecord`]).
    pub collectors: BTreeMap<String, CollectorRecord>,
    /// The id of the most recently started assessment, which is what
    /// `GetLatestAssessmentId` answers. `None` until one is started.
    pub latest_assessment_id: Option<String>,
    /// This account's migration and modernization preferences in this Region, written by
    /// `PutPortfolioPreferences` and reported by `GetPortfolioPreferences`. `None` until a
    /// save stores them: no preference is set in a fresh account.
    pub portfolio_preferences: Option<PortfolioPreferences>,
}

impl RegionState {
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
    /// materialised — on the assessment AND on the servers it covers.
    pub fn settled_assessment_mut(&mut self, id: &str, now: SystemTime) -> Option<&mut Assessment> {
        let completed = self.assessments.get_mut(id)?.settle(now);
        if completed {
            self.finish_server_collection(COLLECTION_SUCCESS);
        }
        self.assessments.get_mut(id)
    }

    /// Stop the assessment `id` names, moving the servers it was still collecting for off
    /// their collecting status as well.
    ///
    /// Answers `None` when this portfolio holds no such assessment, and otherwise whether
    /// the stop moved a RUNNING assessment to [`ASSESSMENT_STOPPED`]: an assessment that
    /// already reached a terminal status is left exactly as it was, servers included.
    /// Any completion the clock already owed the assessment is materialised first, so a
    /// stop that arrives after the assessment finished finds it terminal.
    pub fn stop_assessment(&mut self, id: &str, now: SystemTime) -> Option<bool> {
        let stopped = self.settled_assessment_mut(id, now)?.stop(now);
        if stopped {
            self.finish_server_collection(COLLECTION_STOPPED);
        }
        Some(stopped)
    }

    /// Put every server of this portfolio into [`COLLECTION_STARTED`]: an assessment just
    /// started covering them, so the collection task is running on each one.
    ///
    /// It overwrites whatever status an earlier assessment left, because the status
    /// reports the collection that is running NOW; a portfolio holding no server has
    /// nothing to move.
    pub fn begin_server_collection(&mut self) {
        for server in self.servers.values_mut() {
            server.data_collection_status = Some(COLLECTION_STARTED.to_string());
        }
    }

    /// Move every server whose collection task is still running ([`COLLECTION_RUNNING`])
    /// to `status`, the per-server face of the terminal status its assessment reached.
    ///
    /// A server that is not collecting keeps the status it has: a terminal assessment
    /// status says nothing about a collection that was never running.
    pub fn finish_server_collection(&mut self, status: &str) {
        for server in self.servers.values_mut() {
            if server
                .data_collection_status
                .as_deref()
                .is_some_and(|held| COLLECTION_RUNNING.contains(&held))
            {
                server.data_collection_status = Some(status.to_string());
            }
        }
    }

    /// The recommendation report task `id` names, with any terminal status the clock owes
    /// it already materialised.
    pub fn settled_report_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut RecommendationReport> {
        let report = self.recommendation_reports.get_mut(id)?;
        report.settle(now);
        Some(report)
    }

    /// The id of the report generation task still running at `now`, if there is one.
    ///
    /// Every stored task gets the terminal status the clock already owed it first, so a
    /// task whose span has passed is not reported as running. At most one task is ever in
    /// progress, because a start call made while this answers `Some` coalesces onto that
    /// task instead of minting another.
    pub fn in_progress_report_id(&mut self, now: SystemTime) -> Option<String> {
        for report in self.recommendation_reports.values_mut() {
            report.settle(now);
        }
        self.recommendation_reports
            .values()
            .find(|report| report.is_in_progress())
            .map(|report| report.id.clone())
    }

    /// How many servers of this account's portfolio a report scoped by `groups` covers.
    ///
    /// An EMPTY filter names no group, so it scopes nothing and the whole inventory is in
    /// scope; otherwise a server is in scope when it came in with one of the groups named
    /// — both halves of a `Group` agreeing, as [`ServerRecord::in_any_group`] decides it.
    ///
    /// The groups are collected into a set first, so the count costs one pass over the
    /// inventory and one over the filter rather than the product of the two.
    pub fn servers_in_groups(&self, groups: &[Group]) -> i32 {
        if groups.is_empty() {
            return i32::try_from(self.servers.len()).unwrap_or(i32::MAX);
        }
        let wanted: BTreeSet<(Option<&str>, Option<&str>)> = groups
            .iter()
            .map(|group| (group.name.as_deref(), group.value.as_deref()))
            .collect();
        let covered = self
            .servers
            .values()
            .filter(|server| {
                server
                    .group_id
                    .iter()
                    .any(|held| wanted.contains(&(held.name.as_deref(), held.value.as_deref())))
            })
            .count();
        i32::try_from(covered).unwrap_or(i32::MAX)
    }

    /// The task of this account that has not reached a terminal status at `now`, if there
    /// is one.
    ///
    /// Every stored record gets the terminal status the clock already owed it first
    /// ([`Self::settle_all`]) — owed at this same instant even if nobody had called — so a
    /// task whose span has passed is not reported as ongoing. The three kinds of task this
    /// service runs are looked at in the order they appear in the import-then-assess loop,
    /// and the first one still running is the answer: what a caller has to wait for is that
    /// there is one, not which of them it is.
    pub fn ongoing_task(&mut self, now: SystemTime) -> Option<OngoingTask> {
        self.settle_all(now);
        let importing = self
            .import_tasks
            .values()
            .find(|task| !task.is_complete())
            .map(|task| OngoingTask {
                kind: IMPORT_FILE_TASK,
                id: task.id.clone(),
                status: task.status.clone(),
            });
        let assessing = || {
            self.assessments
                .values()
                .find(|assessment| !assessment.is_terminal())
                .map(|assessment| OngoingTask {
                    kind: ASSESSMENT,
                    id: assessment.id.clone(),
                    status: assessment.status.clone(),
                })
        };
        let reporting = || {
            self.recommendation_reports
                .values()
                .find(|report| report.is_in_progress())
                .map(|report| OngoingTask {
                    kind: RECOMMENDATION_REPORT_GENERATION_TASK,
                    id: report.id.clone(),
                    status: report.status.clone(),
                })
        };
        importing.or_else(assessing).or_else(reporting)
    }

    /// Materialise every completion the clock owes, for a caller about to read across
    /// all of the records — including the per-server face of an assessment the clock just
    /// completed.
    pub fn settle_all(&mut self, now: SystemTime) {
        for task in self.import_tasks.values_mut() {
            task.settle(now);
        }
        let mut completed = false;
        for assessment in self.assessments.values_mut() {
            completed |= assessment.settle(now);
        }
        if completed {
            self.finish_server_collection(COLLECTION_SUCCESS);
        }
        for report in self.recommendation_reports.values_mut() {
            report.settle(now);
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
                let resume_after = decode_page_token(IMPORT_TASK_PAGE_MARKER, token)
                    .ok_or_else(page_token_not_accepted)?;
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
            (Some(_), Some(key)) => Some(encode_page_token(IMPORT_TASK_PAGE_MARKER, &key)),
            _ => None,
        };

        Ok(ImportTaskPage {
            task_infos,
            next_token,
        })
    }

    /// One page of this Region's collectors, continuing after the collector a
    /// `nextToken` names.
    ///
    /// The order is the store's own total order over the collector ids, and a token is
    /// emitted only when another collector actually follows the page, so a caller
    /// following tokens never makes a call that returns nothing. Nothing here writes: the
    /// page is taken off the stored records, so two identical calls answer the same thing.
    ///
    /// Refused with the service's `ValidationException` when `max_results` is outside the
    /// documented 1..=[`MAX_LIST_PAGE_SIZE`], or when the token is not one this list
    /// issued for a collector the store still holds.
    pub fn page_of_collectors(
        &self,
        next_token: Option<&str>,
        max_results: Option<i32>,
    ) -> Result<CollectorPage, ValidationException> {
        let limit = match max_results {
            None => MAX_LIST_PAGE_SIZE,
            Some(requested) if (1..=MAX_LIST_PAGE_SIZE).contains(&requested) => requested,
            Some(_) => return Err(invalid_request_body()),
        };
        let limit = usize::try_from(limit).unwrap_or(1);

        let after = match next_token {
            Some(token) => {
                let resume_after = decode_page_token(COLLECTOR_PAGE_MARKER, token)
                    .ok_or_else(page_token_not_accepted)?;
                if !self.collectors.contains_key(&resume_after) {
                    return Err(page_token_not_accepted());
                }
                Bound::Excluded(resume_after)
            }
            None => Bound::Unbounded,
        };

        let mut rest = self.collectors.range((after, Bound::Unbounded));
        let mut collectors = Vec::new();
        let mut last: Option<&String> = None;
        for (id, collector) in rest.by_ref().take(limit) {
            collectors.push(collector.to_collector());
            last = Some(id);
        }
        let next_token = match (rest.next(), last) {
            (Some(_), Some(key)) => Some(encode_page_token(COLLECTOR_PAGE_MARKER, key)),
            _ => None,
        };

        Ok(CollectorPage {
            collectors,
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

/// A task of this Region's portfolio that has not reached a terminal status yet, as
/// [`RegionState::ongoing_task`]
/// reports it.
///
/// Carries what a refusal caused by the task tells the caller: which task of the
/// portfolio is still running, and the non-terminal status it holds.
#[derive(Debug, Clone)]
pub struct OngoingTask {
    /// What kind of task it is, in this service's own words — one of [`IMPORT_FILE_TASK`],
    /// [`ASSESSMENT`] and [`RECOMMENDATION_REPORT_GENERATION_TASK`].
    pub kind: &'static str,
    /// The id the start call minted for it.
    pub id: String,
    /// The non-terminal status it carries.
    pub status: String,
}

/// How this service names an import file task when a refusal reports one as ongoing.
pub const IMPORT_FILE_TASK: &str = "import file task";

/// How it names an assessment.
pub const ASSESSMENT: &str = "assessment";

/// How it names a recommendation report generation task.
pub const RECOMMENDATION_REPORT_GENERATION_TASK: &str = "recommendation report generation task";

/// The `ConflictException` (HTTP 409) this service answers a write of the account-wide
/// inputs its recommendations are computed from while `ongoing` is still running.
///
/// The model documents the error as "Exception to indicate that there is an ongoing task
/// when a new task is created. Return when once the existing tasks are complete", so the
/// message reports exactly that: which task of the account is still running, the status it
/// holds, and that the write succeeds once the task completes. It carries nothing from the
/// request — the condition is one of the STORE, not of what the caller sent.
pub fn ongoing_task_conflict(ongoing: &OngoingTask) -> ConflictException {
    ConflictException {
        message: format!(
            "The {} {} is {}. Retry once the ongoing task is complete.",
            ongoing.kind, ongoing.id, ongoing.status
        ),
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
/// assessment has ever been started and `GetPortfolioPreferences` in one where no
/// preferences have ever been saved.
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
/// The prefix is the whole of the check: it is the one property of a name this check can
/// decide without a lookup, and a conforming name is taken to name a bucket the service
/// can use rather than refusing every start call and leaving the resource unreachable.
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

/// Refuse a request member whose value is not one the enumeration it targets declares.
///
/// `declared` is the generated value list of the member's target shape (the `*_VALUES`
/// constants in [`crate::ops::types`], rendered from the model's own `enum` definitions),
/// so the catalog a value is held to is the model's and never one written out by hand.
///
/// An ABSENT member is `Ok`: an optional member that was not sent carries no value that
/// could be outside the enumeration, and a `@required` one never reaches here because the
/// boundary validator refuses its absence first. The handler has to do this because the
/// boundary validator admits ANY string for an enum-targeting member — an `enum`'s value
/// list is not a constraint it enforces — so without this check an undeclared value would
/// be persisted and reported back as if the service had accepted it.
///
/// Refused with the service's one `ValidationException` (HTTP 400) sentence
/// ([`invalid_request_body`]), the same refusal every other rule the model cannot express
/// answers; each operation maps it into its own error channel.
pub fn check_enum(value: Option<&str>, declared: &[&str]) -> Result<(), ValidationException> {
    match value {
        Some(named) if !declared.contains(&named) => Err(invalid_request_body()),
        _ => Ok(()),
    }
}

/// Hold every entry of an import-group list to the `GroupName` catalog.
///
/// A `Group`'s `name` targets `GroupName`, so the same enum rule as [`check_enum`] applies
/// to it one level down, inside each element of the list: a request naming a group kind
/// the model does not declare is refused rather than stored. Its `value` is a free-form
/// string and is held only to what the model expresses about it.
pub fn check_group_names(groups: &[Group]) -> Result<(), ValidationException> {
    groups
        .iter()
        .try_for_each(|group| check_enum(group.name.as_deref(), GROUP_NAME_VALUES))
}

/// Whether a list is reported in descending order, from the `sort` the request carries.
///
/// Every `sort` member of this service targets `SortOrder` and is documented the same way
/// — "Specifies whether to sort by ascending (ASC) or descending (DESC) order" — and names
/// no key, so the direction is all a request chooses and the rule is one for every list
/// here. An ABSENT `sort` is ascending ([`SORT_ASCENDING`]); a value `SortOrder` does not
/// declare is refused with the service's [`invalid_request_body`], since the boundary
/// validator admits any string for an enum-targeting member.
pub fn sort_descending(sort: Option<&str>) -> Result<bool, ValidationException> {
    match sort {
        None | Some(SORT_ASCENDING) => Ok(false),
        Some(SORT_DESCENDING) => Ok(true),
        Some(_) => Err(invalid_request_body()),
    }
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

/// `GetRecommendationReportDetails`'s refusal of an `id` that names no report generation
/// task of this account: the not-found its own error list carries
/// (`ResourceNotFoundException`, HTTP 404, "The specified ID in the request is not
/// found."), worded with the service's [`INVALID_RESOURCE_ID`] sentence.
///
/// Like every other refusal of this service it interpolates nothing from the request —
/// the caller already holds the id it sent.
pub fn recommendation_report_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(INVALID_RESOURCE_ID.to_string()),
    }
}

/// The refusal of a `serverId` that names no server of this account's portfolio, which is
/// what `UpdateServerConfig` and `GetServerStrategies` answer for one: the not-found both
/// operations' error lists carry (`ResourceNotFoundException`, HTTP 404, "The specified ID
/// in the request is not found."), worded with the service's [`INVALID_RESOURCE_ID`]
/// sentence.
///
/// Takes no id: like every other refusal of this service it interpolates nothing from the
/// request — the caller already holds the id it sent.
pub fn server_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(INVALID_RESOURCE_ID.to_string()),
    }
}

/// The refusal of an `applicationComponentId` that names no application component of
/// this account's portfolio: the not-found this operation's error list carries
/// (`ResourceNotFoundException`, HTTP 404, "The specified ID in the request is not
/// found."), worded with the service's [`INVALID_RESOURCE_ID`] sentence — the same
/// refusal its sibling reads answer for an id of their own that resolves to nothing.
///
/// Takes no id: like every other refusal of this service it interpolates nothing from the
/// request — the caller already holds the id it sent.
pub fn application_component_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(INVALID_RESOURCE_ID.to_string()),
    }
}

/// `GetLatestAssessmentId`'s answer in an account where no assessment has ever been
/// started: `ValidationException`, HTTP 400.
///
/// The operation's own error list declares no not-found at all (`AccessDeniedException`
/// 403, `DependencyException` 500, `InternalServerException` 500, `ValidationException`
/// 400), so the client-side refusal it can answer is that 400 — reporting the condition
/// as a 404 would put a status and an error code on the wire that the operation never
/// declares, and this service already answers the neighbouring condition "the assessment
/// you asked about is not one of this account's" with the same 400 (see
/// [`assessment_id_not_accepted`]).
///
/// The status and the error code are the requirement; the message is this service's
/// sentence for a report it cannot produce because the resource it would name does not
/// exist yet ([`NOTHING_TO_REPORT`]), and the operation has no input to echo into it.
pub fn no_latest_assessment() -> ValidationException {
    ValidationException {
        message: Some(NOTHING_TO_REPORT.to_string()),
    }
}

/// `GetPortfolioPreferences`'s answer in an account where no preferences have ever been
/// saved: there is no preferences record to report, so the absent resource is reported
/// with `ResourceNotFoundException` (HTTP 404), the not-found this operation's own error
/// list carries — never a 200 whose every member is absent, which would claim the account
/// holds a record nothing wrote.
///
/// The operation takes no input, so the message carries nothing from the request: it is
/// this service's sentence for a report it cannot produce because the resource it would
/// name does not exist yet ([`NOTHING_TO_REPORT`]), the same one `GetLatestAssessmentId`
/// answers in an account with no assessment.
pub fn no_portfolio_preferences() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(NOTHING_TO_REPORT.to_string()),
    }
}

/// The key prefix every import status report object is written under, inside the bucket
/// `StartImportFileTaskRequest$s3bucketForReportData` named.
///
/// Neither AWS nor the model publishes the key layout, so the layout is the emulator's
/// choice; what it has to be is DERIVED from the task it reports on (see
/// [`import_status_report_key`]), so the `statusReportS3Key` a read reports names the one
/// object that import wrote and two imports never write over each other.
pub const IMPORT_STATUS_REPORT_PREFIX: &str = "import-status-reports";

/// The key of the status report object the import file task `task` writes.
///
/// Built from the task's minted id alone, so it is stable for the life of the task and
/// well formed for every import name the boundary validator admits.
pub fn import_status_report_key(task: &ImportFileTask) -> String {
    format!(
        "{IMPORT_STATUS_REPORT_PREFIX}/{}/status-report.json",
        task.id
    )
}

/// A stored instant as the `date-time` string a report document carries, or `None` when
/// the host clock is outside the representable range.
fn rendered_timestamp(at: SystemTime) -> Option<String> {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// The status report document the import file task `task` writes to its report bucket.
///
/// `GetImportFileTaskResponse$statusReportS3Key` documents what the object holds — "The
/// report contains details about whether each record imported successfully or why it did
/// not" — so every value here comes from the task's own record: the import it belongs to,
/// where its input file was read from, the two record counts it reports, and one entry
/// per record outcome it holds. The serialization (JSON) is the emulator's choice; the
/// content is computed from state and never canned.
pub fn import_status_report_document(task: &ImportFileTask) -> String {
    ::serde_json::json!({
        "importTaskId": task.id,
        "importName": task.import_name,
        "importFileS3Bucket": task.input_s3_bucket,
        "importFileS3Key": task.input_s3_key,
        "startTime": rendered_timestamp(task.start_time),
        "numberOfRecordsSuccess": task.number_of_records_success,
        "numberOfRecordsFailed": task.number_of_records_failed,
        // One entry per record the import brought in, each carrying that record's own
        // outcome. The task holds no record outcomes, so there is none to list — which
        // is exactly what the two counts above report.
        "records": [],
    })
    .to_string()
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The callback is the framework's seam for calling another emulated service: it routes
/// in process, so no network client is configured here and this crate names no other
/// service's crate. `region` is the request's own signing region, so the import writes
/// its report where the caller's own S3 requests go.
pub fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the bucket is addressed
        // in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for
        // a stalled stream, and each of those would need a sleep implementation this
        // client is not given. The same goes for caching the identity: the credentials
        // are handed over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Write `body` as the object `key` of `bucket`, through the in-process callback.
///
/// `Ok` only once the object is THERE, which is what lets the task record the key: a
/// later read reports `statusReportS3Key` only for an object a caller can fetch. The
/// `Err` carries the reason the object could not be written — no bucket of that name,
/// say — for the caller to report rather than to name a report nothing wrote.
pub async fn put_import_status_report(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
    body: String,
) -> Result<(), String> {
    s3_client(ctx, region)
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            body.into_bytes(),
        ))
        .content_type("application/json")
        .send()
        .await
        .map(|_| ())
        .map_err(|error| {
            ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string()
        })
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

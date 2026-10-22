// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-comprehend-comprehend-20171127 service.
//!
//! Comprehend's asynchronous analysis jobs all share ONE record shape: every
//! `Start*DetectionJob` family persists the same identity, lifecycle, data-location
//! and security members, and its `Describe*`/`List*` pair projects that record into
//! the family's own `*JobProperties` shape. [`AnalysisJob`] is that record, keyed by
//! [`JobFamily`] so each family has its own id space, and
//! [`AnalysisJob::settle`] is the ONE place a job's clock-owed status change is
//! materialised.
//!
//! ## What a job's terminal status says
//!
//! A job's terminal status is never assumed: [`AnalysisJob::terminal_outcome`] holds
//! the outcome of the run the `Start*DetectionJob` handler actually performed —
//! `COMPLETED` only when this emulator really read the input documents and wrote the
//! output archive, `FAILED` with a reason when it could not — and [`AnalysisJob::settle`]
//! reveals it once the clock owes the transition. A job whose run has not reported yet
//! stays `IN_PROGRESS`, because it is still running.
//!
//! ## Bounds
//!
//! Every collection here is bounded, so no sequence of requests can grow this store
//! without limit:
//!
//! * `analysis_jobs` — [`MAX_ANALYSIS_JOB_RECORDS`], enforced by the ONE insertion
//!   path, [`ComAmazonawsComprehendComprehend20171127State::insert_analysis_job`],
//!   with oldest-first eviction. AWS caps no account's analysis-job count, so a
//!   `Start*DetectionJob` is never refused for a bound on the emulator's own record
//!   set; only the record set is trimmed. The bound also caps the lock-held
//!   `settle` sweep `List*` performs.
//! * `idempotency_tokens` — an entry is dropped together with the job it points at,
//!   so this index cannot outgrow `analysis_jobs`.
//! * `document_classifiers` — [`MAX_DOCUMENT_CLASSIFIER_RECORDS`], enforced by the
//!   ONE insertion path,
//!   [`ComAmazonawsComprehendComprehend20171127State::insert_document_classifier`],
//!   with oldest-first eviction.
//!
//! `ops::list_sentiment_detection_jobs` and `ops::list_document_classifiers` bound
//! the RESPONSE the same way, with their `DEFAULT_MAX_RESULTS` and the modelled
//! `MaxResults` ceiling.
//!
//! ## Custom models
//!
//! [`DocumentClassifier`] is one trained (or training) custom classification model,
//! keyed by the `DocumentClassifierArn` that names it everywhere else in the API
//! (Describe/Delete/StopTraining and the resource-policy operations), and
//! [`DocumentClassifier::document_classifier_properties`] projects it into the
//! modelled `DocumentClassifierProperties` shape that `ListDocumentClassifiers` and
//! `DescribeDocumentClassifier` report.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    ClassifierMetadata, DocumentClassifierInputDataConfig, DocumentClassifierOutputDataConfig,
    DocumentClassifierProperties, DominantLanguageDetectionJobProperties, InputDataConfig,
    KeyPhrasesDetectionJobProperties, OutputDataConfig, SentimentDetectionJobProperties, Tag,
    VpcConfig,
};

/// How long a freshly submitted job stays `SUBMITTED` before the clock owes it
/// `IN_PROGRESS`.
///
/// The emulator's own span, not a documented AWS duration: a real Comprehend job
/// takes minutes, which no test can wait for. It is deliberately non-zero, so a
/// caller that reads a job straight back still observes `SUBMITTED`.
pub const QUEUE_SPAN: Duration = Duration::from_millis(300);

/// How long a job stays `IN_PROGRESS` before the clock owes it its terminal
/// status. The emulator's own span, chosen for the same reason as [`QUEUE_SPAN`].
pub const PROCESSING_SPAN: Duration = Duration::from_millis(300);

/// How long a job stays `STOP_REQUESTED` before the clock owes it `STOPPED`.
/// The emulator's own span, chosen for the same reason as [`QUEUE_SPAN`]. It is
/// deliberately non-zero, so a caller that stops a job and reads it straight back
/// still observes the intermediate `STOP_REQUESTED` state the stop reported.
pub const STOPPING_SPAN: Duration = Duration::from_millis(300);

/// The most tags a Comprehend resource can carry.
///
/// From the modelled documentation of the `TooManyTagsException` every
/// `Start*DetectionJob` declares: "The request contains more tags than can be
/// associated with a resource (50 tags per resource)."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The refusal message for a submission carrying more tags than a resource can
/// hold, or `None` when `tags` is within [`MAX_TAGS_PER_RESOURCE`].
///
/// The bound and its wording live here rather than in one operation, so every
/// family's `Start*DetectionJob` refuses the same count the same way; each handler
/// wraps the message in the `TooManyTagsException` its own error channel names.
pub fn too_many_tags_message(tags: &[Tag]) -> Option<String> {
    (tags.len() > MAX_TAGS_PER_RESOURCE).then(|| {
        format!(
            "The request contains {} tags, more than the {MAX_TAGS_PER_RESOURCE} tags that \
             can be associated with a resource.",
            tags.len()
        )
    })
}

/// Why a job whose batch run this emulator does not perform ends in the modelled
/// `FAILED` state, naming the analysis that was not carried out and both S3
/// locations the submission pointed at.
///
/// Running one of these jobs means reading the documents from the Amazon S3 location
/// `InputDataConfig` names, analysing each one and writing the analysed output back
/// to `OutputDataConfig`. The family whose run this emulator DOES perform —
/// dominant-language detection — records what its run actually did; every other
/// family's job ends with this reason instead of reporting a `COMPLETED` run that
/// nothing performed. `analysis` names the family's work ("Sentiment analysis",
/// "Key-phrase extraction", …).
pub fn unperformed_run_message(
    analysis: &str,
    input: &InputDataConfig,
    output: &OutputDataConfig,
) -> String {
    format!(
        "{analysis} of the documents at {} was not performed by this emulator, so no \
         document was analyzed and no output was written to {}.",
        input.s3_uri, output.s3_uri,
    )
}

/// The statuses an analysis job can never leave, out of
/// `ops::types::JOB_STATUS_VALUES`.
///
/// A status is terminal when no further transition can take a job out of it:
/// `COMPLETED` and `FAILED` end a run, and `STOPPED` ends a cancellation. The
/// remaining three (`SUBMITTED`, `IN_PROGRESS`, `STOP_REQUESTED`) all still owe
/// the clock a transition, so each of them is still stoppable.
///
/// `COMPLETED` is listed because the modelled vocabulary has it and this record
/// serves every job family, not because a job this emulator persists can reach
/// it: [`AnalysisJob::settle`] ends a started job in `FAILED`, so `COMPLETED` is
/// reserved for a family whose work this emulator can genuinely perform.
pub const TERMINAL_JOB_STATUSES: &[&str] = &["COMPLETED", "FAILED", "STOPPED"];

/// The message a `JobNotFoundException` carries, whichever job operation raised
/// it and whatever `JobId` was asked for.
///
/// Amazon Comprehend answers a well-formed `JobId` that names no job with ONE
/// fixed sentence: the refusal states that the service holds no such job, and
/// nothing the caller sent is echoed back into it.
pub const JOB_NOT_FOUND_MESSAGE: &str = "Could not find specified job.";

/// The message the `InvalidRequestException` carries when a stop is refused
/// because the job has already reached one of [`TERMINAL_JOB_STATUSES`]. Fixed
/// for the same reason as [`JOB_NOT_FOUND_MESSAGE`]: it states the job's class of
/// state, not the request.
pub const JOB_ALREADY_TERMINAL_MESSAGE: &str = "Job is already in terminal state.";

/// The most analysis-job records this emulator retains, across every family.
///
/// AWS publishes neither a retention nor an account limit for analysis jobs, so
/// this is the EMULATOR's bound, and it exists so a caller cannot grow the store
/// without limit: the oldest record is evicted (with its idempotency-index entry)
/// once the store passes it. It is far above any test or client working set, so
/// eviction never hides a job a caller is still polling.
pub const MAX_ANALYSIS_JOB_RECORDS: usize = 1000;

/// The most custom document-classifier records this emulator retains.
///
/// The EMULATOR's bound, for the same reason as [`MAX_ANALYSIS_JOB_RECORDS`]: the
/// oldest record is evicted once the store passes it, so no sequence of requests
/// can grow this store without limit. AWS's own per-account model quota is
/// adjustable and is not emulated as a refusal here, so creating a classifier is
/// never refused for this bound; only the record set is trimmed. The bound also
/// caps the page `ListDocumentClassifiers` can ever walk.
pub const MAX_DOCUMENT_CLASSIFIER_RECORDS: usize = 1000;

/// Which `Start*DetectionJob` family a persisted [`AnalysisJob`] belongs to.
///
/// The variant also carries the ARN resource type the family's `JobArn` uses, so
/// the ARN form is stated once. Sibling families (entities, topics, …) are added as
/// their own issues implement them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JobFamily {
    /// `StartSentimentDetectionJob` and its Describe/List/Stop operations.
    SentimentDetection,
    /// `StartDominantLanguageDetectionJob` and its Describe operation.
    DominantLanguageDetection,
    /// `StartKeyPhrasesDetectionJob` and its Describe/List/Stop operations.
    KeyPhrasesDetection,
}

impl JobFamily {
    /// The resource type segment this family's `JobArn` carries, e.g.
    /// `arn:aws:comprehend:<region>:<account>:sentiment-detection-job/<JobId>`.
    pub fn arn_resource_type(self) -> &'static str {
        match self {
            Self::SentimentDetection => "sentiment-detection-job",
            Self::DominantLanguageDetection => "dominant-language-detection-job",
            Self::KeyPhrasesDetection => "key-phrases-detection-job",
        }
    }

    /// The analysis type this family's output folder is named for.
    ///
    /// `OutputDataConfig$S3Uri` documents that the submitted URI "is used as the
    /// prefix for the actual location of the output file" and that "the service
    /// creates an output file in a directory specific to the job"; AWS names that
    /// directory `<account-id>-<ANALYSIS_TYPE>-<JobId>`, which is what
    /// [`output_archive_key`] builds from this token.
    pub fn output_analysis_type(self) -> &'static str {
        match self {
            Self::SentimentDetection => "SENTIMENT",
            Self::DominantLanguageDetection => "LANGUAGE",
            Self::KeyPhrasesDetection => "KEY_PHRASES",
        }
    }
}

/// The outcome of the run a `Start*DetectionJob` handler performed for a job: the
/// terminal status the clock reveals once the job's processing span is over, the
/// `Message` that says why, and where the output it wrote went.
///
/// Held separately from [`AnalysisJob::job_status`] because a status is only reported
/// for work that ran: until the run reports, the job is still processing.
#[derive(Debug, Clone)]
pub struct JobOutcome {
    /// One of `super::ops::types::JOB_STATUS_VALUES`, and terminal:
    /// `COMPLETED` or `FAILED`.
    pub status: String,
    /// What the run has to say — the failure's reason, or nothing for a run that
    /// completed.
    pub message: Option<String>,
    /// The output archive's own location, for a run that wrote one. Revealed with
    /// the terminal status by [`AnalysisJob::settle`], never before it: the
    /// configuration a job reports has to match the status it reports, and
    /// `OutputDataConfig$S3Uri` names the archive only once the job is finished.
    pub output_s3_uri: Option<String>,
}

impl JobOutcome {
    /// The outcome of a run that read every input document and wrote its output to
    /// `output_s3_uri`.
    pub fn completed(output_s3_uri: String) -> Self {
        Self {
            status: "COMPLETED".to_string(),
            message: None,
            output_s3_uri: Some(output_s3_uri),
        }
    }

    /// The outcome of a run that could not be carried out, naming `reason`. Nothing
    /// was written, so the job keeps reporting the output configuration it was
    /// submitted with.
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            status: "FAILED".to_string(),
            message: Some(reason.into()),
            output_s3_uri: None,
        }
    }
}

/// Everything a `Start*DetectionJob` request asks for, borrowed from its input.
///
/// Carried as one value so [`Self::digest`] — the thing that decides whether a
/// repeated `ClientRequestToken` names the submission it already created — is
/// written once for every family rather than once per handler.
#[derive(Debug)]
pub struct JobSubmission<'a> {
    pub family: JobFamily,
    pub job_name: Option<&'a str>,
    pub input_data_config: &'a InputDataConfig,
    pub output_data_config: &'a OutputDataConfig,
    /// The family's `LanguageCode`, for a family that takes one;
    /// `StartDominantLanguageDetectionJob` models none, because detecting the
    /// language is the job.
    pub language_code: Option<&'a str>,
    pub data_access_role_arn: &'a str,
    pub volume_kms_key_id: Option<&'a str>,
    pub vpc_config: Option<&'a VpcConfig>,
    pub tags: &'a [Tag],
}

impl JobSubmission<'_> {
    /// The digest of this submission: the second half of the idempotency key, so a
    /// `ClientRequestToken` re-presented with the SAME request names the job it
    /// already created, and the same token carrying a different request is a
    /// different submission (which mints its own job rather than being refused —
    /// the model declares no error for it).
    ///
    /// The family takes part, so one token presented to two families is two
    /// submissions. Every part is length-prefixed and an optional member renders
    /// its presence, so no two different requests can render alike by splitting
    /// their values differently.
    ///
    /// Each structural member is rendered from ITS OWN members, named one by one —
    /// never through a `Debug` rendering, which is no serialization contract: a
    /// derive or model change would silently move every digest and turn a retry into
    /// a second job.
    pub fn digest(&self) -> String {
        let mut tags: Vec<String> = self
            .tags
            .iter()
            .map(|tag| format!("{}={}", tag.key, rendered(tag.value.as_deref())))
            .collect();
        // A resource's tags are a set, so the same tags in another order are the
        // same request.
        tags.sort();
        let input_data_config = [
            self.input_data_config.s3_uri.clone(),
            rendered(self.input_data_config.input_format.as_deref()),
            self.input_data_config
                .document_reader_config
                .as_ref()
                .map_or_else(
                    || "absent".to_string(),
                    |reader| {
                        format!(
                            "{}|{}|{}",
                            reader.document_read_action,
                            rendered(reader.document_read_mode.as_deref()),
                            reader
                                .feature_types
                                .as_ref()
                                .map_or_else(|| "absent".to_string(), |types| types.join("+")),
                        )
                    },
                ),
        ]
        .join("|");
        let output_data_config = [
            self.output_data_config.s3_uri.clone(),
            rendered(self.output_data_config.kms_key_id.as_deref()),
        ]
        .join("|");
        let vpc_config = self.vpc_config.map_or_else(
            || "absent".to_string(),
            |vpc| {
                format!(
                    "{}|{}",
                    vpc.security_group_ids.join("+"),
                    vpc.subnets.join("+"),
                )
            },
        );
        let parts: Vec<String> = vec![
            self.family.arn_resource_type().to_string(),
            rendered(self.job_name),
            input_data_config,
            output_data_config,
            rendered(self.language_code),
            self.data_access_role_arn.to_string(),
            rendered(self.volume_kms_key_id),
            vpc_config,
            tags.join(","),
        ];
        let mut hasher = Sha256::new();
        for part in &parts {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        hex::encode(hasher.finalize())
    }
}

/// An optional value rendered so a present empty string cannot render like an absent
/// member.
fn rendered(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("present:{value}"),
        None => "absent".to_string(),
    }
}

/// What a `Start*DetectionJob` submission is indexed under, so a retried request
/// resolves to the job it already created.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubmissionKey {
    pub family: JobFamily,
    /// The request's `ClientRequestToken`; a request carrying none is never indexed.
    pub client_request_token: String,
    /// [`JobSubmission::digest`] of the request the token was used with.
    pub request_digest: String,
}

/// The S3 object key an analysis job's output archive is written to under the
/// submitted output prefix.
///
/// `OutputDataConfig$S3Uri` documents the layout: the submitted URI "is used as the
/// prefix for the actual location of the output file", the service "creates an output
/// file in a directory specific to the job", and that file is "called
/// `output.tar.gz`". The per-job directory is `<account-id>-<ANALYSIS_TYPE>-<JobId>`.
pub fn output_archive_key(prefix: &str, family: JobFamily, job_id: &str) -> String {
    let directory = format!(
        "{}-{}-{job_id}/output/output.tar.gz",
        ::engine::EMULATOR_ACCOUNT_ID,
        family.output_analysis_type(),
    );
    match prefix.trim_end_matches('/') {
        "" => directory,
        prefix => format!("{prefix}/{directory}"),
    }
}

/// One persisted asynchronous analysis job.
///
/// Every member is the value `Start*DetectionJob` was called with, or a value this
/// service minted for the job — never anything a later `Describe`/`List` request
/// supplied.
#[derive(Debug, Clone)]
pub struct AnalysisJob {
    /// Submission order within the service, the stable key `List*` pages on.
    pub seq: u64,
    pub job_id: String,
    pub job_arn: String,
    pub job_name: Option<String>,
    /// One of `super::ops::types::JOB_STATUS_VALUES`.
    pub job_status: String,
    pub message: Option<String>,
    pub submit_time: SystemTime,
    pub end_time: Option<SystemTime>,
    /// When the clock owes `SUBMITTED` -> `IN_PROGRESS`.
    pub in_progress_due_at: SystemTime,
    /// When the clock owes `IN_PROGRESS` -> the status [`Self::terminal_outcome`]
    /// reports.
    pub terminal_due_at: SystemTime,
    /// When the clock owes `STOP_REQUESTED` -> `STOPPED`; `None` until a stop is
    /// requested.
    pub stopped_due_at: Option<SystemTime>,
    /// What the run this job's `Start*DetectionJob` performed reported, or `None`
    /// while it is still running.
    pub terminal_outcome: Option<JobOutcome>,
    pub input_data_config: InputDataConfig,
    /// Where the output goes: the SUBMITTED configuration, until the clock reveals a
    /// terminal status whose run wrote an archive, at which point `S3Uri` becomes
    /// that archive's own location — what `OutputDataConfig$S3Uri` documents a
    /// FINISHED job reports ("the location is used as the prefix for the actual
    /// location of the output file"). A job still being processed therefore reports
    /// the prefix it was given, which is the location its status matches.
    pub output_data_config: OutputDataConfig,
    /// One of `super::ops::types::LANGUAGE_CODE_VALUES`, for a family whose
    /// submission names one.
    pub language_code: Option<String>,
    pub data_access_role_arn: String,
    pub volume_kms_key_id: Option<String>,
    pub vpc_config: Option<VpcConfig>,
    pub tags: Vec<Tag>,
    /// The `ClientRequestToken` this job was submitted under, if any. Held so the
    /// idempotency-index entry can be dropped with the record it points at.
    pub client_request_token: Option<String>,
    /// [`JobSubmission::digest`] of the request that created this job — the other
    /// half of its idempotency-index key. Held for the same reason as
    /// `client_request_token`: together they identify the submission, so a retried
    /// request resolves to the job it already created while the same token carrying
    /// a different request gets a job of its own.
    pub request_digest: String,
}

impl AnalysisJob {
    /// Materialise every status change the clock already owed at `now`.
    ///
    /// Called by the reporting operations as well as the mutating ones: the
    /// transition was due at `now` whether or not anybody called, so applying it on
    /// read reports the status the job already had rather than advancing it.
    ///
    /// The terminal transition is only owed once the job's run has reported an
    /// outcome ([`Self::terminal_outcome`]): a job whose run has not finished is
    /// still `IN_PROGRESS`, and no status is ever reported for work nothing
    /// performed. Revealing that status is also what moves `OutputDataConfig$S3Uri`
    /// onto the archive the run wrote, so the location a job reports always matches
    /// the status it reports.
    pub fn settle(&mut self, now: SystemTime) {
        if self.job_status == "SUBMITTED" && now >= self.in_progress_due_at {
            self.job_status = "IN_PROGRESS".to_string();
        }
        if self.job_status == "IN_PROGRESS" && now >= self.terminal_due_at {
            if let Some(outcome) = self.terminal_outcome.clone() {
                self.job_status = outcome.status;
                self.message = outcome.message;
                self.end_time = Some(self.terminal_due_at);
                if let Some(output_s3_uri) = outcome.output_s3_uri {
                    self.output_data_config.s3_uri = output_s3_uri;
                }
            }
        }
        if self.job_status == "STOP_REQUESTED"
            && self.stopped_due_at.is_some_and(|due| now >= due)
        {
            self.job_status = "STOPPED".to_string();
            self.end_time = self.stopped_due_at;
        }
    }

    /// Record that a caller asked for this job to stop, naming the reason in
    /// `Message`.
    ///
    /// Cancelling an analysis job is a TWO-PHASE transition for every job that is
    /// not yet terminal, queued (`SUBMITTED`) as much as running
    /// (`IN_PROGRESS`): the job is marked for termination — `STOP_REQUESTED`, the
    /// status the stop call reports — and the clock owes it `STOPPED`
    /// [`STOPPING_SPAN`] later, which [`Self::settle`] materialises. The job is
    /// never put straight into `STOPPED`, so the intermediate state a client waits
    /// for is always observable.
    ///
    /// A job already in one of [`TERMINAL_JOB_STATUSES`] never reaches here: the
    /// stop operation refuses it with a 400 first ([`Self::is_terminal`]). One
    /// already in `STOP_REQUESTED` keeps the due instant its first stop set — a
    /// repeated stop neither postpones the settlement nor rewrites the `Message`.
    pub fn request_stop(&mut self, now: SystemTime) {
        match self.job_status.as_str() {
            "SUBMITTED" | "IN_PROGRESS" => {
                self.job_status = "STOP_REQUESTED".to_string();
                self.stopped_due_at = Some(now + STOPPING_SPAN);
            }
            // Already stopping, or terminal: nothing more to mark.
            _ => return,
        }
        self.message = Some("The job was stopped at the request of the caller.".to_string());
    }

    /// Whether this job has reached a status it can never leave, i.e. one of
    /// [`TERMINAL_JOB_STATUSES`].
    ///
    /// `STOPPED` is terminal: a cancelled job never re-enters the lifecycle, so a
    /// further stop against it has nothing to mark and is refused with the same
    /// 400 `InvalidRequestException` a finished job's stop gets. (The operation's
    /// own prose names only `COMPLETED` and `FAILED` — and names the
    /// dominant-language operation while doing it — but recorded behaviour refuses
    /// a stop on a `STOPPED` job too, which is what "terminal" means.)
    pub fn is_terminal(&self) -> bool {
        TERMINAL_JOB_STATUSES.contains(&self.job_status.as_str())
    }

    /// The idempotency-index key this job was submitted under, for a job submitted
    /// with a `ClientRequestToken`.
    pub fn submission_key(&self, family: JobFamily) -> Option<SubmissionKey> {
        self.client_request_token
            .clone()
            .map(|client_request_token| SubmissionKey {
                family,
                client_request_token,
                request_digest: self.request_digest.clone(),
            })
    }
}

/// Write the record -> `*JobProperties` projection ONCE, per family shape.
///
/// Comprehend models a separate properties structure per job family, and the
/// families whose shape declares the same members share this one body rather than a
/// copy of it: a member added to [`AnalysisJob`] is reported by every one of them or
/// by none. A family whose shape differs — dominant-language, which models no
/// `LanguageCode` because detecting it is the job — has its own projection below.
macro_rules! job_properties_projections {
    ($($method:ident -> $shape:ident,)+) => {
        impl AnalysisJob {
            $(
                #[doc = concat!(
                    "Project this record into the modelled `",
                    stringify!($shape),
                    "` shape.",
                )]
                pub fn $method(&self) -> $shape {
                    $shape {
                        job_id: Some(self.job_id.clone()),
                        job_arn: Some(self.job_arn.clone()),
                        job_name: self.job_name.clone(),
                        job_status: Some(self.job_status.clone()),
                        message: self.message.clone(),
                        submit_time: Some(::aws_smithy_types::DateTime::from(self.submit_time)),
                        end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
                        input_data_config: Some(self.input_data_config.clone()),
                        output_data_config: Some(self.output_data_config.clone()),
                        language_code: self.language_code.clone(),
                        data_access_role_arn: Some(self.data_access_role_arn.clone()),
                        volume_kms_key_id: self.volume_kms_key_id.clone(),
                        vpc_config: self.vpc_config.clone(),
                    }
                }
            )+
        }
    };
}

job_properties_projections! {
    sentiment_detection_job_properties -> SentimentDetectionJobProperties,
    key_phrases_detection_job_properties -> KeyPhrasesDetectionJobProperties,
}

impl AnalysisJob {
    /// Project this record into the dominant-language family's modelled properties
    /// shape, which models no `LanguageCode`: the language is what the job detects.
    /// Every other member is the one the macro above projects.
    pub fn dominant_language_detection_job_properties(
        &self,
    ) -> DominantLanguageDetectionJobProperties {
        DominantLanguageDetectionJobProperties {
            job_id: Some(self.job_id.clone()),
            job_arn: Some(self.job_arn.clone()),
            job_name: self.job_name.clone(),
            job_status: Some(self.job_status.clone()),
            message: self.message.clone(),
            submit_time: Some(::aws_smithy_types::DateTime::from(self.submit_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
            input_data_config: Some(self.input_data_config.clone()),
            output_data_config: Some(self.output_data_config.clone()),
            data_access_role_arn: Some(self.data_access_role_arn.clone()),
            volume_kms_key_id: self.volume_kms_key_id.clone(),
            vpc_config: self.vpc_config.clone(),
        }
    }
}

/// One persisted custom document classifier.
///
/// Every member is a value the operation that created the classifier was called
/// with, or a value this service minted for it — never anything a later
/// `Describe`/`List` request supplied. The record is keyed by its
/// `DocumentClassifierArn`, which is what every other classifier operation names it
/// by, and [`Self::document_classifier_properties`] is the ONE place it is projected
/// into the modelled shape the reporting operations return.
#[derive(Debug, Clone)]
pub struct DocumentClassifier {
    /// Creation order within the service, the stable key
    /// `ListDocumentClassifiers` pages on.
    pub seq: u64,
    /// The classifier's ARN, as [`document_classifier_arn`] renders it.
    pub classifier_arn: String,
    /// `DocumentClassifierName`, the name `DocumentClassifierFilter$DocumentClassifierName`
    /// selects on. Not a member of `DocumentClassifierProperties`: the ARN carries it.
    pub classifier_name: String,
    /// `VersionName`, when the classifier was created as a named version.
    pub version_name: Option<String>,
    /// One of `super::ops::types::LANGUAGE_CODE_VALUES`.
    pub language_code: String,
    /// One of `super::ops::types::MODEL_STATUS_VALUES`.
    pub status: String,
    pub message: Option<String>,
    pub submit_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub training_start_time: Option<SystemTime>,
    pub training_end_time: Option<SystemTime>,
    pub input_data_config: Option<DocumentClassifierInputDataConfig>,
    pub output_data_config: Option<DocumentClassifierOutputDataConfig>,
    /// The metrics of a finished training run; `None` while nothing has been
    /// trained, which is what a model that never reached `TRAINED` reports.
    pub classifier_metadata: Option<ClassifierMetadata>,
    pub data_access_role_arn: Option<String>,
    pub volume_kms_key_id: Option<String>,
    pub model_kms_key_id: Option<String>,
    pub vpc_config: Option<VpcConfig>,
    /// One of `super::ops::types::DOCUMENT_CLASSIFIER_MODE_VALUES`.
    pub mode: String,
    pub source_model_arn: Option<String>,
    pub flywheel_arn: Option<String>,
    pub tags: Vec<Tag>,
    /// The `ClientRequestToken` the classifier was created under, if any.
    pub client_request_token: Option<String>,
}

impl DocumentClassifier {
    /// A classifier record as it stands the instant it was submitted: the identity
    /// and the configuration it was created with, nothing trained yet.
    ///
    /// `MULTI_CLASS` is the mode `CreateDocumentClassifier` documents as its
    /// default ("the default is `MULTI_CLASS`" on
    /// `CreateDocumentClassifierRequest$Mode`); a caller that named a mode
    /// overwrites the field.
    pub fn submitted(
        seq: u64,
        classifier_arn: String,
        classifier_name: String,
        language_code: String,
        submit_time: SystemTime,
    ) -> Self {
        Self {
            seq,
            classifier_arn,
            classifier_name,
            version_name: None,
            language_code,
            status: "SUBMITTED".to_string(),
            message: None,
            submit_time,
            end_time: None,
            training_start_time: None,
            training_end_time: None,
            input_data_config: None,
            output_data_config: None,
            classifier_metadata: None,
            data_access_role_arn: None,
            volume_kms_key_id: None,
            model_kms_key_id: None,
            vpc_config: None,
            mode: "MULTI_CLASS".to_string(),
            source_model_arn: None,
            flywheel_arn: None,
            tags: Vec::new(),
            client_request_token: None,
        }
    }

    /// Project this record into the modelled `DocumentClassifierProperties` shape.
    pub fn document_classifier_properties(&self) -> DocumentClassifierProperties {
        DocumentClassifierProperties {
            document_classifier_arn: Some(self.classifier_arn.clone()),
            language_code: Some(self.language_code.clone()),
            status: Some(self.status.clone()),
            message: self.message.clone(),
            submit_time: Some(::aws_smithy_types::DateTime::from(self.submit_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
            training_start_time: self
                .training_start_time
                .map(::aws_smithy_types::DateTime::from),
            training_end_time: self
                .training_end_time
                .map(::aws_smithy_types::DateTime::from),
            input_data_config: self.input_data_config.clone(),
            output_data_config: self.output_data_config.clone(),
            classifier_metadata: self.classifier_metadata.clone(),
            data_access_role_arn: self.data_access_role_arn.clone(),
            volume_kms_key_id: self.volume_kms_key_id.clone(),
            vpc_config: self.vpc_config.clone(),
            mode: Some(self.mode.clone()),
            model_kms_key_id: self.model_kms_key_id.clone(),
            version_name: self.version_name.clone(),
            source_model_arn: self.source_model_arn.clone(),
            flywheel_arn: self.flywheel_arn.clone(),
        }
    }
}

/// The ARN a document classifier carries, stated once here.
///
/// The form the modelled `DocumentClassifierArn` pattern requires:
/// `arn:aws:comprehend:<region>:<account>:document-classifier/<name>` for an
/// unversioned classifier, with `/version/<version-name>` appended when the
/// classifier was created as a named version.
pub fn document_classifier_arn(region: &str, name: &str, version_name: Option<&str>) -> String {
    let resource = match version_name {
        Some(version) => format!("document-classifier/{name}/version/{version}"),
        None => format!("document-classifier/{name}"),
    };
    ::emulator_server::emulator_arn("comprehend", region, &resource)
}

/// Per-service state for com-amazonaws-comprehend-comprehend-20171127. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsComprehendComprehend20171127State {
    /// Every persisted asynchronous analysis job, keyed by its family and `JobId`.
    /// Bounded by [`MAX_ANALYSIS_JOB_RECORDS`]: insert through
    /// [`Self::insert_analysis_job`], never directly.
    pub analysis_jobs: BTreeMap<(JobFamily, String), AnalysisJob>,
    /// The `JobId` each `(family, ClientRequestToken, request digest)` submission
    /// minted, so a retried `Start*DetectionJob` — the same token with the same
    /// request — resolves to the job it already created, while the same token with a
    /// different request is a submission of its own. An entry is dropped with the
    /// job it points at.
    pub idempotency_tokens: HashMap<SubmissionKey, String>,
    /// Monotonic submission counter: the source of both a job's `seq` and the
    /// uniqueness of a minted `JobId`.
    pub job_id_seq: u64,
    /// Every persisted custom document classifier, keyed by its
    /// `DocumentClassifierArn`. Bounded by [`MAX_DOCUMENT_CLASSIFIER_RECORDS`]:
    /// insert through [`Self::insert_document_classifier`], never directly.
    pub document_classifiers: BTreeMap<String, DocumentClassifier>,
    /// Monotonic creation counter, the source of a classifier's `seq` — the stable
    /// order `ListDocumentClassifiers` pages on.
    pub document_classifier_seq: u64,
}

impl ComAmazonawsComprehendComprehend20171127State {
    /// Mint the next submission sequence number and a `JobId` for it.
    ///
    /// The id is the first 16 bytes of a digest over the sequence number and the
    /// submission instant, rendered as 32 lowercase hex characters — the form and
    /// the modelled 1..32 length an AWS `JobId` has, and unique per submission
    /// because the sequence number is.
    pub fn mint_job_id(&mut self, submitted_at: SystemTime) -> (u64, String) {
        self.job_id_seq += 1;
        let nanos = submitted_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut hasher = Sha256::new();
        hasher.update(self.job_id_seq.to_be_bytes());
        hasher.update(nanos.to_be_bytes());
        let digest = hasher.finalize();
        (self.job_id_seq, hex::encode(&digest[..16]))
    }

    /// The `JobId` an earlier submission under exactly this key minted, if the job
    /// it created is still held.
    pub fn replayed_job_id(&self, key: &SubmissionKey) -> Option<String> {
        let job_id = self.idempotency_tokens.get(key)?;
        self.analysis_jobs
            .contains_key(&(key.family, job_id.clone()))
            .then(|| job_id.clone())
    }

    /// Persist a submitted job, index it under its submission key — its
    /// `ClientRequestToken` and request digest — and evict the oldest records once
    /// the store passes [`MAX_ANALYSIS_JOB_RECORDS`].
    ///
    /// The ONE insertion path for [`Self::analysis_jobs`], so the bound cannot be
    /// bypassed by a handler. Submission never fails for the bound: AWS refuses no
    /// `Start*DetectionJob` for a record count, so only this emulator's record set
    /// is trimmed.
    pub fn insert_analysis_job(&mut self, family: JobFamily, job: AnalysisJob) {
        let job_id = job.job_id.clone();
        if let Some(key) = job.submission_key(family) {
            self.idempotency_tokens.insert(key, job_id.clone());
        }
        self.analysis_jobs.insert((family, job_id), job);
        while self.analysis_jobs.len() > MAX_ANALYSIS_JOB_RECORDS {
            let oldest = self
                .analysis_jobs
                .iter()
                .min_by(|((left_family, left_id), left), ((right_family, right_id), right)| {
                    (left.seq, left_family, left_id.as_str()).cmp(&(
                        right.seq,
                        right_family,
                        right_id.as_str(),
                    ))
                })
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            let evicted = self.analysis_jobs.remove(&oldest);
            // Drop the index entry only while it still points at the evicted
            // record: a later submission may have taken the same key over.
            if let Some(key) = evicted.and_then(|job| job.submission_key(oldest.0)) {
                if self
                    .idempotency_tokens
                    .get(&key)
                    .is_some_and(|id| *id == oldest.1)
                {
                    self.idempotency_tokens.remove(&key);
                }
            }
        }
    }

    /// Mint the next classifier creation sequence number.
    ///
    /// A classifier's identity is its ARN (name plus optional version), which the
    /// caller chooses, so nothing is hashed here: the counter exists only to give
    /// the records a stable creation order for paging.
    pub fn mint_document_classifier_seq(&mut self) -> u64 {
        self.document_classifier_seq += 1;
        self.document_classifier_seq
    }

    /// Persist a classifier record and evict the oldest once the store passes
    /// [`MAX_DOCUMENT_CLASSIFIER_RECORDS`].
    ///
    /// The ONE insertion path for [`Self::document_classifiers`], so the bound
    /// cannot be bypassed by a handler.
    pub fn insert_document_classifier(&mut self, classifier: DocumentClassifier) {
        self.document_classifiers
            .insert(classifier.classifier_arn.clone(), classifier);
        while self.document_classifiers.len() > MAX_DOCUMENT_CLASSIFIER_RECORDS {
            let oldest = self
                .document_classifiers
                .iter()
                .min_by(|(left_arn, left), (right_arn, right)| {
                    (left.seq, left_arn.as_str()).cmp(&(right.seq, right_arn.as_str()))
                })
                .map(|(arn, _)| arn.clone());
            let Some(oldest) = oldest else { break };
            self.document_classifiers.remove(&oldest);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsComprehendComprehend20171127State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsComprehendComprehend20171127State>)
}

/// The AWS region this request was signed for, the region every ARN this service
/// mints carries. Falls back to the emulator's default region for an unsigned
/// request, which carries no credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request is answered in; emulators default to `us-east-1`
/// (see `emulator_server::emulator_arn`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// A `SystemTime` as the epoch-relative `(seconds, subsecond nanos)` pair a modelled
/// timestamp compares against. Goes through the modelled timestamp type, so a time
/// and a filter bound are always compared on the same scale.
pub fn epoch_parts(time: SystemTime) -> (i64, u32) {
    datetime_parts(&::aws_smithy_types::DateTime::from(time))
}

/// A modelled timestamp as the same epoch-relative pair as [`epoch_parts`].
pub fn datetime_parts(timestamp: &::aws_smithy_types::DateTime) -> (i64, u32) {
    (timestamp.secs(), timestamp.subsec_nanos())
}

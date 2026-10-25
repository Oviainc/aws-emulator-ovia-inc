// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-transcribe-transcribe service.
//!
//! Several resource families live here: the transcription-job lifecycle — the stored
//! record, the clock-owed status transitions, and the projections of a record onto
//! the modelled `TranscriptionJob` / `TranscriptionJobSummary` shapes — the MEDICAL
//! transcription jobs ([`MedicalTranscriptionJobRecord`]), a family of its own with
//! the same lifecycle projected onto `MedicalTranscriptionJob` — the
//! custom-vocabulary family (vocabularies and vocabulary filters), with its own
//! records, its `PENDING` -> `READY` transition and its projections onto
//! `VocabularyInfo` / `VocabularyFilterInfo` — the custom MEDICAL vocabularies
//! ([`MedicalVocabularyRecord`]), a family of the same shape projected onto the same
//! `VocabularyInfo` — the Call Analytics categories, each a record of the rules
//! and input type it was created with and its projection onto `CategoryProperties` —
//! the custom LANGUAGE models ([`LanguageModelRecord`]), each a record of the
//! base model, language and training data a create named, with its
//! `IN_PROGRESS` -> `FAILED` transition and its projection onto the modelled
//! `LanguageModel` shape — the Call Analytics jobs ([`CallAnalyticsJobRecord`]), each
//! a record of the media, output location and settings a start named, with its own
//! `IN_PROGRESS` -> `COMPLETED` transition and its projections onto `CallAnalyticsJob`
//! and `CallAnalyticsJobSummary` — and the Medical Scribe (AWS HealthScribe) jobs, each a
//! record of the media, settings and channel definitions it was started with, with
//! its own `IN_PROGRESS` -> `COMPLETED` transition and its projections onto
//! `MedicalScribeJob` and `MedicalScribeJobSummary`.
//! Several operations read each record, so
//! the projections are single-homed here rather than copied into each handler; the
//! three tagging operations reach every family's tag set through one lookup
//! ([`ComAmazonawsTranscribeTranscribeState::tags_mut`]).
//!
//! WHAT THIS EMULATES, AND WHAT IT DOES NOT: the job RECORD — its name, its
//! settings, its status, its timestamps and the location its output would be
//! written to. No audio is transcribed and no object is ever written at the
//! reported `Transcript.TranscriptFileUri` / `SubtitlesOutput.SubtitleFileUris`,
//! so a completed job means "the control plane finished with this record", never
//! "a transcript is fetchable from that URI". Nothing invents transcript content: no
//! word, sentiment or summary of a call appears anywhere, because no engine produced
//! one. A member the service itself DETERMINES is reported where the request states
//! what it is determined from and withheld where it does not — the transcription
//! family's per-language breakdown (`LanguageCodes`) stays absent for that reason,
//! while the Call Analytics family's language, media format and sample rate are
//! determined from the request and this family's own constants (see
//! [`CallAnalyticsJobRecord`]).
//! A custom vocabulary is the same: its terms are held in this state, where a
//! `StartTranscriptionJob` that names the vocabulary would read them from, while the
//! `DownloadUri` it reports is a location nothing serves (see
//! [`vocabulary_download_uri`]) and no operation of this service reports the terms
//! themselves — none of their output shapes has a member for them. A vocabulary whose
//! terms would have to be READ from a file URI is RECORDED like any other — the
//! control plane is real, so the create succeeds and every later operation finds the
//! resource — and the build this emulator cannot run ends in the service's own
//! modelled `FAILED` state, with a `FailureReason` naming the file nothing read (see
//! [`VocabularyRecord::advance_clock`] and [`file_not_read_failure_reason`]); the
//! custom MEDICAL vocabulary family, whose terms can ONLY come from such a file, does
//! exactly the same (see [`MedicalVocabularyRecord`]). Either way this emulator reads
//! no object store and makes no outbound request, so it has nothing to build a
//! vocabulary's terms from and never reports a vocabulary built from a file as
//! `READY`. A vocabulary FILTER whose words would have come from such a file is
//! RECORDED the same way ([`VocabularyFilterRecord`]), and that is all there is to
//! report for it: the filter family models no processing state and no operation of it
//! reports a filter's words, so the only thing a filter nothing read a file for does
//! not report is its own `DownloadUri`, the file this service would have built.
//! A Call Analytics category is the same kind of record: its
//! rules are held here, where a Call Analytics job that applies the category would
//! read them from, and nothing evaluates a rule against a call, because no audio is
//! analysed. A Call Analytics JOB is the job record again
//! ([`CallAnalyticsJobRecord`]): no call is transcribed, no call characteristic,
//! sentiment or summary is derived and no category is matched, so the
//! `Transcript.TranscriptFileUri` — and, for a job that asked for redaction, the
//! `RedactedTranscriptFileUri` beside it — a completed one reports names a location
//! nothing writes to or serves. That family's start request carries no language,
//! media-format or sample-rate member at all, because the service DETERMINES each of
//! them, so a finished job of it reports the determination this emulator can make
//! without audio: the language the request narrowed identification to
//! ([`identified_language`]), the format the media URI it was given names
//! ([`media_format_of`]) and this family's own sample rate
//! ([`CALL_AUDIO_SAMPLE_RATE_HERTZ`]). A custom
//! language model is the MEDICAL vocabulary's case again: its
//! training and tuning data can only be read from the Amazon S3 location a create
//! names ([`LanguageModelRecord`]), so the record is real and the training it cannot
//! run ends in the service's own modelled `FAILED` state — nothing here reports a
//! `COMPLETED` model, because nothing trained one. A Medical Scribe job is the job
//! RECORD too, and the strongest case of the same limit: no conversation is
//! transcribed, no speaker role is identified and no clinical note is generated, so
//! the `MedicalScribeOutput.TranscriptFileUri` / `ClinicalDocumentUri` a completed
//! job reports name locations nothing writes to or serves (see
//! [`medical_scribe_output_uri`]), and a `FailureReason` stays absent because nothing
//! this emulator does can fail a job it accepted. A MEDICAL transcription job is the
//! transcription-job RECORD again, under its own name: no audio is transcribed and no
//! personal health information is identified, so the
//! `MedicalTranscript.TranscriptFileUri` a completed job reports names a location
//! nothing writes to or serves (see [`MedicalTranscriptionJobRecord`]).

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AbsoluteTimeRange, CallAnalyticsJob, CallAnalyticsJobSettings, CallAnalyticsJobSummary,
    CategoryProperties, ChannelDefinition, ContentRedaction, EncryptionConfiguration,
    InputDataConfig, JobExecutionSettings, LanguageIdSettings, LanguageModel, Media,
    MedicalScribeChannelDefinition, MedicalScribeJob, MedicalScribeJobSummary,
    MedicalScribeOutput,
    MedicalScribeSettings, MedicalTranscript, MedicalTranscriptionJob,
    MedicalTranscriptionJobSummary, MedicalTranscriptionSetting, ModelSettings, RelativeTimeRange,
    Rule, Settings, SubtitlesOutput, Tag,
    ToxicityDetectionSettings, Transcript, TranscriptionJob, TranscriptionJobSummary,
    VocabularyFilterInfo, VocabularyInfo, MEDIA_FORMAT_VALUES,
    MEDICAL_SCRIBE_LANGUAGE_CODE_VALUES, REDACTION_OUTPUT_VALUES,
};

/// `TranscriptionJobStatus` values, from the model's enum
/// (`smithy_query(query="shape", shape="TranscriptionJobStatus")`).
pub const STATUS_QUEUED: &str = "QUEUED";
/// See [`STATUS_QUEUED`].
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`STATUS_QUEUED`].
pub const STATUS_COMPLETED: &str = "COMPLETED";
/// See [`STATUS_QUEUED`]. No job this emulator records reaches it: nothing it does
/// can fail a job that was accepted.
pub const STATUS_FAILED: &str = "FAILED";

/// Whether a transcription job in `status` has finished, so the service is no longer
/// working on it.
///
/// `TranscriptionJobStatus` is `QUEUED | IN_PROGRESS | FAILED | COMPLETED`
/// (`smithy_query(query="shape", shape="TranscriptionJobStatus")`); the first two are
/// the non-terminal ones a delete is refused for, per `BadRequestException`'s own
/// documentation ("if it's in a non-terminal state (such as IN PROGRESS)").
///
/// `MedicalScribeJobStatus` and `CallAnalyticsJobStatus` are modelled with the same
/// four values (`smithy_query(query="shape", shape="MedicalScribeJobStatus")`,
/// `smithy_query(query="shape", shape="CallAnalyticsJobStatus")`) and their deletes
/// carry the same `BadRequestException`, so this one predicate answers for every job
/// family rather than each handler spelling its own list.
pub fn is_terminal_job_status(status: &str) -> bool {
    status == STATUS_COMPLETED || status == STATUS_FAILED
}

/// The two lifecycle facts every job family's record holds: the status it is in, and
/// the clock-owed transitions that status still has to catch up with.
///
/// One trait rather than four copies of [`jobs_in_flight`], which is what every
/// family's concurrency quota is measured against: the four job records differ in
/// what a job IS, not in how its status moves.
pub trait JobLifecycle {
    /// Materialise every status transition the clock already owes this job — the
    /// record's own `advance_clock`.
    fn materialise_due_transitions(&mut self, now: SystemTime);

    /// The job's current status, one of the four values its family's `*JobStatus`
    /// enum models.
    fn job_status(&self) -> &str;

    /// The job's creation sequence number, so "the oldest record" is a total order
    /// even for two jobs started in the same clock tick.
    fn sequence(&self) -> u64;
}

impl JobLifecycle for TranscriptionJobRecord {
    fn materialise_due_transitions(&mut self, now: SystemTime) {
        self.advance_clock(now);
    }

    fn job_status(&self) -> &str {
        &self.status
    }

    fn sequence(&self) -> u64 {
        self.sequence
    }
}

impl JobLifecycle for MedicalTranscriptionJobRecord {
    fn materialise_due_transitions(&mut self, now: SystemTime) {
        self.advance_clock(now);
    }

    fn job_status(&self) -> &str {
        &self.status
    }

    fn sequence(&self) -> u64 {
        self.sequence
    }
}

impl JobLifecycle for CallAnalyticsJobRecord {
    fn materialise_due_transitions(&mut self, now: SystemTime) {
        self.advance_clock(now);
    }

    fn job_status(&self) -> &str {
        &self.status
    }

    fn sequence(&self) -> u64 {
        self.sequence
    }
}

impl JobLifecycle for MedicalScribeJobRecord {
    fn materialise_due_transitions(&mut self, now: SystemTime) {
        self.advance_clock(now);
    }

    fn job_status(&self) -> &str {
        &self.status
    }

    fn sequence(&self) -> u64 {
        self.sequence
    }
}

/// How many of `jobs` the service is still working on at `now`: the records whose
/// status is not terminal ([`is_terminal_job_status`]), counted after every
/// clock-owed transition has been materialised, so a job the clock already finished
/// is not counted as running.
///
/// This — not how many records the account holds — is what a job family's ceiling
/// bounds (see [`MAX_CONCURRENT_TRANSCRIPTION_JOBS`]), so a finished job releases its
/// slot while staying listable.
pub fn jobs_in_flight<R: JobLifecycle>(
    jobs: &mut BTreeMap<String, R>,
    now: SystemTime,
) -> usize {
    let mut in_flight = 0;
    for record in jobs.values_mut() {
        record.materialise_due_transitions(now);
        if !is_terminal_job_status(record.job_status()) {
            in_flight += 1;
        }
    }
    in_flight
}

/// The most job records of one family this emulator KEEPS.
///
/// Not a quota and not a refusal: a start is never refused for it. It is the bound on
/// this process's memory, since the quota above counts only the jobs in flight and so
/// cannot cap how many records a caller that keeps starting jobs accumulates. A
/// CHOSEN figure, several times the concurrency quota so recent history stays
/// readable and listable; the real service keeps a job record far longer than any
/// emulator run.
pub const MAX_RETAINED_JOB_RECORDS: usize = 500;

/// Keep a family's store at [`MAX_RETAINED_JOB_RECORDS`] records by dropping the
/// OLDEST ones the service has already finished with, as a start that adds a record
/// does.
///
/// Only a terminal record is ever dropped, so no job in flight and no job a caller
/// could still be polling to completion disappears; a record is dropped exactly as a
/// delete of that job would drop it. With the concurrency quota capping how many jobs
/// can be in flight at once, the two together bound a family's store at
/// [`MAX_RETAINED_JOB_RECORDS`] plus that quota.
pub fn evict_oldest_finished_jobs<R: JobLifecycle>(jobs: &mut BTreeMap<String, R>) {
    if jobs.len() <= MAX_RETAINED_JOB_RECORDS {
        return;
    }
    let excess = jobs.len() - MAX_RETAINED_JOB_RECORDS;
    let mut finished: Vec<(u64, String)> = jobs
        .iter()
        .filter(|(_, record)| is_terminal_job_status(record.job_status()))
        .map(|(name, record)| (record.sequence(), name.clone()))
        .collect();
    finished.sort_unstable();
    for (_, name) in finished.into_iter().take(excess) {
        jobs.remove(&name);
    }
}

/// The refusal text a start past a job family's concurrency ceiling carries:
/// `limit` jobs of `family` are already running, so the caller has to wait for one to
/// finish rather than delete a record.
///
/// One spelling for every family, since the condition is the same one in each: the
/// modelled `LimitExceededException` itself documents the remedy as waiting ("Wait
/// before retrying your request",
/// `smithy_query(query="shape", shape="LimitExceededException")`).
pub fn concurrency_limit_message(limit: usize, family: &str) -> String {
    format!(
        "You have reached the limit of {limit} concurrent {family} for this account. \
         Wait for a running job to finish and try your request again."
    )
}

/// The one language an AWS HealthScribe job runs in.
///
/// `MedicalScribeLanguageCode` is modelled as an enum with the single value `en-US`
/// (`smithy_query(query="shape", shape="MedicalScribeLanguageCode")`, also
/// [`MEDICAL_SCRIBE_LANGUAGE_CODE_VALUES`]) and `StartMedicalScribeJob` takes no
/// language member at all, so the language a recorded Medical Scribe job reports is
/// that value rather than anything the request chose.
pub const MEDICAL_SCRIBE_LANGUAGE_CODE: &str = MEDICAL_SCRIBE_LANGUAGE_CODE_VALUES[0];

/// The most Medical Scribe jobs this emulator runs AT ONCE for an account.
///
/// A CHOSEN figure, not a published one — AWS gives no per-account number for
/// HealthScribe jobs in the model — applying the same one the other families use. It
/// bounds the jobs IN FLIGHT, like every other family's
/// ([`MAX_CONCURRENT_TRANSCRIPTION_JOBS`]): a start made while that many jobs of this
/// family are still running answers the `LimitExceededException` the operation models
/// for an exceeded quota, and a job that has reached a terminal status releases its
/// slot while its record stays readable and listable.
pub const MAX_CONCURRENT_MEDICAL_SCRIBE_JOBS: usize = MAX_CONCURRENT_TRANSCRIPTION_JOBS;

/// The family [`concurrency_limit_message`] names in a Medical Scribe start's refusal.
pub const FAMILY_MEDICAL_SCRIBE_JOBS: &str = "Medical Scribe jobs";

/// The object name of the transcript a completed Medical Scribe job reports, under
/// the prefix [`medical_scribe_output_uri`] builds.
///
/// CHOSEN, like the prefix: `MedicalScribeOutput$TranscriptFileUri` documents only
/// that it "Holds the Amazon S3 URI for the Transcript"
/// (`smithy_query(query="doc", shape="MedicalScribeOutput", member="TranscriptFileUri")`),
/// and neither the model nor the operation's documentation states an object key.
pub const MEDICAL_SCRIBE_TRANSCRIPT_FILE: &str = "transcript.json";

/// The object name of the clinical document a completed Medical Scribe job reports.
/// See [`MEDICAL_SCRIBE_TRANSCRIPT_FILE`].
pub const MEDICAL_SCRIBE_CLINICAL_DOCUMENT_FILE: &str = "summary.json";

/// Where one of a Medical Scribe job's two output files is reported to live.
///
/// Reported, not served — exactly as a transcription job's
/// `Transcript.TranscriptFileUri` is (see this module's header): nothing writes an
/// object at this URI, because nothing transcribes the conversation or generates the
/// clinical note. `OutputBucketName` is REQUIRED for this family
/// (`smithy_query(query="operation", operation="StartMedicalScribeJob")`), so there is
/// no service-managed branch as [`output_uri`] has: both files are in the caller's
/// bucket, under a prefix named for the job so two jobs writing to one bucket do not
/// collide.
pub fn medical_scribe_output_uri(bucket: &str, job_name: &str, file: &str) -> String {
    format!("s3://{bucket}/{job_name}/{file}")
}

/// `OutputLocationType` values, from the model's enum
/// (`smithy_query(query="shape", shape="OutputLocationType")`): the transcript is
/// in the caller's bucket when the request named one, else in a service-managed one.
pub const OUTPUT_LOCATION_CUSTOMER_BUCKET: &str = "CUSTOMER_BUCKET";
/// See [`OUTPUT_LOCATION_CUSTOMER_BUCKET`].
pub const OUTPUT_LOCATION_SERVICE_BUCKET: &str = "SERVICE_BUCKET";

/// The region an unsigned request is served in. `emulator_arn`'s contract states
/// emulators default to `us-east-1`; a signed request carries its own region in the
/// SigV4 credential scope.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after the call that starts it a transcription job's next status
/// transition is owed (`QUEUED` -> `IN_PROGRESS` -> `COMPLETED`).
///
/// A chosen span, not a documented one: Amazon Transcribe gives no processing-time
/// guarantee. It is deliberately non-zero, so a caller that reads the job straight
/// back still observes the non-terminal status (and a poll-until-done script makes
/// at least one real poll), and short enough that such a script terminates quickly.
/// The due instant is stored on the record; nothing ever sleeps for it.
pub const JOB_TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The number of jobs `ListTranscriptionJobs` / `ListCallAnalyticsJobs` return when
/// the request names no `MaxResults`: "If you do not specify a value, a default of 5 is
/// used" (model doc for `ListTranscriptionJobsRequest$MaxResults` and for
/// `ListCallAnalyticsJobsRequest$MaxResults`).
pub const DEFAULT_LIST_MAX_RESULTS: i32 = 5;

/// The largest page `ListTranscriptionJobs` / `ListCallAnalyticsJobs` serve: their
/// `MaxResults` is modelled `@range(min: 1, max: 100)`, so the boundary validator
/// refuses more than this before a handler runs.
pub const MAX_LIST_MAX_RESULTS: usize = 100;

/// The most transcription jobs this emulator runs AT ONCE for an account.
///
/// The quota Amazon Transcribe publishes for this family is a CONCURRENCY one — 100
/// concurrent batch transcription jobs per account per Region — and the
/// `LimitExceededException` a start past it answers documents its own remedy as
/// waiting rather than deleting ("You've either sent too many requests or your input
/// file is too long. Wait before retrying your request",
/// `smithy_query(query="shape", shape="LimitExceededException")`); the model says the
/// same from the other side, since `JobExecutionSettings$AllowDeferredExecution`
/// queues a request "until the number of jobs falls below the concurrent request
/// limit". So this figure bounds the jobs the service is still working on
/// ([`jobs_in_flight`]), never the records an account holds: a job that reached a
/// terminal status releases its slot and stays readable and listable, which is what
/// makes `ListTranscriptionJobs`' documented `COMPLETED` status filter meaningful.
pub const MAX_CONCURRENT_TRANSCRIPTION_JOBS: usize = 100;

/// The family [`concurrency_limit_message`] names in a transcription start's refusal.
pub const FAMILY_TRANSCRIPTION_JOBS: &str = "transcription jobs";

/// The most MEDICAL transcription jobs this emulator runs AT ONCE for an account.
///
/// Amazon Transcribe Medical answers the same `LimitExceededException` for an
/// exceeded quota, so this family is bounded the same way the non-medical one is
/// ([`MAX_CONCURRENT_TRANSCRIPTION_JOBS`]): by how many of its jobs are in flight,
/// with a terminal job releasing its slot.
pub const MAX_CONCURRENT_MEDICAL_TRANSCRIPTION_JOBS: usize = MAX_CONCURRENT_TRANSCRIPTION_JOBS;

/// The family [`concurrency_limit_message`] names in a medical transcription start's
/// refusal.
pub const FAMILY_MEDICAL_TRANSCRIPTION_JOBS: &str = "medical transcription jobs";

/// The most Call Analytics jobs this emulator runs AT ONCE for an account.
///
/// A CHOSEN figure, not a published one: the model carries no per-account number for
/// Call Analytics jobs, but it does describe the ceiling this family has as a
/// concurrency one — a start is queued when an account "exceeds your Concurrent Job
/// Limit" (`smithy_query(query="operation", operation="StartCallAnalyticsJob")`). So
/// the same figure the other families use bounds the jobs in flight
/// ([`MAX_CONCURRENT_TRANSCRIPTION_JOBS`]), and a terminal job releases its slot.
pub const MAX_CONCURRENT_CALL_ANALYTICS_JOBS: usize = MAX_CONCURRENT_TRANSCRIPTION_JOBS;

/// The family [`concurrency_limit_message`] names in a Call Analytics start's refusal.
pub const FAMILY_CALL_ANALYTICS_JOBS: &str = "Call Analytics jobs";

/// `VocabularyState` values, from the model's enum
/// (`smithy_query(query="shape", shape="VocabularyState")`).
pub const VOCABULARY_STATE_PENDING: &str = "PENDING";
/// See [`VOCABULARY_STATE_PENDING`].
pub const VOCABULARY_STATE_READY: &str = "READY";
/// See [`VOCABULARY_STATE_PENDING`]. Where a vocabulary whose terms would have had to
/// be READ from a file ends up, in EITHER family: the record is kept and the build
/// this emulator cannot perform fails, carrying a `FailureReason` that says the file
/// was not read ([`file_not_read_failure_reason`]).
pub const VOCABULARY_STATE_FAILED: &str = "FAILED";

/// How long after the call that starts it a custom vocabulary's next state
/// transition is owed (`PENDING` -> `READY` for one built from the terms the request
/// listed, `PENDING` -> `FAILED` for one whose terms would have come from a file this
/// emulator does not read).
///
/// A chosen span, not a documented one: Amazon Transcribe gives no processing-time
/// guarantee for building a custom vocabulary. It is deliberately non-zero, so a
/// caller that reads the vocabulary straight back still observes `PENDING` (and the
/// state errors that go with it), and short enough that a poll-until-ready script
/// terminates quickly. The due instant is stored on the record; nothing sleeps for it.
pub const VOCABULARY_TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The only language a custom medical vocabulary (or any Amazon Transcribe Medical
/// resource) may be in: "US English (en-US) is the only language supported with
/// Amazon Transcribe Medical" (model doc for
/// `CreateMedicalVocabularyRequest$LanguageCode`).
///
/// The member targets the service-wide `LanguageCode` enum, so the model's own
/// constraint traits cannot express this narrowing: the handler is where a request in
/// any other language is refused.
pub const MEDICAL_LANGUAGE_CODE: &str = "en-US";

/// The most custom vocabularies this emulator holds for an account.
///
/// Amazon Transcribe publishes a default quota of 100 custom vocabularies per account
/// per Region, and the same figure for custom medical vocabularies and for custom
/// vocabulary filters. Applying the published figure to each family keeps the store
/// bounded: a caller that never deletes cannot grow it without end, and
/// `DeleteVocabulary` / `DeleteMedicalVocabulary` / `DeleteVocabularyFilter` is what
/// makes room again.
pub const MAX_VOCABULARIES: usize = 100;

/// The number of custom vocabularies `ListVocabularies` / `ListMedicalVocabularies` /
/// `ListVocabularyFilters` return when the request names no `MaxResults`: "If you do
/// not specify a value, a default of 5 is used" (model doc for
/// `ListVocabulariesRequest$MaxResults`, `ListMedicalVocabulariesRequest$MaxResults`
/// and `ListVocabularyFiltersRequest$MaxResults`).
pub const DEFAULT_VOCABULARY_LIST_MAX_RESULTS: i32 = 5;

/// The number of Call Analytics categories `ListCallAnalyticsCategories` returns when
/// the request names no `MaxResults`: "If you do not specify a value, a default of 5
/// is used" (model doc for `ListCallAnalyticsCategoriesRequest$MaxResults`).
pub const DEFAULT_CATEGORY_LIST_MAX_RESULTS: i32 = 5;

/// The number of custom language models `ListLanguageModels` returns when the request
/// names no `MaxResults`: "If you do not specify a value, a default of 5 is used"
/// (model doc for `ListLanguageModelsRequest$MaxResults`).
pub const DEFAULT_MODEL_LIST_MAX_RESULTS: i32 = 5;

/// `InputType` values, from the model's enum
/// (`smithy_query(query="shape", shape="InputType")`, also
/// [`crate::ops::types::INPUT_TYPE_VALUES`]): a Call Analytics category applies
/// either to post-call transcriptions or to streaming ones.
pub const INPUT_TYPE_POST_CALL: &str = "POST_CALL";
/// See [`INPUT_TYPE_POST_CALL`].
pub const INPUT_TYPE_REAL_TIME: &str = "REAL_TIME";

/// The `InputType` a Call Analytics category created without one gets: "If you do not
/// include InputType, your category is created as a post-call category by default"
/// (model doc for `CreateCallAnalyticsCategoryRequest$InputType`). The default is
/// RECORDED on the category, so every operation that reports it reports the same
/// value rather than each one re-defaulting.
pub const DEFAULT_INPUT_TYPE: &str = INPUT_TYPE_POST_CALL;

/// The most Call Analytics categories this emulator holds for an account.
///
/// A CHOSEN ceiling, not a published one: AWS gives no per-account figure for Call
/// Analytics categories in the model, and the only count the model bounds is the
/// RULES of one category (`RuleList` is `length {min 1, max 20}`, which the boundary
/// validator already enforces). Applying the same figure the other families use keeps
/// the store bounded — a caller that never deletes cannot grow it without end, and
/// `DeleteCallAnalyticsCategory` is what makes room again.
pub const MAX_CALL_ANALYTICS_CATEGORIES: usize = 100;

/// `ModelStatus` values, from the model's enum
/// (`smithy_query(query="shape", shape="ModelStatus")`): a custom language model is
/// being trained, has finished training, or failed.
pub const MODEL_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`MODEL_STATUS_IN_PROGRESS`]. No model this emulator records reaches it:
/// training would have to READ the training data at `InputDataConfig$S3Uri`, which
/// this emulator does not do (see [`LanguageModelRecord`]).
pub const MODEL_STATUS_COMPLETED: &str = "COMPLETED";
/// See [`MODEL_STATUS_IN_PROGRESS`].
pub const MODEL_STATUS_FAILED: &str = "FAILED";

/// Whether a custom language model in `status` has finished training, so the service
/// is no longer working on it.
///
/// `ModelStatus` is `IN_PROGRESS | FAILED | COMPLETED`
/// (`smithy_query(query="shape", shape="ModelStatus")`); the first is the
/// non-terminal one a delete is refused for, per `BadRequestException`'s own
/// documentation ("if it's in a non-terminal state (such as IN PROGRESS)"), the same
/// rule the vocabulary and job families apply ([`is_terminal_job_status`]).
pub fn is_terminal_model_status(status: &str) -> bool {
    status == MODEL_STATUS_COMPLETED || status == MODEL_STATUS_FAILED
}

/// How long after the call that starts it a custom language model's next status
/// transition is owed (`IN_PROGRESS` -> `FAILED` for one whose training data this
/// emulator cannot read).
///
/// A chosen span, not a documented one: Amazon Transcribe gives no training-time
/// guarantee for a custom language model. It is deliberately non-zero, so a caller
/// that describes the model straight back still observes `IN_PROGRESS` (and the
/// state errors that go with it), and short enough that a poll-until-done script
/// terminates quickly. The due instant is stored on the record; nothing sleeps for it.
pub const MODEL_TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The most custom language models this emulator holds for an account.
///
/// A CHOSEN ceiling, not a published one: the model carries no per-account figure for
/// custom language models. Applying the same figure the other families use keeps the
/// store bounded — a caller that never deletes cannot grow it without end, and
/// `DeleteLanguageModel` is what makes room again.
pub const MAX_LANGUAGE_MODELS: usize = 100;

/// The ARN resource type of a transcription job, as the `TranscribeArn` pattern's
/// `<type>/<name>` resource segment spells it
/// (`^arn:aws(-[^:]+)?:transcribe:[a-zA-Z0-9-]*:[0-9]{12}:[a-zA-Z-]*/[0-9a-zA-Z._-]+$`),
/// and as the Amazon Transcribe "Tagging resources" guide lists it.
pub const RESOURCE_TYPE_TRANSCRIPTION_JOB: &str = "transcription-job";
/// The ARN resource type of a MEDICAL transcription job, as the model's own
/// `ResourceArn` doc lists it among the valid `resource-type` values
/// (`smithy_query(query="doc", shape="TagResourceRequest", member="ResourceArn")`) —
/// a resource type of its own, so a medical transcription job and a transcription job
/// of the same name are two resources. See [`RESOURCE_TYPE_TRANSCRIPTION_JOB`].
pub const RESOURCE_TYPE_MEDICAL_TRANSCRIPTION_JOB: &str = "medical-transcription-job";
/// The ARN resource type of a custom vocabulary. See
/// [`RESOURCE_TYPE_TRANSCRIPTION_JOB`].
pub const RESOURCE_TYPE_VOCABULARY: &str = "vocabulary";
/// The ARN resource type of a custom MEDICAL vocabulary, as the Amazon Transcribe
/// "Tagging resources" guide lists it — a resource type of its own, so a medical
/// vocabulary and a custom vocabulary of the same name are two resources. See
/// [`RESOURCE_TYPE_TRANSCRIPTION_JOB`].
pub const RESOURCE_TYPE_MEDICAL_VOCABULARY: &str = "medical-vocabulary";
/// The ARN resource type of a custom vocabulary filter. See
/// [`RESOURCE_TYPE_TRANSCRIPTION_JOB`].
pub const RESOURCE_TYPE_VOCABULARY_FILTER: &str = "vocabulary-filter";
/// The ARN resource type of a custom language model, as the Amazon Transcribe
/// "Tagging resources" guide lists it. See [`RESOURCE_TYPE_TRANSCRIPTION_JOB`].
pub const RESOURCE_TYPE_LANGUAGE_MODEL: &str = "language-model";
/// The ARN resource type of a Call Analytics category, as the Amazon Transcribe
/// resource-type ARN formats spell this family's `<type>/<name>` segment
/// (`arn:${Partition}:transcribe:${Region}:${Account}:analytics-category/${CategoryName}`).
/// See [`RESOURCE_TYPE_TRANSCRIPTION_JOB`].
pub const RESOURCE_TYPE_CALL_ANALYTICS_CATEGORY: &str = "analytics-category";
/// The ARN resource type of a Medical Scribe (AWS HealthScribe) job, spelled as this
/// family's published `<type>/${JobName}` segment — the same hyphenated family name
/// the other job families' types use. See [`RESOURCE_TYPE_TRANSCRIPTION_JOB`].
pub const RESOURCE_TYPE_MEDICAL_SCRIBE_JOB: &str = "medical-scribe-job";

/// The word this service's own client-error messages use for the kind of resource an
/// ARN's `<type>` segment names, for each type this emulator stores records of.
///
/// The kinds are the ones the service's recorded messages name ("vocabulary",
/// "vocabulary filter", "job"), so one template plus a kind words a refusal for every
/// family rather than each handler spelling its own. An ARN whose type names nothing
/// this emulator holds gets the neutral `KIND_RESOURCE`.
pub const KIND_JOB: &str = "job";
/// See [`KIND_JOB`].
pub const KIND_VOCABULARY: &str = "vocabulary";
/// See [`KIND_JOB`].
pub const KIND_MEDICAL_VOCABULARY: &str = "medical vocabulary";
/// See [`KIND_JOB`].
pub const KIND_VOCABULARY_FILTER: &str = "vocabulary filter";
/// See [`KIND_JOB`].
pub const KIND_LANGUAGE_MODEL: &str = "language model";
/// See [`KIND_JOB`]: the word the recorded Call Analytics category responses name
/// that family by ("The category that you've specified couldn't be found. Check the
/// category name and try your request again.").
pub const KIND_CATEGORY: &str = "category";
/// See [`KIND_JOB`].
pub const KIND_RESOURCE: &str = "resource";

/// A family of resource this emulator holds records of, as an ARN's `<type>` segment
/// names it.
///
/// The ARN `<type>` segment is what says WHICH family a tagging operation is talking
/// about, so the mapping from that segment to a family is single-homed here: the tag
/// lookups ([`ComAmazonawsTranscribeTranscribeState::tags`] /
/// [`ComAmazonawsTranscribeTranscribeState::tags_mut`]) and the word a refusal names
/// the family by ([`resource_kind`]) all read the same mapping, so a family cannot be
/// reachable through one and unknown to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceFamily {
    /// [`RESOURCE_TYPE_TRANSCRIPTION_JOB`].
    TranscriptionJob,
    /// [`RESOURCE_TYPE_MEDICAL_TRANSCRIPTION_JOB`].
    MedicalTranscriptionJob,
    /// [`RESOURCE_TYPE_VOCABULARY`].
    Vocabulary,
    /// [`RESOURCE_TYPE_MEDICAL_VOCABULARY`].
    MedicalVocabulary,
    /// [`RESOURCE_TYPE_VOCABULARY_FILTER`].
    VocabularyFilter,
    /// [`RESOURCE_TYPE_LANGUAGE_MODEL`].
    LanguageModel,
    /// [`RESOURCE_TYPE_CALL_ANALYTICS_CATEGORY`].
    CallAnalyticsCategory,
    /// [`RESOURCE_TYPE_MEDICAL_SCRIBE_JOB`].
    MedicalScribeJob,
}

impl ResourceFamily {
    /// The family an ARN's `<type>` segment names, or `None` for a type segment this
    /// emulator holds no records of at all — which every caller renders as the
    /// modelled not-found error, never as an empty tag set.
    pub fn from_resource_type(resource_type: &str) -> Option<Self> {
        match resource_type {
            RESOURCE_TYPE_TRANSCRIPTION_JOB => Some(Self::TranscriptionJob),
            RESOURCE_TYPE_MEDICAL_TRANSCRIPTION_JOB => Some(Self::MedicalTranscriptionJob),
            RESOURCE_TYPE_VOCABULARY => Some(Self::Vocabulary),
            RESOURCE_TYPE_MEDICAL_VOCABULARY => Some(Self::MedicalVocabulary),
            RESOURCE_TYPE_VOCABULARY_FILTER => Some(Self::VocabularyFilter),
            RESOURCE_TYPE_LANGUAGE_MODEL => Some(Self::LanguageModel),
            RESOURCE_TYPE_CALL_ANALYTICS_CATEGORY => Some(Self::CallAnalyticsCategory),
            RESOURCE_TYPE_MEDICAL_SCRIBE_JOB => Some(Self::MedicalScribeJob),
            _ => None,
        }
    }

    /// The ARN `<type>` segment that names this family.
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::TranscriptionJob => RESOURCE_TYPE_TRANSCRIPTION_JOB,
            Self::MedicalTranscriptionJob => RESOURCE_TYPE_MEDICAL_TRANSCRIPTION_JOB,
            Self::Vocabulary => RESOURCE_TYPE_VOCABULARY,
            Self::MedicalVocabulary => RESOURCE_TYPE_MEDICAL_VOCABULARY,
            Self::VocabularyFilter => RESOURCE_TYPE_VOCABULARY_FILTER,
            Self::LanguageModel => RESOURCE_TYPE_LANGUAGE_MODEL,
            Self::CallAnalyticsCategory => RESOURCE_TYPE_CALL_ANALYTICS_CATEGORY,
            Self::MedicalScribeJob => RESOURCE_TYPE_MEDICAL_SCRIBE_JOB,
        }
    }

    /// The word this service's own client-error messages name this family by.
    ///
    /// A Medical Scribe job and a medical transcription job are each a "job" like the
    /// transcription-job family: the recorded not-found response of every job family
    /// reads "The requested job couldn't be found."
    pub fn kind(self) -> &'static str {
        match self {
            Self::TranscriptionJob | Self::MedicalTranscriptionJob | Self::MedicalScribeJob => {
                KIND_JOB
            }
            Self::Vocabulary => KIND_VOCABULARY,
            Self::MedicalVocabulary => KIND_MEDICAL_VOCABULARY,
            Self::VocabularyFilter => KIND_VOCABULARY_FILTER,
            Self::LanguageModel => KIND_LANGUAGE_MODEL,
            Self::CallAnalyticsCategory => KIND_CATEGORY,
        }
    }
}

/// The resource kind the ARN resource type `resource_type` denotes — the word a
/// refusal about that resource names it by.
///
/// Reads [`ResourceFamily`]'s own mapping, so the kind a refusal names and the store a
/// tag lookup reaches cannot disagree about which ARN names which family. A type
/// segment that names no family of this mapping at all gets the neutral
/// [`KIND_RESOURCE`].
pub fn resource_kind(resource_type: &str) -> &'static str {
    ResourceFamily::from_resource_type(resource_type).map_or(KIND_RESOURCE, ResourceFamily::kind)
}

/// How this service words "there is no such resource", for a resource of `kind`.
///
/// The service's own texts name the resource KIND and do NOT echo the name the
/// caller asked about (the recorded responses read "The requested job couldn't be
/// found. Check the job name and try your request again."), so the message carries
/// no request value. Single-homed here because every family's not-found refusal uses
/// it; which error SHAPE carries it is the handler's own modelled choice
/// (`BadRequestException` — see [`crate::scaffolded::wire_status`] for the status
/// side of the same error channel).
pub fn not_found_message(kind: &str) -> String {
    format!(
        "The requested {kind} couldn't be found. Check the {kind} name and try your \
         request again."
    )
}

/// How this service words "that name is already taken", for a resource of `kind`.
///
/// Same shape of text as [`not_found_message`], and the same reason for naming only
/// the kind: "The requested vocabulary filter name already exists. Use a different
/// vocabulary filter name."
pub fn name_already_exists_message(kind: &str) -> String {
    format!("The requested {kind} name already exists. Use a different {kind} name.")
}

/// How this service refuses a `Settings` toggle and the value that qualifies it when
/// only one of the two is in the request, or `None` when the pair holds together.
///
/// A settings toggle and the value that qualifies it are CO-REQUIRED in both
/// directions: the model documents each direction on the member itself — "If you
/// enable `ShowSpeakerLabels` in your request, you must also include
/// `MaxSpeakerLabels`" and "If you specify the `MaxSpeakerLabels` field, you must set
/// the `ShowSpeakerLabels` field to true", with the same two sentences for
/// `ShowAlternatives` / `MaxAlternatives` and for `VocabularyFilterName` /
/// `VocabularyFilterMethod` (`smithy_query(query="shape",
/// shape="Settings,MedicalTranscriptionSetting,MedicalScribeSettings")`). A request
/// that satisfies only one side of such a pair is refused rather than stored, so a
/// stored job never reports speaker partitioning with no speaker ceiling (or the
/// reverse). Single-homed here because every settings shape of this service that
/// documents such a pair applies the same rule.
///
/// `enabled` is whether the toggle is present AND asks for the feature; `qualifier_set`
/// whether the qualifying member is present at all — a qualifier with the toggle
/// absent and one with the toggle explicitly off are the same refusal.
pub fn co_required_setting_error(
    toggle: &str,
    enabled: bool,
    qualifier: &str,
    qualifier_set: bool,
) -> Option<String> {
    co_required_pair_error("Settings", toggle, enabled, qualifier, qualifier_set)
}

/// How this service refuses a member pair that is co-required in BOTH directions when
/// a request carries one side of it and not the other, or `None` when the pair holds
/// together (both sides, or neither).
///
/// A pair whose two members are each documented as requiring the other has ONE refusal
/// whichever side is missing: the member the caller DID send, and the companion it
/// needs, both spelled from `parent` as the caller spells them
/// (`Settings.ShowSpeakerLabels`, `Rules[0].SentimentFilter.RelativeTimeRange.StartPercentage`).
/// Single-homed here because every such pair of this service — the settings toggles and
/// the values that qualify them ([`co_required_setting_error`]), and the bounded
/// segment of a time range ([`time_range_co_requirement_error`]) — is refused the same
/// way, and because Smithy's constraint traits cannot express a conditional
/// co-requirement, so no boundary validation can refuse one.
pub fn co_required_pair_error(
    parent: &str,
    left: &str,
    left_present: bool,
    right: &str,
    right_present: bool,
) -> Option<String> {
    match (left_present, right_present) {
        (true, false) => Some(missing_companion_message(
            &format!("{parent}.{left}"),
            &format!("{parent}.{right}"),
        )),
        (false, true) => Some(missing_companion_message(
            &format!("{parent}.{right}"),
            &format!("{parent}.{left}"),
        )),
        _ => None,
    }
}

/// How this service words "the member you included needs one you left out", for
/// `member` present in a request whose companion `companion` is absent.
///
/// The one template for every co-requirement of this service, so a refusal reads the
/// same whichever member pair it is about: the BOTH-ways settings pairs that
/// [`co_required_setting_error`] decides and the ONE-way requirements a request member
/// documents on itself ("If you include X in your request, you must also include Y").
/// Both member names are spelled by the CALLER, fully qualified
/// (`Settings.VocabularyFilterMethod`, `JobExecutionSettings.DataAccessRoleArn`), so
/// the text names the member path the caller sent and carries no other request value.
pub fn missing_companion_message(member: &str, companion: &str) -> String {
    format!(
        "{companion} is required when {member} is specified. Correct your request and \
         try again."
    )
}

/// The refusal for a ONE-way co-requirement: `Some(message)` when `member` is in the
/// request and the `companion` it documents as co-required is not, `None` otherwise.
///
/// Several members of this service's start requests document the same rule on the
/// member itself — "Note that if you include `VocabularyFilterName` in your request,
/// you must also include `VocabularyFilterMethod`", "If you include
/// `JobExecutionSettings` in your request, you must also include the sub-parameters:
/// `AllowDeferredExecution` and `DataAccessRoleArn`", "If you include `ModelSettings`
/// in your request, you must include the `LanguageModelName` sub-parameter", "If you
/// include `LanguageOptions` in your request, you must also include
/// `IdentifyLanguage`", "Use in combination with `OutputBucketName`"
/// (`smithy_query(query="operation", operation="StartTranscriptionJob")` and
/// `smithy_query(query="shape", shape="Settings,ModelSettings,JobExecutionSettings")`).
/// Unlike [`co_required_setting_error`] the rule holds in ONE direction only: the
/// companion alone is a whole request.
pub fn missing_companion_error(
    member: &str,
    present: bool,
    companion: &str,
    companion_present: bool,
) -> Option<String> {
    (present && !companion_present).then(|| missing_companion_message(member, companion))
}

/// The refusal for a half-specified time range under `parent`, or `None` when both the
/// time ranges it carries are whole.
///
/// Each of the two time-range shapes bounds a custom segment with a PAIR of members
/// that are co-required in both directions — "If you include `StartTime` in your
/// request, you must also include `EndTime`" and the converse, with the shape doc
/// repeating "You must use `StartTime` and `EndTime` as a set; that is, if you include
/// one, you must include both", and the same two sentences for `StartPercentage` /
/// `EndPercentage` (`smithy_query(query="shape",
/// shape="AbsoluteTimeRange,RelativeTimeRange")`). The open-ended members `First` and
/// `Last` carry no such sentence ("You can use `First` and `Last` independently of each
/// other"), so either alone is a whole range. Single-homed here because every filter
/// that can carry a time range carries the same two shapes, so no operation ends up
/// checking a different pair. A filter carrying BOTH shapes half-specified is refused
/// over the ABSOLUTE one, so which member a refusal names is the same on every run.
pub fn time_range_co_requirement_error(
    parent: &str,
    absolute: Option<&AbsoluteTimeRange>,
    relative: Option<&RelativeTimeRange>,
) -> Option<String> {
    absolute
        .and_then(|range| {
            co_required_pair_error(
                &format!("{parent}.AbsoluteTimeRange"),
                "StartTime",
                range.start_time.is_some(),
                "EndTime",
                range.end_time.is_some(),
            )
        })
        .or_else(|| {
            relative.and_then(|range| {
                co_required_pair_error(
                    &format!("{parent}.RelativeTimeRange"),
                    "StartPercentage",
                    range.start_percentage.is_some(),
                    "EndPercentage",
                    range.end_percentage.is_some(),
                )
            })
        })
}

/// The refusal for the first rule of `rules` that carries a half-specified time range,
/// or `None` when every time range of every rule holds together.
///
/// The `Rule` union's four members are the filters a category's rules are built from,
/// and each one can bound its search to a time range in milliseconds or in percentage
/// (`smithy_query(query="shape",
/// shape="Rule,NonTalkTimeFilter,InterruptionFilter,TranscriptFilter,SentimentFilter")`),
/// so the co-requirement [`time_range_co_requirement_error`] decides is read on every
/// member of the union rather than on the filters one request happened to use. The
/// rules are walked in the order the request lists them, so which rule a refusal names
/// is the same on every run; the name is the member PATH the caller sent, carrying no
/// other request value.
pub fn rule_time_range_error(rules: &[Rule]) -> Option<String> {
    rules.iter().enumerate().find_map(|(index, rule)| {
        let (filter, absolute, relative) = match rule {
            Rule::NonTalkTimeFilter(filter) => (
                "NonTalkTimeFilter",
                filter.absolute_time_range.as_ref(),
                filter.relative_time_range.as_ref(),
            ),
            Rule::InterruptionFilter(filter) => (
                "InterruptionFilter",
                filter.absolute_time_range.as_ref(),
                filter.relative_time_range.as_ref(),
            ),
            Rule::TranscriptFilter(filter) => (
                "TranscriptFilter",
                filter.absolute_time_range.as_ref(),
                filter.relative_time_range.as_ref(),
            ),
            Rule::SentimentFilter(filter) => (
                "SentimentFilter",
                filter.absolute_time_range.as_ref(),
                filter.relative_time_range.as_ref(),
            ),
        };
        time_range_co_requirement_error(&format!("Rules[{index}].{filter}"), absolute, relative)
    })
}

/// The references a request's settings name, as
/// [`ComAmazonawsTranscribeTranscribeState::unheld_reference_error`] reads them: the
/// custom vocabulary, custom vocabulary filter and custom language model a settings
/// shape of this service can name, each paired with the family its name belongs to.
///
/// Every settings shape that can name custom resources names these same three
/// families, so which family each member reads is decided here once rather than in
/// each operation that accepts one.
pub fn custom_resource_references<'a>(
    vocabulary: Option<&'a str>,
    vocabulary_filter: Option<&'a str>,
    language_model: Option<&'a str>,
) -> Vec<(ResourceFamily, Option<&'a str>)> {
    vec![
        (ResourceFamily::Vocabulary, vocabulary),
        (ResourceFamily::VocabularyFilter, vocabulary_filter),
        (ResourceFamily::LanguageModel, language_model),
    ]
}

/// Append every custom resource the per-language `LanguageIdSettings` of a request
/// name to `references`, in the form
/// [`ComAmazonawsTranscribeTranscribeState::unheld_reference_error`] reads.
///
/// `LanguageIdSettings` names resources of the very same three families a request's
/// top-level settings do (`VocabularyName`, `VocabularyFilterName`,
/// `LanguageModelName` — `smithy_query(query="shape", shape="LanguageIdSettings")`),
/// so the expansion is single-homed here: the operations that accept the map cannot
/// end up checking different members of it. The entries are walked in language-code
/// order rather than the map's own, so which dangling name a refusal reports is the
/// same on every run.
pub fn extend_language_id_references<'a>(
    references: &mut Vec<(ResourceFamily, Option<&'a str>)>,
    per_language: Option<&'a std::collections::HashMap<String, LanguageIdSettings>>,
) {
    let Some(per_language) = per_language else {
        return;
    };
    let mut entries: Vec<(&String, &LanguageIdSettings)> = per_language.iter().collect();
    entries.sort_by_key(|(language, _)| *language);
    for (_, settings) in entries {
        references.extend([
            (
                ResourceFamily::Vocabulary,
                settings.vocabulary_name.as_deref(),
            ),
            (
                ResourceFamily::VocabularyFilter,
                settings.vocabulary_filter_name.as_deref(),
            ),
            (
                ResourceFamily::LanguageModel,
                settings.language_model_name.as_deref(),
            ),
        ]);
    }
}

/// The most tags one Amazon Transcribe resource may carry.
///
/// A CHOSEN ceiling, not a modelled one: `TagList` is modelled
/// `length {min 1, max 200}` (`smithy_query(query="operation", operation="TagResource")`),
/// which bounds ONE request's tag list — the boundary validator already refuses a longer
/// one — and AWS publishes no total-per-resource figure for this service. Applying the
/// same figure to the merged set is what keeps a resource's tag set bounded under
/// repeated tagging calls, so a merge past it is refused rather than stored.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// Per-service state for com-amazonaws-transcribe-transcribe. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTranscribeTranscribeState {
    /// Every transcription job this account has started, keyed by its name (which
    /// `StartTranscriptionJob` requires to be unique).
    pub transcription_jobs: BTreeMap<String, TranscriptionJobRecord>,
    /// Every MEDICAL transcription job this account has started, keyed by its name
    /// (which `StartMedicalTranscriptionJob` requires to be unique within an account).
    /// A family of its own: a medical transcription job and a transcription job may
    /// share a name.
    pub medical_transcription_jobs: BTreeMap<String, MedicalTranscriptionJobRecord>,
    /// Every custom vocabulary this account holds, keyed by its name (which
    /// `CreateVocabulary` requires to be unique within an account).
    pub vocabularies: BTreeMap<String, VocabularyRecord>,
    /// Every custom MEDICAL vocabulary this account holds, keyed by its name (which
    /// `CreateMedicalVocabulary` requires to be unique within an account). A family
    /// of its own: a medical vocabulary and a custom vocabulary may share a name.
    pub medical_vocabularies: BTreeMap<String, MedicalVocabularyRecord>,
    /// Every custom vocabulary filter this account holds, keyed by its name (which
    /// `CreateVocabularyFilter` requires to be unique within an account).
    pub vocabulary_filters: BTreeMap<String, VocabularyFilterRecord>,
    /// Every Call Analytics category this account holds, keyed by its name ("Category
    /// names are case sensitive and must be unique within an Amazon Web Services
    /// account" — model doc for `CategoryProperties$CategoryName`).
    pub call_analytics_categories: BTreeMap<String, CallAnalyticsCategoryRecord>,
    /// Every custom language model this account holds, keyed by its name (which
    /// `CreateLanguageModel` requires to be unique within an account: "If you try to
    /// create a new custom language model with the same name as an existing custom
    /// language model, you get a `ConflictException` error").
    pub language_models: BTreeMap<String, LanguageModelRecord>,
    /// Every Medical Scribe job this account has started, keyed by its name (which
    /// `StartMedicalScribeJob` requires to be "unique within an Amazon Web Services
    /// account").
    ///
    /// A job of this family carries the tags its start request asked for, reports them
    /// through its own operations, and is reachable from [`Self::tags`] /
    /// [`Self::tags_mut`] under [`RESOURCE_TYPE_MEDICAL_SCRIBE_JOB`], like every other
    /// family this state holds records of.
    pub medical_scribe_jobs: BTreeMap<String, MedicalScribeJobRecord>,
    /// Every Call Analytics job this account has started, keyed by its name ("Job
    /// names are case sensitive and must be unique within an Amazon Web Services
    /// account" — model doc for `CallAnalyticsJob$CallAnalyticsJobName`).
    ///
    /// A family of its own: a Call Analytics job and a transcription job may share a
    /// name, and like the Medical Scribe jobs it is absent from [`Self::tags`] /
    /// [`Self::tags_mut`] because this emulator has no recorded spelling for the ARN
    /// `<type>` segment of a Call Analytics job. The tags its start request asked for
    /// are carried on the record and reported by that family's own operations.
    pub call_analytics_jobs: BTreeMap<String, CallAnalyticsJobRecord>,
    /// Source of [`TranscriptionJobRecord::sequence`]: a monotonic counter, so
    /// creation order is total even when two jobs start in the same clock tick.
    next_sequence: u64,
}

impl ComAmazonawsTranscribeTranscribeState {
    /// The creation sequence number for the next job started.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }

    /// Materialise every clock-owed status transition, then the stored jobs newest
    /// first — the order `ListTranscriptionJobs` documents ("Jobs are ordered by
    /// creation date, with the newest job first").
    pub fn jobs_newest_first(&mut self, now: SystemTime) -> Vec<&TranscriptionJobRecord> {
        for record in self.transcription_jobs.values_mut() {
            record.advance_clock(now);
        }
        let mut records: Vec<&TranscriptionJobRecord> = self.transcription_jobs.values().collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.sequence));
        records
    }

    /// The MEDICAL transcription job named `name`, with every clock-owed transition of
    /// it already materialised, or `None` when this account has started no such job.
    ///
    /// The single lookup for this family, so every operation that reads or deletes a
    /// medical transcription job sees the same status: the one the clock already owed
    /// it.
    pub fn medical_transcription_job_mut(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut MedicalTranscriptionJobRecord> {
        let record = self.medical_transcription_jobs.get_mut(name)?;
        record.advance_clock(now);
        Some(record)
    }

    /// Materialise every clock-owed MEDICAL transcription job transition, then the
    /// stored jobs newest first — the order `ListMedicalTranscriptionJobs` documents
    /// ("Jobs are ordered by creation date, with the newest job first").
    pub fn medical_transcription_jobs_newest_first(
        &mut self,
        now: SystemTime,
    ) -> Vec<&MedicalTranscriptionJobRecord> {
        for record in self.medical_transcription_jobs.values_mut() {
            record.advance_clock(now);
        }
        let mut records: Vec<&MedicalTranscriptionJobRecord> =
            self.medical_transcription_jobs.values().collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.sequence));
        records
    }

    /// The Medical Scribe job named `name`, with every clock-owed transition of it
    /// already materialised, or `None` when this account has started no such job.
    ///
    /// The single lookup for this family, so every operation that reads or deletes a
    /// Medical Scribe job sees the same status: the one the clock already owed it.
    pub fn medical_scribe_job_mut(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut MedicalScribeJobRecord> {
        let record = self.medical_scribe_jobs.get_mut(name)?;
        record.advance_clock(now);
        Some(record)
    }

    /// Materialise every clock-owed Medical Scribe job transition, then the stored
    /// jobs newest first — the order `ListMedicalScribeJobs` documents ("Jobs are
    /// ordered by creation date, with the newest job first").
    pub fn medical_scribe_jobs_newest_first(
        &mut self,
        now: SystemTime,
    ) -> Vec<&MedicalScribeJobRecord> {
        for record in self.medical_scribe_jobs.values_mut() {
            record.advance_clock(now);
        }
        let mut records: Vec<&MedicalScribeJobRecord> = self.medical_scribe_jobs.values().collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.sequence));
        records
    }

    /// The Call Analytics job named `name`, with every clock-owed transition of it
    /// already materialised, or `None` when this account has started no such job.
    ///
    /// The single lookup for this family, so every operation that reads a Call
    /// Analytics job sees the same status: the one the clock already owed it.
    pub fn call_analytics_job_mut(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut CallAnalyticsJobRecord> {
        let record = self.call_analytics_jobs.get_mut(name)?;
        record.advance_clock(now);
        Some(record)
    }

    /// Materialise every clock-owed Call Analytics job transition, then the stored
    /// jobs newest first — the order `ListCallAnalyticsJobs` documents ("Jobs are
    /// ordered by creation date, with the newest job first").
    pub fn call_analytics_jobs_newest_first(
        &mut self,
        now: SystemTime,
    ) -> Vec<&CallAnalyticsJobRecord> {
        for record in self.call_analytics_jobs.values_mut() {
            record.advance_clock(now);
        }
        let mut records: Vec<&CallAnalyticsJobRecord> = self.call_analytics_jobs.values().collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.sequence));
        records
    }

    /// The custom vocabulary named `name`, with every clock-owed transition of it
    /// already materialised, or `None` when this account holds no such vocabulary.
    pub fn vocabulary_mut(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut VocabularyRecord> {
        let record = self.vocabularies.get_mut(name)?;
        record.advance_clock(now);
        Some(record)
    }

    /// Materialise every clock-owed vocabulary transition, then the stored
    /// vocabularies newest first.
    ///
    /// Newest first for the same reason the job listing is: a listing that put the
    /// oldest first would page a caller through history before reaching what it just
    /// created.
    pub fn vocabularies_newest_first(&mut self, now: SystemTime) -> Vec<&VocabularyRecord> {
        for record in self.vocabularies.values_mut() {
            record.advance_clock(now);
        }
        let mut records: Vec<&VocabularyRecord> = self.vocabularies.values().collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.sequence));
        records
    }

    /// The custom MEDICAL vocabulary named `name`, with every clock-owed transition
    /// of it already materialised, or `None` when this account holds no such medical
    /// vocabulary.
    pub fn medical_vocabulary_mut(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut MedicalVocabularyRecord> {
        let record = self.medical_vocabularies.get_mut(name)?;
        record.advance_clock(now);
        Some(record)
    }

    /// Materialise every clock-owed medical-vocabulary transition, then the stored
    /// medical vocabularies newest first — the order `ListMedicalVocabularies`
    /// documents ("Custom vocabularies are ordered by creation date, with the newest
    /// vocabulary first").
    pub fn medical_vocabularies_newest_first(
        &mut self,
        now: SystemTime,
    ) -> Vec<&MedicalVocabularyRecord> {
        for record in self.medical_vocabularies.values_mut() {
            record.advance_clock(now);
        }
        let mut records: Vec<&MedicalVocabularyRecord> =
            self.medical_vocabularies.values().collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.sequence));
        records
    }

    /// The stored custom vocabulary filters, newest first. A filter has no
    /// `VocabularyState` in the model, so none of them has a clock-owed transition.
    pub fn vocabulary_filters_newest_first(&self) -> Vec<&VocabularyFilterRecord> {
        let mut records: Vec<&VocabularyFilterRecord> =
            self.vocabulary_filters.values().collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.sequence));
        records
    }

    /// The custom language model named `name`, with every clock-owed transition of it
    /// already materialised, or `None` when this account holds no such model.
    pub fn language_model_mut(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut LanguageModelRecord> {
        let record = self.language_models.get_mut(name)?;
        record.advance_clock(now);
        Some(record)
    }

    /// Materialise every clock-owed language-model transition, then the stored custom
    /// language models newest first — the order `ListLanguageModels` documents
    /// ("Language models are ordered by creation date, with the newest model first").
    pub fn language_models_newest_first(
        &mut self,
        now: SystemTime,
    ) -> Vec<&LanguageModelRecord> {
        for record in self.language_models.values_mut() {
            record.advance_clock(now);
        }
        let mut records: Vec<&LanguageModelRecord> = self.language_models.values().collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.sequence));
        records
    }

    /// Whether this account holds a resource of `family` named `name`.
    ///
    /// Reads the same per-family lookup [`Self::tags`] does, so "this account holds
    /// it" cannot mean one thing to a tagging call and another to a job start. Borrows
    /// the name rather than building a [`ResourceRef`], so a start checking several
    /// references allocates nothing.
    pub fn holds(&self, family: ResourceFamily, name: &str) -> bool {
        self.tags_of(family, name).is_some()
    }

    /// The refusal for the first resource `references` names that this account does
    /// not hold, or `None` when every named one is held.
    ///
    /// A job's settings may NAME a custom vocabulary, custom vocabulary filter or
    /// custom language model, and a name no create of that family ever recorded names
    /// nothing this emulator could apply — so a start that carries one is refused with
    /// the family's own not-found text ([`not_found_message`]) rather than stored on a
    /// job that would report the dangling name back. Each reference is the family the
    /// member's target resource belongs to and the name the request put there, or
    /// `None` where the request left the member out; the families are distinct stores,
    /// so a resource of one family never stands in for another's.
    pub fn unheld_reference_error(
        &self,
        references: &[(ResourceFamily, Option<&str>)],
    ) -> Option<String> {
        references.iter().find_map(|(family, name)| {
            let name = (*name)?;
            (!self.holds(*family, name)).then(|| not_found_message(family.kind()))
        })
    }

    /// The tag set of the resource `resource` names, or `None` when this account holds
    /// no such resource.
    ///
    /// The read-only half of [`Self::tags_mut`], so a reporting operation
    /// (`ListTagsForResource`) takes no mutable borrow for a read. The two must agree
    /// about which ARN names which stored record, so both match on the same resource
    /// types.
    pub fn tags(&self, resource: &ResourceRef) -> Option<&Vec<Tag>> {
        let family = ResourceFamily::from_resource_type(&resource.resource_type)?;
        self.tags_of(family, &resource.name)
    }

    /// The tag set of the resource `family` names `name`, or `None` when this account
    /// holds no such resource.
    ///
    /// The lookup [`Self::tags`] and [`Self::holds`] both read, by family and borrowed
    /// name: one place decides which family's store answers for which resource.
    pub fn tags_of(&self, family: ResourceFamily, name: &str) -> Option<&Vec<Tag>> {
        match family {
            ResourceFamily::TranscriptionJob => {
                self.transcription_jobs.get(name).map(|record| &record.tags)
            }
            ResourceFamily::MedicalTranscriptionJob => self
                .medical_transcription_jobs
                .get(name)
                .map(|record| &record.tags),
            ResourceFamily::Vocabulary => self.vocabularies.get(name).map(|record| &record.tags),
            ResourceFamily::MedicalVocabulary => self
                .medical_vocabularies
                .get(name)
                .map(|record| &record.tags),
            ResourceFamily::VocabularyFilter => self
                .vocabulary_filters
                .get(name)
                .map(|record| &record.tags),
            ResourceFamily::LanguageModel => {
                self.language_models.get(name).map(|record| &record.tags)
            }
            ResourceFamily::CallAnalyticsCategory => self
                .call_analytics_categories
                .get(name)
                .map(|record| &record.tags),
            ResourceFamily::MedicalScribeJob => {
                self.medical_scribe_jobs.get(name).map(|record| &record.tags)
            }
        }
    }

    /// The tag set of the resource `resource` names, for reading and writing, or
    /// `None` when this account holds no such resource.
    ///
    /// One lookup for the two tagging operations that WRITE, so `TagResource` and
    /// `UntagResource` cannot disagree about which ARN names which stored record. It
    /// reaches EVERY family this emulator holds records of — each of their creates
    /// accepts `Tags`, so each of their records carries a tag set the standard tagging
    /// API owns. An ARN whose `<type>` segment names no family this emulator stores, or
    /// whose `<name>` names no stored record of that family, answers `None`, which the
    /// caller renders as the modelled not-found error — never as an empty tag set.
    pub fn tags_mut(&mut self, resource: &ResourceRef) -> Option<&mut Vec<Tag>> {
        match ResourceFamily::from_resource_type(&resource.resource_type)? {
            ResourceFamily::TranscriptionJob => self
                .transcription_jobs
                .get_mut(&resource.name)
                .map(|record| &mut record.tags),
            ResourceFamily::MedicalTranscriptionJob => self
                .medical_transcription_jobs
                .get_mut(&resource.name)
                .map(|record| &mut record.tags),
            ResourceFamily::Vocabulary => self
                .vocabularies
                .get_mut(&resource.name)
                .map(|record| &mut record.tags),
            ResourceFamily::MedicalVocabulary => self
                .medical_vocabularies
                .get_mut(&resource.name)
                .map(|record| &mut record.tags),
            ResourceFamily::VocabularyFilter => self
                .vocabulary_filters
                .get_mut(&resource.name)
                .map(|record| &mut record.tags),
            ResourceFamily::LanguageModel => self
                .language_models
                .get_mut(&resource.name)
                .map(|record| &mut record.tags),
            ResourceFamily::CallAnalyticsCategory => self
                .call_analytics_categories
                .get_mut(&resource.name)
                .map(|record| &mut record.tags),
            ResourceFamily::MedicalScribeJob => self
                .medical_scribe_jobs
                .get_mut(&resource.name)
                .map(|record| &mut record.tags),
        }
    }
}

/// The resource an Amazon Transcribe ARN names: its type and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRef {
    /// The ARN's resource type, e.g. [`RESOURCE_TYPE_VOCABULARY`].
    pub resource_type: String,
    /// The resource's name, which is also its key in the store that holds it.
    pub name: String,
}

/// The resource `arn` names, or `None` when it is not an Amazon Transcribe resource
/// ARN.
///
/// The form is the one `TranscribeArn`'s own pattern pins —
/// `arn:<partition>:transcribe:<region>:<account>:<type>/<name>`
/// (`smithy_query(query="operation", operation="TagResource")`) — so an ARN of
/// another service, or a string that merely has six `:`-separated fields, names
/// nothing. The constraint trait has already refused anything outside that pattern
/// before a handler runs; parsing here is what turns the matched ARN into a lookup.
pub fn parse_resource_arn(arn: &str) -> Option<ResourceRef> {
    let fields: Vec<&str> = arn.split(':').collect();
    let [prefix, _partition, service, _region, _account, resource] = fields.as_slice() else {
        return None;
    };
    if !prefix.eq_ignore_ascii_case("arn") || !service.eq_ignore_ascii_case("transcribe") {
        return None;
    }
    let (resource_type, name) = resource.split_once('/')?;
    if resource_type.is_empty() || name.is_empty() {
        return None;
    }
    Some(ResourceRef {
        resource_type: resource_type.to_string(),
        name: name.to_string(),
    })
}

/// The ARN of one Amazon Transcribe resource, built through the framework's own ARN
/// helper so the account segment is the canonical emulator account.
pub fn resource_arn(resource_type: &str, name: &str, region: &str) -> String {
    ::engine::emulator_arn("transcribe", region, &format!("{resource_type}/{name}"))
}

/// One stored custom vocabulary: where its terms come from, plus the lifecycle facts
/// the service itself owns (its processing state, why that state is what it is, and
/// when it was last modified).
///
/// The two content members `CreateVocabulary` publishes are alternatives — the terms
/// in `Phrases`, or the file at `VocabularyFileUri` — and BOTH are recorded here: the
/// resource is the same control-plane record either way, and which alternative the
/// request chose is what decides the one transition the clock owes it (see
/// [`Self::advance_clock`]).
#[derive(Debug, Clone)]
pub struct VocabularyRecord {
    /// `VocabularyName`, also this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor `ListVocabularies` pages
    /// with.
    pub sequence: u64,
    /// `LanguageCode`: the one language this vocabulary's terms are in.
    pub language_code: String,
    /// The terms this vocabulary holds, as the request listed them in `Phrases` —
    /// EMPTY for one whose terms would have come from a file, which nothing loaded.
    pub phrases: Vec<String>,
    /// `VocabularyFileUri` when the request named a file as the source of its terms,
    /// and `None` when it listed them inline. Held so the failure reason can name it
    /// and an update can replace it; never fetched.
    pub file_uri: Option<String>,
    /// `VocabularyState`: [`VOCABULARY_STATE_PENDING`], [`VOCABULARY_STATE_READY`]
    /// or [`VOCABULARY_STATE_FAILED`].
    pub state: String,
    /// `FailureReason`, set only on a `FAILED` vocabulary.
    pub failure_reason: Option<String>,
    /// When the build's own transition falls due — `PENDING` -> `READY` for inline
    /// terms, `PENDING` -> `FAILED` for a file this emulator does not read; `None`
    /// once it has been materialised. A `PENDING` record always carries it.
    pub transition_due: Option<SystemTime>,
    pub last_modified_time: SystemTime,
    /// The region the request that created or last updated it was signed for; the
    /// service-managed `DownloadUri` is in that region.
    pub region: String,
    pub data_access_role_arn: Option<String>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub tags: Vec<Tag>,
}

impl VocabularyRecord {
    /// Materialise the state transition the CLOCK already owes this vocabulary: it
    /// was due at its stored instant whether or not anybody called. A reporting
    /// operation (`Get`, `List`) runs this before projecting the record, and that is
    /// the only write it performs.
    ///
    /// WHICH state it lands in is decided by where the terms were to come from: a
    /// vocabulary built from the terms the request listed is `READY`, while one whose
    /// terms would have had to be READ from a file is `FAILED` — that read is work
    /// this emulator cannot perform, so nothing built the vocabulary and nothing may
    /// report it as usable. The medical family's single transition is the same one
    /// ([`MedicalVocabularyRecord::advance_clock`]).
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.state != VOCABULARY_STATE_PENDING {
            return;
        }
        if let Some(due) = self.transition_due.filter(|due| now >= *due) {
            match &self.file_uri {
                Some(file_uri) => {
                    self.state = VOCABULARY_STATE_FAILED.to_string();
                    self.failure_reason = Some(file_not_read_failure_reason(
                        KIND_VOCABULARY,
                        file_uri,
                    ));
                }
                None => {
                    self.state = VOCABULARY_STATE_READY.to_string();
                    self.failure_reason = None;
                }
            }
            // The resource changed when the build finished, so the modification
            // instant is that of the transition, not of the request that started it.
            self.last_modified_time = due;
            self.transition_due = None;
        }
    }

    /// This record as the modelled `VocabularyInfo` shape (`ListVocabularies`).
    pub fn to_info(&self) -> VocabularyInfo {
        VocabularyInfo {
            vocabulary_name: Some(self.name.clone()),
            language_code: Some(self.language_code.clone()),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            vocabulary_state: Some(self.state.clone()),
        }
    }

    /// Where this vocabulary's own file is reported to live, for one this service
    /// built from the terms the request listed — and `None` for one whose terms would
    /// have come from a file, which nothing read: a vocabulary no terms were loaded
    /// for has no file to download, exactly as in the medical family
    /// ([`MedicalVocabularyRecord::download_uri`]). See [`vocabulary_download_uri`].
    pub fn download_uri(&self) -> Option<String> {
        self.file_uri
            .is_none()
            .then(|| vocabulary_download_uri(RESOURCE_TYPE_VOCABULARY, &self.name, &self.region))
    }
}

/// One stored custom MEDICAL vocabulary: the file the request named as the source of
/// its terms, plus the lifecycle facts the service itself owns (its processing state,
/// why that state is what it is, and when it was last modified).
///
/// WHY THIS FAMILY NEVER REACHES `READY`: a custom medical vocabulary can only be
/// built by READING the table file at `VocabularyFileUri`, which
/// `CreateMedicalVocabulary` and `UpdateMedicalVocabulary` both model `@required`
/// with no inline alternative (`smithy_query(query="operation",
/// operation="CreateMedicalVocabulary")`) — unlike `CreateVocabulary`, which also
/// takes the terms in `Phrases`. This emulator reads no object store and makes no
/// outbound request, so that build is work it cannot perform. The RECORD is still
/// real and is kept — the name, the language, the state, the timestamps and the tag
/// set are the control plane every operation of this family reports — and the build
/// it cannot run ends in the service's own modelled failure state: `PENDING` at the
/// create, then `FAILED` with a `FailureReason` that says the file was not read
/// ([`file_not_read_failure_reason`]). Nothing here reports a `READY` medical
/// vocabulary, because nothing ever read its terms; a NON-medical vocabulary whose
/// request named a file instead of listing its terms is recorded the same way and
/// ends the same way ([`VocabularyRecord::advance_clock`]).
#[derive(Debug, Clone)]
pub struct MedicalVocabularyRecord {
    /// `VocabularyName`, also this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor
    /// `ListMedicalVocabularies` pages with.
    pub sequence: u64,
    /// `LanguageCode`: the one language this vocabulary's terms are in.
    pub language_code: String,
    /// `VocabularyFileUri`: where the terms would be read from. Held so the failure
    /// reason can name it and an update can replace it; never fetched.
    pub file_uri: String,
    /// `VocabularyState`: [`VOCABULARY_STATE_PENDING`] until the clock makes the
    /// build due, then [`VOCABULARY_STATE_FAILED`].
    pub state: String,
    /// `FailureReason`, set only on a `FAILED` vocabulary.
    pub failure_reason: Option<String>,
    /// When `PENDING` -> `FAILED` falls due; `None` once it has been materialised.
    pub failed_due: Option<SystemTime>,
    pub last_modified_time: SystemTime,
    /// The region the request that created or last updated it was signed for.
    pub region: String,
    pub tags: Vec<Tag>,
}

impl MedicalVocabularyRecord {
    /// The record a create of `name` from the file at `file_uri` stores: `PENDING`,
    /// with its one clock-owed transition already due at a fixed span from `now`.
    ///
    /// Single-homed so the create and any later update cannot disagree about the
    /// invariant that a `PENDING` record always carries the instant its transition
    /// falls due.
    pub fn pending(
        name: String,
        sequence: u64,
        language_code: String,
        file_uri: String,
        region: String,
        tags: Vec<Tag>,
        now: SystemTime,
    ) -> Self {
        Self {
            name,
            sequence,
            language_code,
            file_uri,
            state: VOCABULARY_STATE_PENDING.to_string(),
            failure_reason: None,
            failed_due: Some(now + VOCABULARY_TRANSITION_DELAY),
            last_modified_time: now,
            region,
            tags,
        }
    }

    /// Materialise the state transition the CLOCK already owes this medical
    /// vocabulary: it was due at its stored instant whether or not anybody called. A
    /// reporting operation (`Get`, `List`) runs this before projecting the record,
    /// and that is the only write it performs.
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.state != VOCABULARY_STATE_PENDING {
            return;
        }
        if let Some(due) = self.failed_due.filter(|due| now >= *due) {
            self.state = VOCABULARY_STATE_FAILED.to_string();
            self.failure_reason = Some(file_not_read_failure_reason(
                KIND_MEDICAL_VOCABULARY,
                &self.file_uri,
            ));
            // The resource changed when the build ended, so the modification instant
            // is that of the transition, not of the request that started it.
            self.last_modified_time = due;
            self.failed_due = None;
        }
    }

    /// This record as the modelled `VocabularyInfo` shape
    /// (`ListMedicalVocabularies`), the same projection the non-medical family uses:
    /// the two listings report the same shape.
    pub fn to_info(&self) -> VocabularyInfo {
        VocabularyInfo {
            vocabulary_name: Some(self.name.clone()),
            language_code: Some(self.language_code.clone()),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            vocabulary_state: Some(self.state.clone()),
        }
    }

    /// Where this medical vocabulary's own file is reported to live, for one this
    /// service built — and `None` for one it did not, which is every record of this
    /// family (see the type's own docs): a vocabulary whose terms were never loaded
    /// has no file to download. See [`vocabulary_download_uri`].
    pub fn download_uri(&self) -> Option<String> {
        (self.state == VOCABULARY_STATE_READY).then(|| {
            vocabulary_download_uri(RESOURCE_TYPE_MEDICAL_VOCABULARY, &self.name, &self.region)
        })
    }
}

/// Why a custom vocabulary this emulator recorded ends `FAILED`: the file its terms
/// would have come from is one this emulator does not read (see
/// [`VocabularyRecord::advance_clock`] and [`MedicalVocabularyRecord`]).
///
/// The wording is this emulator's own — AWS publishes no text for a custom
/// vocabulary's `FailureReason` — and it names the URI the request gave, so a caller
/// can see which file was not read. One text for both families, worded by the
/// resource `kind` ([`KIND_VOCABULARY`], [`KIND_MEDICAL_VOCABULARY`]), because the
/// reason they fail is the same one.
pub fn file_not_read_failure_reason(kind: &str, file_uri: &str) -> String {
    format!(
        "The VocabularyFileUri {file_uri} was not read: this emulator reads no object \
         store and makes no outbound request, so it cannot build a custom {kind} from \
         a file. No terms were loaded for this vocabulary."
    )
}

/// One stored custom language model: the base model, language and training-data
/// location a create named, plus the lifecycle facts the service itself owns (its
/// training status and its timestamps).
///
/// WHAT IS REAL HERE: the record. A custom language model's training and tuning data
/// can only come from the Amazon S3 location at `InputDataConfig$S3Uri`, which
/// `CreateLanguageModel` models `@required` and for which it publishes no inline
/// alternative ("The location of your training and tuning files (this must be an
/// Amazon S3 URI)" — the operation's own doc). Reading that location is work this
/// emulator cannot perform, so the resource is recorded — its name, language, base
/// model, input data config, status, timestamps and tag set are the control plane
/// every operation of this family reports — and the training it cannot run ends in
/// the service's own modelled failure state: [`MODEL_STATUS_IN_PROGRESS`] at the
/// create, then [`MODEL_STATUS_FAILED`] with a `FailureReason` that says the data was
/// not read ([`language_model_failure_reason`]) once the clock makes that transition
/// due. Nothing here reports a `COMPLETED` model, because nothing trained one; this
/// is the custom MEDICAL vocabulary's case again ([`MedicalVocabularyRecord`]).
#[derive(Debug, Clone)]
pub struct LanguageModelRecord {
    /// `ModelName`, also this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor a listing of this family
    /// would page with.
    pub sequence: u64,
    /// `LanguageCode`: the one language this model's terms are in.
    pub language_code: String,
    /// `BaseModelName`: the standard Amazon Transcribe model this one was built on.
    pub base_model_name: String,
    /// `InputDataConfig`: where the training (and optionally tuning) data would be
    /// read from, and the role that would read it. Held so the failure reason can
    /// name it and every reader can report it; never fetched.
    pub input_data_config: InputDataConfig,
    /// `EncryptionConfiguration`, as the create named it, or `None`.
    pub encryption_configuration: Option<EncryptionConfiguration>,
    /// `ModelStatus`: [`MODEL_STATUS_IN_PROGRESS`] until the clock makes the training
    /// due, then [`MODEL_STATUS_FAILED`].
    pub status: String,
    /// `FailureReason`, set only on a `FAILED` model.
    pub failure_reason: Option<String>,
    /// When `IN_PROGRESS` -> `FAILED` falls due; `None` once it has been materialised.
    pub failed_due: Option<SystemTime>,
    /// `CreateTime`: when the create that stored this record was served.
    pub create_time: SystemTime,
    /// `LastModifiedTime`: when the record last changed.
    pub last_modified_time: SystemTime,
    pub tags: Vec<Tag>,
}

impl LanguageModelRecord {
    /// The record a create of `name` stores: `IN_PROGRESS`, with its one clock-owed
    /// transition already due at a fixed span from `now`.
    ///
    /// Single-homed so the create and any later update cannot disagree about the
    /// invariant that an `IN_PROGRESS` record always carries the instant its
    /// transition falls due.
    #[allow(clippy::too_many_arguments)]
    pub fn in_progress(
        name: String,
        sequence: u64,
        language_code: String,
        base_model_name: String,
        input_data_config: InputDataConfig,
        encryption_configuration: Option<EncryptionConfiguration>,
        tags: Vec<Tag>,
        now: SystemTime,
    ) -> Self {
        Self {
            name,
            sequence,
            language_code,
            base_model_name,
            input_data_config,
            encryption_configuration,
            status: MODEL_STATUS_IN_PROGRESS.to_string(),
            failure_reason: None,
            failed_due: Some(now + MODEL_TRANSITION_DELAY),
            create_time: now,
            last_modified_time: now,
            tags,
        }
    }

    /// Materialise the status transition the CLOCK already owes this model: it was due
    /// at its stored instant whether or not anybody called. A reporting operation
    /// (`DescribeLanguageModel`, a listing) runs this before projecting the record,
    /// and that is the only write it performs.
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.status != MODEL_STATUS_IN_PROGRESS {
            return;
        }
        if let Some(due) = self.failed_due.filter(|due| now >= *due) {
            self.status = MODEL_STATUS_FAILED.to_string();
            self.failure_reason = Some(language_model_failure_reason(
                &self.input_data_config.s3_uri,
            ));
            // The resource changed when the training ended, so the modification
            // instant is that of the transition, not of the request that started it.
            self.last_modified_time = due;
            self.failed_due = None;
        }
    }

    /// This record as the modelled `LanguageModel` shape, which
    /// `DescribeLanguageModel` and `ListLanguageModels` both report.
    pub fn to_language_model(&self) -> LanguageModel {
        LanguageModel {
            model_name: Some(self.name.clone()),
            create_time: Some(to_date_time(self.create_time)),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            language_code: Some(self.language_code.clone()),
            base_model_name: Some(self.base_model_name.clone()),
            model_status: Some(self.status.clone()),
            // "If `false`, your custom language model is using the most up-to-date
            // base model" (model doc for `LanguageModel$UpgradeAvailability`). This
            // emulator publishes exactly the two base models the `BaseModelName` enum
            // names and never supersedes one, so the base model a create named is
            // still the most current one for this model.
            upgrade_availability: Some(false),
            failure_reason: self.failure_reason.clone(),
            input_data_config: Some(self.input_data_config.clone()),
            encryption_configuration: self.encryption_configuration.clone(),
        }
    }
}

/// Why a custom language model this emulator recorded ends `FAILED`: the Amazon S3
/// location its training data would have come from is one this emulator does not read
/// (see [`LanguageModelRecord`]).
///
/// The wording is this emulator's own — AWS publishes no text for a custom language
/// model's `FailureReason` — and it names the URI the request gave, so a caller can
/// see which location was not read.
pub fn language_model_failure_reason(s3_uri: &str) -> String {
    format!(
        "The InputDataConfig S3Uri {s3_uri} was not read: this emulator reads no object \
         store and makes no outbound request, so it cannot train a custom language \
         model from data held there. No model was trained."
    )
}

/// One stored custom vocabulary filter: where its words come from and when it was
/// last modified.
///
/// A filter has no `VocabularyState` anywhere in the model — neither
/// `CreateVocabularyFilter`'s output nor `VocabularyFilterInfo` carries one
/// (`smithy_query(query="operation", operation="CreateVocabularyFilter")`) — so it
/// is usable from the moment it is created and has no clock-owed transition.
///
/// The two content members `CreateVocabularyFilter` and `UpdateVocabularyFilter`
/// publish are alternatives — the words in `Words`, or the file at
/// `VocabularyFilterFileUri` — and BOTH are recorded here, exactly as in the
/// vocabulary family ([`VocabularyRecord`]): the resource is the same control-plane
/// record either way. Which alternative the request chose decides only what this
/// record can report: a filter whose words would have had to be READ from a file has
/// none loaded and no file of its own to download ([`Self::download_uri`]), because
/// this emulator reads no object store and makes no outbound request.
#[derive(Debug, Clone)]
pub struct VocabularyFilterRecord {
    /// `VocabularyFilterName`, also this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor `ListVocabularyFilters`
    /// pages with.
    pub sequence: u64,
    pub language_code: String,
    /// The words this filter masks, deletes or flags, as the request listed them in
    /// `Words` — EMPTY for one whose words would have come from a file, which nothing
    /// loaded.
    pub words: Vec<String>,
    /// `VocabularyFilterFileUri` when the request named a file as the source of its
    /// words, and `None` when it listed them inline. Held so an update can replace it
    /// and so this record knows it holds no words of its own; never fetched.
    pub file_uri: Option<String>,
    pub last_modified_time: SystemTime,
    /// The region the request that created or last updated it was signed for.
    pub region: String,
    pub data_access_role_arn: Option<String>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub tags: Vec<Tag>,
}

impl VocabularyFilterRecord {
    /// This record as the modelled `VocabularyFilterInfo` shape
    /// (`ListVocabularyFilters`).
    pub fn to_info(&self) -> VocabularyFilterInfo {
        VocabularyFilterInfo {
            vocabulary_filter_name: Some(self.name.clone()),
            language_code: Some(self.language_code.clone()),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
        }
    }

    /// Where this filter's own file is reported to live, for one built from the words
    /// the request listed — and `None` for one whose words would have come from a
    /// file, which nothing read: a filter no words were loaded for has no file to
    /// download, exactly as in the vocabulary family
    /// ([`VocabularyRecord::download_uri`]). See [`vocabulary_download_uri`].
    pub fn download_uri(&self) -> Option<String> {
        self.file_uri.is_none().then(|| {
            vocabulary_download_uri(RESOURCE_TYPE_VOCABULARY_FILTER, &self.name, &self.region)
        })
    }
}

/// One stored Call Analytics category: the rules and input type the request asked
/// for, plus the two lifecycle facts the service itself owns (when it was created and
/// when it was last updated).
///
/// A category has no status member in the model
/// (`smithy_query(query="shape", shape="CategoryProperties")`), so it is usable from
/// the moment it is created and has no clock-owed transition. WHAT IT EMULATES is the
/// category RECORD: nothing here evaluates a rule against a call, because this
/// emulator transcribes no audio (see the module header) — a stored category is what a
/// Call Analytics job would be matched against, never a match.
#[derive(Debug, Clone)]
pub struct CallAnalyticsCategoryRecord {
    /// `CategoryName`, also this record's key in the store.
    pub name: String,
    /// The rules that define the category, as the request listed them. `RuleList` is
    /// modelled `length {min 1, max 20}`, so the boundary validator has already
    /// refused an empty or over-long list before a handler stores one.
    pub rules: Vec<Rule>,
    /// `POST_CALL` or `REAL_TIME` — [`DEFAULT_INPUT_TYPE`] when the create named
    /// neither.
    pub input_type: String,
    /// When this category was created; never changed afterwards.
    pub create_time: SystemTime,
    /// When this category's rules were last written: its creation instant until an
    /// update rewrites them.
    pub last_update_time: SystemTime,
    /// The tags this category carries, empty when it carries none.
    pub tags: Vec<Tag>,
}

impl CallAnalyticsCategoryRecord {
    /// This record as the modelled `CategoryProperties` shape, which is the whole
    /// answer of every operation that reports a category.
    ///
    /// `Tags` is left ABSENT for a category carrying none rather than reported as an
    /// empty list: `TagList` is modelled `length {min 1, max 200}`, so an empty list
    /// is not a value of that shape.
    pub fn to_properties(&self) -> CategoryProperties {
        CategoryProperties {
            category_name: Some(self.name.clone()),
            rules: Some(self.rules.clone()),
            create_time: Some(to_date_time(self.create_time)),
            last_update_time: Some(to_date_time(self.last_update_time)),
            tags: (!self.tags.is_empty()).then(|| self.tags.clone()),
            input_type: Some(self.input_type.clone()),
        }
    }
}

/// The most content one custom vocabulary or vocabulary filter holds, in bytes.
///
/// Amazon Transcribe publishes a 50 KB ceiling on the size of a custom vocabulary (its
/// table, however it was supplied) and on a custom vocabulary filter, so a request
/// carrying more than that is refused rather than stored. With [`MAX_VOCABULARIES`]
/// that is what bounds the terms this store can hold: at most 100 vocabularies and 100
/// filters, each of at most this many bytes of terms.
pub const MAX_VOCABULARY_CONTENT_BYTES: usize = 50 * 1024;

/// How many bytes of content `terms` carries, as [`MAX_VOCABULARY_CONTENT_BYTES`]
/// measures it.
pub fn content_bytes(terms: &[String]) -> usize {
    terms.iter().map(String::len).sum()
}

/// The `NextToken` a listing hands out to resume after the record with this creation
/// sequence and name.
///
/// The token is opaque to clients: this service writes `{sequence}|{name}` and reads
/// back the sequence ([`parse_page_cursor`]). The name travels with it, which is what
/// makes a token readable in a trace; only the sequence is interpreted, so a record
/// deleted between pages does not stop the next page from resuming strictly past it.
/// Single-homed so every listing of this service pages the same way.
pub fn page_cursor(sequence: u64, name: &str) -> String {
    format!("{sequence}|{name}")
}

/// The creation sequence a `NextToken` resumes after, or `None` for a token this
/// service never handed out — which a listing refuses rather than silently restarting
/// from the top.
///
/// Creation sequences start at 1, so a cursor of 0 is one this service never wrote.
pub fn parse_page_cursor(token: &str) -> Option<u64> {
    token
        .split('|')
        .next()
        .and_then(|sequence| sequence.parse::<u64>().ok())
        .filter(|sequence| *sequence > 0)
}

/// The prefix a name-keyed page cursor carries, which is what tells a token this
/// service handed out from one it never wrote. See [`name_page_cursor`].
const NAME_CURSOR_PREFIX: &str = "name|";

/// The `NextToken` a listing of records keyed by NAME ALONE hands out to resume
/// strictly after `name`.
///
/// The Call Analytics categories are stored without a creation sequence — their
/// record holds no such member (see [`CallAnalyticsCategoryRecord`]) and their listing
/// is documented with no order — so `ListCallAnalyticsCategories` pages over the
/// store's own key order (the name, ascending) and its cursor is a name rather than a
/// sequence ([`page_cursor`]). Resuming strictly past a NAME is what makes paging
/// stable under concurrent writes: a category created or deleted between two pages
/// cannot shift the position the next page resumes from. Single-homed here beside the
/// sequence cursor so both token formats of this service live in one place.
pub fn name_page_cursor(name: &str) -> String {
    format!("{NAME_CURSOR_PREFIX}{name}")
}

/// The record name a `NextToken` resumes after, or `None` for a token this service
/// never handed out — which a listing refuses rather than silently restarting from the
/// top.
///
/// Every name this service keys a record by is non-empty (`CategoryName` is modelled
/// `@length(min: 1, max: 200)`), so a bare prefix is a token this service never wrote.
pub fn parse_name_page_cursor(token: &str) -> Option<&str> {
    token
        .strip_prefix(NAME_CURSOR_PREFIX)
        .filter(|name| !name.is_empty())
}

/// What a request's two content members say about where a vocabulary's terms come
/// from: a list in the request, or a file the service would have to read.
///
/// `CreateVocabulary`/`UpdateVocabulary` (`Phrases` / `VocabularyFileUri`) and
/// `CreateVocabularyFilter`/`UpdateVocabularyFilter` (`Words` /
/// `VocabularyFilterFileUri`) publish the same pair of alternatives, so the rule
/// that reads them is single-homed here while each handler words its own refusal
/// with its own member names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentSource<'a> {
    /// The terms are listed in the request.
    Inline(&'a [String]),
    /// The terms are in the file at this URI, which the service would have to read.
    File(&'a str),
    /// Neither member carries content.
    Neither,
    /// Both members carry content, which the two operations document as an error.
    Both,
}

/// Which of the two alternatives a request chose.
///
/// An EMPTY list counts as no content: the member's own model constraint allows it
/// (`Phrases` has no minimum length), and a vocabulary with no terms is not a
/// vocabulary — so `Phrases: []` is the `Neither` case, not an empty inline build.
pub fn content_source<'a>(
    terms: Option<&'a [String]>,
    file_uri: Option<&'a str>,
) -> ContentSource<'a> {
    let terms = terms.filter(|terms| !terms.is_empty());
    let file_uri = file_uri.map(str::trim).filter(|uri| !uri.is_empty());
    match (terms, file_uri) {
        (Some(terms), None) => ContentSource::Inline(terms),
        (None, Some(uri)) => ContentSource::File(uri),
        (None, None) => ContentSource::Neither,
        (Some(_), Some(_)) => ContentSource::Both,
    }
}

/// The `DownloadUri` of a custom vocabulary or vocabulary filter: the service-managed
/// S3 location its file is reported to live at.
///
/// Reported, not served — exactly as a completed transcription job's
/// `Transcript.TranscriptFileUri` is (see this module's header): the terms themselves
/// live in this emulator's own state, where `GetVocabulary` and `GetVocabularyFilter`
/// read them from, and nothing writes an object at this URI or answers a request for
/// it. The form mirrors [`output_uri`]'s service-managed branch, so both URIs this
/// service reports name the same service-managed bucket.
pub fn vocabulary_download_uri(resource_type: &str, name: &str, region: &str) -> String {
    format!(
        "https://s3.{region}.amazonaws.com/aws-transcribe-{region}-prod/{account}/\
         {resource_type}/{name}",
        account = ::engine::emulator_account_id(),
    )
}

/// One stored transcription job: exactly what the request asked for, plus the
/// lifecycle facts the service itself owns (status, timestamps, transcript location).
#[derive(Debug, Clone)]
pub struct TranscriptionJobRecord {
    /// `TranscriptionJobName`, also this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor `ListTranscriptionJobs`
    /// pages with.
    pub sequence: u64,
    /// `TranscriptionJobStatus`: one of [`STATUS_QUEUED`], [`STATUS_IN_PROGRESS`],
    /// [`STATUS_COMPLETED`].
    pub status: String,
    pub language_code: Option<String>,
    pub media: Media,
    pub media_format: Option<String>,
    pub media_sample_rate_hertz: Option<i32>,
    pub output_bucket_name: Option<String>,
    pub output_key: Option<String>,
    /// `OutputLocationType`, decided once at start from `OutputBucketName`.
    pub output_location_type: String,
    /// The region the start request was signed for; the service-managed transcript
    /// URI is in that region.
    pub region: String,
    pub settings: Option<Settings>,
    pub model_settings: Option<ModelSettings>,
    pub job_execution_settings: Option<JobExecutionSettings>,
    pub content_redaction: Option<ContentRedaction>,
    pub identify_language: Option<bool>,
    pub identify_multiple_languages: Option<bool>,
    pub language_options: Option<Vec<String>>,
    pub language_id_settings: Option<std::collections::HashMap<String, LanguageIdSettings>>,
    pub toxicity_detection: Option<Vec<ToxicityDetectionSettings>>,
    /// The subtitle request, echoed back as `SubtitlesOutput`; its
    /// `SubtitleFileUris` are filled in when the job completes.
    pub subtitles: Option<SubtitlesOutput>,
    /// Where the transcript was written; `None` until the job completes.
    pub transcript: Option<Transcript>,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    /// When the job began processing; `None` while it is still `QUEUED`.
    pub start_time: Option<SystemTime>,
    pub completion_time: Option<SystemTime>,
    /// When `QUEUED` -> `IN_PROGRESS` falls due; `None` for a job that started
    /// processing immediately.
    pub queued_until: Option<SystemTime>,
    /// When `IN_PROGRESS` -> `COMPLETED` falls due.
    pub completion_due: SystemTime,
    pub tags: Vec<Tag>,
}

impl TranscriptionJobRecord {
    /// Materialise the status transitions the CLOCK already owes this job: each one
    /// was due at its stored instant whether or not anybody called. A reporting
    /// operation (`Get`, `List`) runs this before projecting the record, and that
    /// is the only write it performs.
    ///
    /// Completing a job records WHERE its output would be written (see the module
    /// header); it does not write anything there.
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.status == STATUS_QUEUED {
            match self.queued_until {
                Some(due) if now >= due => {
                    self.status = STATUS_IN_PROGRESS.to_string();
                    self.start_time = Some(due);
                }
                _ => return,
            }
        }
        if self.status == STATUS_IN_PROGRESS && now >= self.completion_due {
            self.status = STATUS_COMPLETED.to_string();
            self.completion_time = Some(self.completion_due);
            self.transcript = Some(Transcript {
                transcript_file_uri: Some(self.output_uri("json")),
                redacted_transcript_file_uri: None,
            });
            // The subtitle files a completed job produced sit beside the
            // transcript, one per requested format.
            let subtitle_uris: Option<Vec<String>> = self.subtitles.as_ref().map(|subtitles| {
                subtitles
                    .formats
                    .iter()
                    .flatten()
                    .map(|format| self.output_uri(format))
                    .collect()
            });
            if let (Some(subtitles), Some(uris)) = (self.subtitles.as_mut(), subtitle_uris) {
                subtitles.subtitle_file_uris = Some(uris);
            }
        }
    }

    /// The URI of one of this job's output files.
    fn output_uri(&self, extension: &str) -> String {
        output_uri(
            self.output_bucket_name.as_deref(),
            self.output_key.as_deref(),
            &self.name,
            &self.region,
            extension,
        )
    }

    /// This record as the modelled `TranscriptionJob` shape.
    pub fn to_job(&self) -> TranscriptionJob {
        TranscriptionJob {
            transcription_job_name: Some(self.name.clone()),
            transcription_job_status: Some(self.status.clone()),
            language_code: self.language_code.clone(),
            media_sample_rate_hertz: self.media_sample_rate_hertz,
            media_format: self.media_format.clone(),
            media: Some(self.media.clone()),
            transcript: self.transcript.clone(),
            start_time: self.start_time.map(to_date_time),
            creation_time: Some(to_date_time(self.creation_time)),
            completion_time: self.completion_time.map(to_date_time),
            failure_reason: self.failure_reason.clone(),
            settings: self.settings.clone(),
            model_settings: self.model_settings.clone(),
            job_execution_settings: self.job_execution_settings.clone(),
            content_redaction: self.content_redaction.clone(),
            identify_language: self.identify_language,
            identify_multiple_languages: self.identify_multiple_languages,
            language_options: self.language_options.clone(),
            // Language identification is not performed by this emulator, so the
            // score and the per-language breakdown it would produce stay absent
            // rather than being invented.
            identified_language_score: None,
            language_codes: None,
            tags: (!self.tags.is_empty()).then(|| self.tags.clone()),
            subtitles: self.subtitles.clone(),
            language_id_settings: self.language_id_settings.clone(),
            toxicity_detection: self.toxicity_detection.clone(),
        }
    }

    /// This record as the modelled `TranscriptionJobSummary` shape.
    pub fn to_summary(&self) -> TranscriptionJobSummary {
        TranscriptionJobSummary {
            transcription_job_name: Some(self.name.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            start_time: self.start_time.map(to_date_time),
            completion_time: self.completion_time.map(to_date_time),
            language_code: self.language_code.clone(),
            transcription_job_status: Some(self.status.clone()),
            failure_reason: self.failure_reason.clone(),
            output_location_type: Some(self.output_location_type.clone()),
            content_redaction: self.content_redaction.clone(),
            model_settings: self.model_settings.clone(),
            identify_language: self.identify_language,
            identify_multiple_languages: self.identify_multiple_languages,
            identified_language_score: None,
            language_codes: None,
            toxicity_detection: self.toxicity_detection.clone(),
        }
    }
}

/// One stored MEDICAL transcription job (Amazon Transcribe Medical): exactly what the
/// request asked for, plus the lifecycle facts the service itself owns (status,
/// timestamps and the location its transcript would be written to).
///
/// NOTHING IS TRANSCRIBED, exactly as for the non-medical family (see this module's
/// header): the `MedicalTranscript.TranscriptFileUri` a completed job reports names a
/// location nothing writes to or serves, and no PHI is identified. The job RECORD is
/// what this emulates.
///
/// `StartMedicalTranscriptionJob` has no deferred-execution member of any kind
/// (`smithy_query(query="operation", operation="StartMedicalTranscriptionJob")` lists
/// no `JobExecutionSettings`), so a job of this family is processing from the moment
/// it is started and has exactly one clock-owed transition, `IN_PROGRESS` ->
/// `COMPLETED`.
#[derive(Debug, Clone)]
pub struct MedicalTranscriptionJobRecord {
    /// `MedicalTranscriptionJobName`, also this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor a listing of this family
    /// would page with.
    pub sequence: u64,
    /// `TranscriptionJobStatus`: [`STATUS_IN_PROGRESS`] until the clock owes it
    /// [`STATUS_COMPLETED`]. The enum's other two values are modelled but unreached —
    /// nothing queues such a job and nothing this emulator does can fail one.
    pub status: String,
    /// `LanguageCode`, a REQUIRED member the operation narrows to one value: "US
    /// English (en-US) is the only valid value for medical transcription jobs"
    /// ([`MEDICAL_LANGUAGE_CODE`]).
    pub language_code: String,
    pub media: Media,
    pub media_format: Option<String>,
    pub media_sample_rate_hertz: Option<i32>,
    /// `OutputBucketName`, a REQUIRED member of the start request: the bucket the
    /// reported transcript URI names, so this family has no service-managed branch as
    /// [`output_uri`] has.
    pub output_bucket_name: String,
    pub output_key: Option<String>,
    /// `OutputEncryptionKMSKeyId`: the key the transcript would be encrypted with.
    /// Recorded as asked for; nothing encrypts anything, because nothing is written.
    pub output_encryption_kms_key_id: Option<String>,
    /// `KMSEncryptionContext`, recorded for the same reason as
    /// [`Self::output_encryption_kms_key_id`].
    pub kms_encryption_context: Option<std::collections::HashMap<String, String>>,
    pub settings: Option<MedicalTranscriptionSetting>,
    pub content_identification_type: Option<String>,
    pub specialty: String,
    /// `Type`: whether the media is a `CONVERSATION` or a `DICTATION`. Named
    /// `job_type` here because `type` is a Rust keyword.
    pub job_type: String,
    /// Where the transcript was written; `None` until the job completes.
    pub transcript: Option<MedicalTranscript>,
    /// `FailureReason`, set only on a `FAILED` job — which none of this emulator's are.
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub start_time: Option<SystemTime>,
    pub completion_time: Option<SystemTime>,
    /// When `IN_PROGRESS` -> `COMPLETED` falls due.
    pub completion_due: SystemTime,
    pub tags: Vec<Tag>,
}

impl MedicalTranscriptionJobRecord {
    /// Materialise the status transition the CLOCK already owes this job: it was due
    /// at the stored instant whether or not anybody called. A reporting operation
    /// (`Get`) runs this before projecting the record, and that is the only write it
    /// performs.
    ///
    /// Completing a job records WHERE its transcript would be written (see the module
    /// header); it does not write anything there.
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.status != STATUS_IN_PROGRESS || now < self.completion_due {
            return;
        }
        self.status = STATUS_COMPLETED.to_string();
        self.completion_time = Some(self.completion_due);
        self.transcript = Some(MedicalTranscript {
            // The same `OutputKey` rule the non-medical family applies, which this
            // operation documents in the same words — so one implementation
            // ([`output_uri`]) answers for both. `OutputBucketName` is REQUIRED for
            // this family, so the transcript is always in the caller's bucket and the
            // `region` argument (which only the service-managed branch reads) cannot
            // reach the URI.
            transcript_file_uri: Some(output_uri(
                Some(&self.output_bucket_name),
                self.output_key.as_deref(),
                &self.name,
                DEFAULT_REGION,
                "json",
            )),
        });
    }

    /// This record as the modelled `MedicalTranscriptionJob` shape, which is the whole
    /// answer of `StartMedicalTranscriptionJob` and `GetMedicalTranscriptionJob`.
    ///
    /// `Tags` is left ABSENT for a job carrying none rather than reported as an empty
    /// list, for the reason [`CallAnalyticsCategoryRecord::to_properties`] gives:
    /// `TagList` is modelled `length {min 1, max 200}`, so an empty list is not a
    /// value of that shape.
    pub fn to_job(&self) -> MedicalTranscriptionJob {
        MedicalTranscriptionJob {
            medical_transcription_job_name: Some(self.name.clone()),
            transcription_job_status: Some(self.status.clone()),
            language_code: Some(self.language_code.clone()),
            media_sample_rate_hertz: self.media_sample_rate_hertz,
            media_format: self.media_format.clone(),
            media: Some(self.media.clone()),
            transcript: self.transcript.clone(),
            start_time: self.start_time.map(to_date_time),
            creation_time: Some(to_date_time(self.creation_time)),
            completion_time: self.completion_time.map(to_date_time),
            failure_reason: self.failure_reason.clone(),
            settings: self.settings.clone(),
            content_identification_type: self.content_identification_type.clone(),
            specialty: Some(self.specialty.clone()),
            r#type: Some(self.job_type.clone()),
            tags: (!self.tags.is_empty()).then(|| self.tags.clone()),
        }
    }

    /// This record as the modelled `MedicalTranscriptionJobSummary` shape, which is
    /// what `ListMedicalTranscriptionJobs` reports one stored job as.
    ///
    /// `OutputLocationType` is always [`OUTPUT_LOCATION_CUSTOMER_BUCKET`] for this
    /// family: `OutputBucketName` is a REQUIRED member of the start request
    /// (`smithy_query(query="operation", operation="StartMedicalTranscriptionJob")`),
    /// so a job of this family never writes to a service-managed bucket. The summary
    /// carries no `Media`, `Transcript` or `Tags` member at all — those are read
    /// through `GetMedicalTranscriptionJob`.
    pub fn to_summary(&self) -> MedicalTranscriptionJobSummary {
        MedicalTranscriptionJobSummary {
            medical_transcription_job_name: Some(self.name.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            start_time: self.start_time.map(to_date_time),
            completion_time: self.completion_time.map(to_date_time),
            language_code: Some(self.language_code.clone()),
            transcription_job_status: Some(self.status.clone()),
            failure_reason: self.failure_reason.clone(),
            output_location_type: Some(OUTPUT_LOCATION_CUSTOMER_BUCKET.to_string()),
            specialty: Some(self.specialty.clone()),
            content_identification_type: self.content_identification_type.clone(),
            r#type: Some(self.job_type.clone()),
        }
    }
}

/// One stored Medical Scribe job (AWS HealthScribe): exactly what the request asked
/// for, plus the lifecycle facts the service itself owns (status, timestamps and the
/// location its output would be written to).
///
/// `StartMedicalScribeJob` has no deferred-execution member of any kind
/// (`smithy_query(query="operation", operation="StartMedicalScribeJob")`), so a job
/// of this family is processing from the moment it is started and has exactly one
/// clock-owed transition, `IN_PROGRESS` -> `COMPLETED`.
#[derive(Debug, Clone)]
pub struct MedicalScribeJobRecord {
    /// `MedicalScribeJobName`, also this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor a listing of this family
    /// would page with.
    pub sequence: u64,
    /// `MedicalScribeJobStatus`: [`STATUS_IN_PROGRESS`] until the clock owes it
    /// [`STATUS_COMPLETED`]. The enum's other two values are modelled but unreached —
    /// nothing queues such a job and nothing this emulator does can fail one.
    pub status: String,
    pub media: Media,
    /// `OutputBucketName`, a REQUIRED member of the start request: the bucket both
    /// reported output URIs name.
    pub output_bucket_name: String,
    /// `OutputEncryptionKMSKeyId`: the key the output would be encrypted with.
    /// Recorded as asked for; nothing encrypts anything, because nothing is written.
    pub output_encryption_kms_key_id: Option<String>,
    /// `KMSEncryptionContext`, recorded for the same reason as
    /// [`Self::output_encryption_kms_key_id`].
    pub kms_encryption_context: Option<std::collections::HashMap<String, String>>,
    pub data_access_role_arn: String,
    pub settings: MedicalScribeSettings,
    pub channel_definitions: Option<Vec<MedicalScribeChannelDefinition>>,
    /// Whether the start request carried a `MedicalScribeContext`, which is the ONLY
    /// thing the model reports about it (`MedicalScribeJob$MedicalScribeContextProvided`
    /// "Indicates whether the MedicalScribeContext object was provided"). The context
    /// itself is not kept: no operation of this service reports it, and its one member
    /// (`MedicalScribePatientContext$Pronouns`) is modelled `@sensitive`.
    pub context_provided: bool,
    /// Where the transcript and the clinical note would be written; `None` until the
    /// job completes. See [`medical_scribe_output_uri`].
    pub output: Option<MedicalScribeOutput>,
    /// `FailureReason`, set only on a `FAILED` job — which none of this emulator's are.
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub start_time: Option<SystemTime>,
    pub completion_time: Option<SystemTime>,
    /// When `IN_PROGRESS` -> `COMPLETED` falls due.
    pub completion_due: SystemTime,
    pub tags: Vec<Tag>,
}

impl MedicalScribeJobRecord {
    /// Materialise the status transition the CLOCK already owes this job: it was due
    /// at the stored instant whether or not anybody called. A reporting operation
    /// (`Get`) runs this before projecting the record, and that is the only write it
    /// performs.
    ///
    /// Completing a job records WHERE its output would be written (see the module
    /// header); it does not write anything there.
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.status != STATUS_IN_PROGRESS || now < self.completion_due {
            return;
        }
        self.status = STATUS_COMPLETED.to_string();
        self.completion_time = Some(self.completion_due);
        self.output = Some(MedicalScribeOutput {
            transcript_file_uri: medical_scribe_output_uri(
                &self.output_bucket_name,
                &self.name,
                MEDICAL_SCRIBE_TRANSCRIPT_FILE,
            ),
            clinical_document_uri: medical_scribe_output_uri(
                &self.output_bucket_name,
                &self.name,
                MEDICAL_SCRIBE_CLINICAL_DOCUMENT_FILE,
            ),
        });
    }

    /// This record as the modelled `MedicalScribeJob` shape, which is the whole answer
    /// of `StartMedicalScribeJob` and `GetMedicalScribeJob`.
    ///
    /// `Tags` is left ABSENT for a job carrying none rather than reported as an empty
    /// list, for the reason [`CallAnalyticsCategoryRecord::to_properties`] gives:
    /// `TagList` is modelled `length {min 1, max 200}`, so an empty list is not a
    /// value of that shape.
    pub fn to_job(&self) -> MedicalScribeJob {
        MedicalScribeJob {
            medical_scribe_job_name: Some(self.name.clone()),
            medical_scribe_job_status: Some(self.status.clone()),
            language_code: Some(MEDICAL_SCRIBE_LANGUAGE_CODE.to_string()),
            media: Some(self.media.clone()),
            medical_scribe_output: self.output.clone(),
            start_time: self.start_time.map(to_date_time),
            creation_time: Some(to_date_time(self.creation_time)),
            completion_time: self.completion_time.map(to_date_time),
            failure_reason: self.failure_reason.clone(),
            settings: Some(self.settings.clone()),
            data_access_role_arn: Some(self.data_access_role_arn.clone()),
            channel_definitions: self.channel_definitions.clone(),
            medical_scribe_context_provided: Some(self.context_provided),
            tags: (!self.tags.is_empty()).then(|| self.tags.clone()),
        }
    }

    /// This record as the modelled `MedicalScribeJobSummary` shape, which is what
    /// `ListMedicalScribeJobs` reports one stored job as.
    ///
    /// The summary is a strict projection of the same record `GetMedicalScribeJob`
    /// reports: it carries only the seven members the shape declares
    /// (`smithy_query(query="shape", shape="MedicalScribeJobSummary")`), so the
    /// media, settings, channel definitions, output locations and tags of a job are
    /// read through `GetMedicalScribeJob` rather than from a listing. `LanguageCode`
    /// is [`MEDICAL_SCRIBE_LANGUAGE_CODE`] for every job of this family, for the
    /// reason that constant gives.
    pub fn to_summary(&self) -> MedicalScribeJobSummary {
        MedicalScribeJobSummary {
            medical_scribe_job_name: Some(self.name.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            start_time: self.start_time.map(to_date_time),
            completion_time: self.completion_time.map(to_date_time),
            language_code: Some(MEDICAL_SCRIBE_LANGUAGE_CODE.to_string()),
            medical_scribe_job_status: Some(self.status.clone()),
            failure_reason: self.failure_reason.clone(),
        }
    }
}

/// `RedactionOutput` values, from the model's enum
/// (`smithy_query(query="shape", shape="RedactionOutput")`).
pub const REDACTION_OUTPUT_REDACTED: &str = REDACTION_OUTPUT_VALUES[0];
/// See [`REDACTION_OUTPUT_REDACTED`].
pub const REDACTION_OUTPUT_REDACTED_AND_UNREDACTED: &str = REDACTION_OUTPUT_VALUES[1];

/// Which of the two transcripts the modelled `Transcript` shape can report a finished
/// job produced, and so which of its two locations that job reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptOutputs {
    /// Whether an unredacted transcript was produced (`Transcript$TranscriptFileUri`).
    pub unredacted: bool,
    /// Whether a redacted one was (`Transcript$RedactedTranscriptFileUri`).
    pub redacted: bool,
}

/// What a job asked the service to produce, read from the two members that ask for
/// redaction.
///
/// `ContentRedaction$RedactionOutput` is a required member wherever
/// `ContentRedaction` is given, and it decides this: "When you choose `redacted`
/// Amazon Transcribe creates only a redacted transcript. When you choose
/// `redacted_and_unredacted` Amazon Transcribe creates a redacted and an unredacted
/// transcript (as two separate files)"
/// (`smithy_query(query="doc", shape="ContentRedaction", member="RedactionOutput")`).
/// `Media$RedactedMediaFileUri` asks for a redacted transcript by another route — it
/// "produces a redacted audio file in addition to a redacted transcript" — and
/// documents nothing about dropping the unredacted one, so a job that named it and no
/// `ContentRedaction` produces both.
///
/// A job that asked for neither produces the unredacted transcript alone, which is
/// why `RedactedTranscriptFileUri` is not reported for every job.
pub fn transcript_outputs(
    content_redaction: Option<&ContentRedaction>,
    redacts_media: bool,
) -> TranscriptOutputs {
    match content_redaction.map(|redaction| redaction.redaction_output.as_str()) {
        Some(REDACTION_OUTPUT_REDACTED) => TranscriptOutputs {
            unredacted: false,
            redacted: true,
        },
        Some(_) => TranscriptOutputs {
            unredacted: true,
            redacted: true,
        },
        None => TranscriptOutputs {
            unredacted: true,
            redacted: redacts_media,
        },
    }
}

/// What distinguishes the redacted transcript's location from the unredacted one's.
///
/// CHOSEN: the model documents the two as "two separate files" at two locations
/// (`Transcript$TranscriptFileUri` / `Transcript$RedactedTranscriptFileUri`) but names
/// no key for either, so the redacted one is the unredacted file name under this
/// prefix — in whatever folder or bucket the request directed the output to.
pub const REDACTED_OUTPUT_PREFIX: &str = "redacted-";

/// Where the REDACTED transcript of a job that asked for redaction is reported to
/// live, beside the unredacted one at `transcript_uri`.
///
/// Reported, not served, exactly as the unredacted location is (see this module's
/// header): nothing redacts a transcript nothing produced, so what this answers is
/// WHERE the redacted output would be — a location distinct from the unredacted one,
/// as the model's two separate members require.
pub fn redacted_transcript_uri(transcript_uri: &str) -> String {
    match transcript_uri.rsplit_once('/') {
        Some((parent, file)) => format!("{parent}/{REDACTED_OUTPUT_PREFIX}{file}"),
        None => format!("{REDACTED_OUTPUT_PREFIX}{transcript_uri}"),
    }
}

/// The `MediaFormat` of the audio a job was given, as the media URI it was given
/// evidences it — `None` for a URI that evidences no modelled format.
///
/// `MediaFormat` is "The format of the input media file"
/// (`smithy_query(query="shape", shape="CallAnalyticsJob")`) and no member of
/// `StartCallAnalyticsJob` carries it, so the service reports what it found in the
/// file. Nothing here opens that file, and the one thing the request says about it is
/// the name it is stored under: a URI whose last path segment ends in one of the
/// modelled `MediaFormat` extensions ([`MEDIA_FORMAT_VALUES`]) is reported as that
/// format — `Media`'s own documented examples are spelled that way
/// (`s3://DOC-EXAMPLE-BUCKET/my-media-file.flac`) — and a URI that ends in no such
/// extension leaves the member absent rather than guessed.
///
/// `MediaFileUri` is the input audio; a job that named only `RedactedMediaFileUri`
/// gave its input audio there instead, so that URI answers for it.
pub fn media_format_of(media: &Media) -> Option<String> {
    let uri = [
        media.media_file_uri.as_deref(),
        media.redacted_media_file_uri.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|uri| !uri.is_empty())?;
    // The path, without the query or fragment an http(s) URI may carry.
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    let extension = path.rsplit('/').next()?.rsplit_once('.')?.1.to_ascii_lowercase();
    MEDIA_FORMAT_VALUES
        .iter()
        .find(|format| **format == extension)
        .map(|format| (*format).to_string())
}

/// The `MediaSampleRateHertz` a finished Call Analytics job reports: "The sample rate,
/// in hertz, of the audio track in your input media file".
///
/// No member of `StartCallAnalyticsJob` carries it either, and unlike the format the
/// request says nothing at all about it, so this is a CHOSEN figure: the lowest rate
/// the member itself models (`@range(min: 8000, max: 48000)`), which is the rate the
/// telephone call audio this family analyses is sampled at. One value for every job of
/// the family, fixed when the job finishes, so what a job reports never changes
/// between reads.
pub const CALL_AUDIO_SAMPLE_RATE_HERTZ: i32 = 8_000;

/// The language a job reports when the request narrowed identification to nothing.
///
/// A CHOSEN value of the modelled `LanguageCode` enum
/// ([`crate::ops::types::LANGUAGE_CODE_VALUES`]), used only where
/// [`identified_language`] has no option list to read.
pub const DEFAULT_IDENTIFIED_LANGUAGE: &str = "en-US";

/// The `IdentifiedLanguageScore` that accompanies an identified language:
/// "Confidence scores are values between 0 and 1; a larger value indicates a higher
/// probability" (`smithy_query(query="doc", shape="CallAnalyticsJob", member="IdentifiedLanguageScore")`).
///
/// This emulator's identification is not probabilistic — it reads the language off the
/// request rather than weighing audio (see [`identified_language`]) — so what it
/// reports is the top of that documented range, the certainty of a determination that
/// had nothing to weigh, rather than an invented probability.
pub const IDENTIFIED_LANGUAGE_SCORE: f32 = 1.0;

/// The language a Call Analytics job was transcribed in, narrowed to the options the
/// request allowed.
///
/// `StartCallAnalyticsJob` carries NO language member
/// (`smithy_query(query="operation", operation="StartCallAnalyticsJob")`): Amazon
/// Transcribe identifies the language of the call itself, and
/// `CallAnalyticsJobSettings$LanguageOptions` only narrows what identification may
/// choose between ("You can specify two or more language codes that represent the
/// languages you think may be present in your media"). So the language is the
/// service's own determination, and a job that finished has one — "The language code
/// used to create your Call Analytics job"
/// (`smithy_query(query="shape", shape="CallAnalyticsJob")`) — however many options
/// the request listed, or none.
///
/// This emulator analyses no audio, so the determination is made from the request
/// instead of from the call: the FIRST code `LanguageOptions` lists, since
/// identification may only answer with one of them, and
/// [`DEFAULT_IDENTIFIED_LANGUAGE`] where the request narrowed the choice to nothing.
/// It is a function of the request alone, so every read of a job reports the same
/// language.
pub fn identified_language(language_options: Option<&[String]>) -> String {
    language_options
        .and_then(<[String]>::first)
        .cloned()
        .unwrap_or_else(|| DEFAULT_IDENTIFIED_LANGUAGE.to_string())
}

/// One stored Call Analytics job: exactly what the request asked for, plus the
/// lifecycle facts the service itself owns (status, timestamps and the location its
/// transcript would be written to).
///
/// `StartCallAnalyticsJob` has no deferred-execution member
/// (`smithy_query(query="operation", operation="StartCallAnalyticsJob")`) — the queue
/// it documents is entered only when an account "exceeds your Concurrent Job Limit",
/// which no member of the request opts into — so a job this emulator accepts is
/// processing from the moment it is started and has exactly one clock-owed
/// transition, `IN_PROGRESS` -> `COMPLETED`, like the Medical Scribe family
/// ([`MedicalScribeJobRecord`]); a start made while that concurrent limit is already
/// reached is refused instead ([`MAX_CONCURRENT_CALL_ANALYTICS_JOBS`]).
///
/// WHAT IT EMULATES is the job RECORD (see the module header): no call is
/// transcribed, no sentiment or call characteristic is derived and no category is
/// matched, so the `Transcript.TranscriptFileUri` a completed job reports names a
/// location nothing writes to or serves.
#[derive(Debug, Clone)]
pub struct CallAnalyticsJobRecord {
    /// `CallAnalyticsJobName`, also this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor `ListCallAnalyticsJobs`
    /// pages with.
    pub sequence: u64,
    /// `CallAnalyticsJobStatus`: [`STATUS_IN_PROGRESS`] until the clock owes it
    /// [`STATUS_COMPLETED`]. The enum's other two values are modelled but unreached —
    /// nothing queues such a job and nothing this emulator does can fail one.
    pub status: String,
    pub media: Media,
    /// `OutputLocation`, the one output member this operation takes; `None` when the
    /// request named none, in which case the transcript is reported in a
    /// service-managed location (see [`call_analytics_output_uri`]).
    pub output_location: Option<String>,
    /// `OutputEncryptionKMSKeyId`: the key the output would be encrypted with.
    /// Recorded as asked for; nothing encrypts anything, because nothing is written.
    pub output_encryption_kms_key_id: Option<String>,
    /// `DataAccessRoleArn`, "The Amazon Resource Name (ARN) you included in your
    /// request" (model doc for `CallAnalyticsJob$DataAccessRoleArn`) — optional on
    /// this operation, so `None` when the request named none.
    pub data_access_role_arn: Option<String>,
    /// The region the start request was signed for; the service-managed transcript
    /// URI is in that region.
    pub region: String,
    pub settings: Option<CallAnalyticsJobSettings>,
    pub channel_definitions: Option<Vec<ChannelDefinition>>,
    /// `LanguageCode`, the language the service identified for this call; `None`
    /// until the job completes, because identification is part of processing it
    /// ([`identified_language`]).
    pub language_code: Option<String>,
    /// `IdentifiedLanguageScore`, set with [`Self::language_code`]
    /// ([`IDENTIFIED_LANGUAGE_SCORE`]).
    pub identified_language_score: Option<f32>,
    /// `MediaFormat`, the format the service determined the input audio is in; `None`
    /// until the job completes, and for a media URI that evidences no modelled format
    /// ([`media_format_of`]).
    pub media_format: Option<String>,
    /// `MediaSampleRateHertz`, the rate the service determined the input audio is
    /// sampled at; `None` until the job completes
    /// ([`CALL_AUDIO_SAMPLE_RATE_HERTZ`]).
    pub media_sample_rate_hertz: Option<i32>,
    /// Where the transcript was written; `None` until the job completes.
    pub transcript: Option<Transcript>,
    /// `FailureReason`, set only on a `FAILED` job — which none of this emulator's
    /// are: nothing it does can fail a job it accepted.
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub start_time: Option<SystemTime>,
    pub completion_time: Option<SystemTime>,
    /// When `IN_PROGRESS` -> `COMPLETED` falls due.
    pub completion_due: SystemTime,
    pub tags: Vec<Tag>,
}

impl CallAnalyticsJobRecord {
    /// Materialise the status transition the CLOCK already owes this job: it was due
    /// at the stored instant whether or not anybody called. A reporting operation
    /// (`Get`, `List`) runs this before projecting the record, and that is the only
    /// write it performs.
    ///
    /// Completing a job records what the service determined while it processed the
    /// call — the language it identified, the format and sample rate of the audio it
    /// was given, and WHERE each transcript it produced would be written (see the
    /// module header) — and writes nothing at those locations. Every one of those
    /// values is a function of the stored record, so a job reports the same ones at
    /// every later read.
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.status != STATUS_IN_PROGRESS || now < self.completion_due {
            return;
        }
        self.status = STATUS_COMPLETED.to_string();
        self.completion_time = Some(self.completion_due);
        let settings = self.settings.as_ref();
        self.language_code = Some(identified_language(
            settings.and_then(|settings| settings.language_options.as_deref()),
        ));
        self.identified_language_score = Some(IDENTIFIED_LANGUAGE_SCORE);
        self.media_format = media_format_of(&self.media);
        self.media_sample_rate_hertz = Some(CALL_AUDIO_SAMPLE_RATE_HERTZ);
        let transcript_uri =
            call_analytics_output_uri(self.output_location.as_deref(), &self.name, &self.region);
        // Which transcripts the request asked the service to produce decides which
        // of the two locations the job reports ([`transcript_outputs`]).
        let outputs = transcript_outputs(
            settings.and_then(|settings| settings.content_redaction.as_ref()),
            self.media
                .redacted_media_file_uri
                .as_deref()
                .is_some_and(|uri| !uri.trim().is_empty()),
        );
        self.transcript = Some(Transcript {
            transcript_file_uri: outputs.unredacted.then(|| transcript_uri.clone()),
            redacted_transcript_file_uri: outputs
                .redacted
                .then(|| redacted_transcript_uri(&transcript_uri)),
        });
    }

    /// This record as the modelled `CallAnalyticsJob` shape, which is the whole answer
    /// of `StartCallAnalyticsJob` and `GetCallAnalyticsJob`.
    ///
    /// `Tags` is left ABSENT for a job carrying none rather than reported as an empty
    /// list, for the reason [`CallAnalyticsCategoryRecord::to_properties`] gives:
    /// `TagList` is modelled `length {min 1, max 200}`, so an empty list is not a
    /// value of that shape.
    pub fn to_job(&self) -> CallAnalyticsJob {
        CallAnalyticsJob {
            call_analytics_job_name: Some(self.name.clone()),
            call_analytics_job_status: Some(self.status.clone()),
            // Nothing here skips an analytics feature: the one feature the model
            // names a skip for is generative summarization, and both reason codes it
            // models turn on the CONTENT of the call, which nothing evaluates. A job
            // that skipped nothing reports no details at all, rather than an empty
            // list of skipped features.
            call_analytics_job_details: None,
            language_code: self.language_code.clone(),
            media_sample_rate_hertz: self.media_sample_rate_hertz,
            media_format: self.media_format.clone(),
            media: Some(self.media.clone()),
            transcript: self.transcript.clone(),
            start_time: self.start_time.map(to_date_time),
            creation_time: Some(to_date_time(self.creation_time)),
            completion_time: self.completion_time.map(to_date_time),
            failure_reason: self.failure_reason.clone(),
            data_access_role_arn: self.data_access_role_arn.clone(),
            identified_language_score: self.identified_language_score,
            settings: self.settings.clone(),
            channel_definitions: self.channel_definitions.clone(),
            tags: (!self.tags.is_empty()).then(|| self.tags.clone()),
        }
    }

    /// This record as the modelled `CallAnalyticsJobSummary` shape, which is what
    /// `ListCallAnalyticsJobs` reports for each result.
    ///
    /// `LanguageCode` is the same determination the job itself reports ("The language
    /// code used to create your Call Analytics transcription",
    /// `smithy_query(query="shape", shape="CallAnalyticsJobSummary")`), read off the
    /// same record rather than recomputed here.
    pub fn to_summary(&self) -> CallAnalyticsJobSummary {
        CallAnalyticsJobSummary {
            call_analytics_job_name: Some(self.name.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            start_time: self.start_time.map(to_date_time),
            completion_time: self.completion_time.map(to_date_time),
            language_code: self.language_code.clone(),
            call_analytics_job_status: Some(self.status.clone()),
            call_analytics_job_details: None,
            failure_reason: self.failure_reason.clone(),
        }
    }
}

/// Where a Call Analytics job's transcript is reported to live.
///
/// `StartCallAnalyticsJob` takes ONE `OutputLocation` URI rather than the
/// bucket-and-key pair a transcription job takes, and documents three forms —
/// `s3://BUCKET`, `s3://BUCKET/my-output-folder/` and
/// `s3://BUCKET/my-output-folder/my-call-analytics-job.json`: "Unless you specify a
/// file name (option 3), the name of your output file has a default value that
/// matches the name you specified for your transcription job using the
/// CallAnalyticsJobName parameter" (model doc for
/// `StartCallAnalyticsJobRequest$OutputLocation`). A request that names no output
/// location gets the service-managed location every job family reports
/// ([`output_uri`]).
///
/// Reported, not served: nothing writes an object there (see the module header).
///
/// [`output_object_key`] is NOT reused: that rule names an object KEY relative to a
/// separate `OutputBucketName`, while this member is a whole URI whose bucket is part
/// of the value, so the two share the documented wording but not an input.
pub fn call_analytics_output_uri(
    output_location: Option<&str>,
    job_name: &str,
    region: &str,
) -> String {
    let Some(location) = output_location else {
        return output_uri(None, None, job_name, region, CALL_ANALYTICS_OUTPUT_EXTENSION);
    };
    match location.strip_suffix(&format!(".{CALL_ANALYTICS_OUTPUT_EXTENSION}")) {
        // The third documented form: a location naming a file is used as given.
        Some(_) => location.to_string(),
        None => format!(
            "{folder}/{job_name}.{CALL_ANALYTICS_OUTPUT_EXTENSION}",
            folder = location.trim_end_matches('/')
        ),
    }
}

/// The file extension of a Call Analytics job's transcript, as the `OutputLocation`
/// doc's own example file name (`my-call-analytics-job.json`) spells it.
pub const CALL_ANALYTICS_OUTPUT_EXTENSION: &str = "json";

/// The documented `OutputKey` rule, as a free function so `StartTranscriptionJob`
/// and the completion path agree on one implementation.
///
/// From the model doc for `StartTranscriptionJobRequest$OutputKey`: "The default
/// name for your transcription output is the same as the name you specified for your
/// transcription job", `OutputKey` `my-transcript.json` yields
/// `my-transcript.json`, and `OutputKey` `my-transcript` with job
/// `my-first-transcription` yields `my-transcript/my-first-transcription.json`.
pub fn output_object_key(output_key: Option<&str>, job_name: &str, extension: &str) -> String {
    match output_key {
        None => format!("{job_name}.{extension}"),
        // A key naming a file is used as given — with this output's own
        // extension, so the subtitle files sit beside the transcript.
        Some(key) => match key.strip_suffix(".json") {
            Some(stem) => format!("{stem}.{extension}"),
            None => format!("{folder}/{job_name}.{extension}", folder = key.trim_end_matches('/')),
        },
    }
}

/// The URI of one output file of a job, in the caller's bucket when the request
/// named one and in a service-managed bucket otherwise (model doc for
/// `Transcript$TranscriptFileUri`).
pub fn output_uri(
    output_bucket_name: Option<&str>,
    output_key: Option<&str>,
    job_name: &str,
    region: &str,
    extension: &str,
) -> String {
    let key = output_object_key(output_key, job_name, extension);
    match output_bucket_name {
        Some(bucket) => format!("s3://{bucket}/{key}"),
        None => format!(
            "https://s3.{region}.amazonaws.com/aws-transcribe-{region}-prod/{account}/{key}",
            account = ::engine::emulator_account_id(),
        ),
    }
}

/// A `SystemTime` as the `DateTime` the modelled timestamp members carry.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTranscribeTranscribeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTranscribeTranscribeState>)
}

/// The region the request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope or one that does not name a region.
///
/// The scope is caller-supplied and the region reaches the host part of a
/// service-managed transcript URI, so a value that is not shaped like a Region
/// identifier is not reflected: the real service would not have served the request
/// under a region it does not have.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_like(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is shaped like an AWS Region identifier: two or more
/// hyphen-separated lowercase-letter parts followed by a digit group
/// (`us-east-1`, `ap-southeast-3`, `us-gov-west-1`, `cn-north-1`).
fn is_region_like(region: &str) -> bool {
    let Some((parts, index)) = region.rsplit_once('-') else {
        return false;
    };
    region.len() <= 32
        && !index.is_empty()
        && index.bytes().all(|byte| byte.is_ascii_digit())
        && parts.split('-').count() >= 2
        && parts
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_lowercase()))
}

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
//! `IN_PROGRESS` -> `COMPLETED` (or `FAILED`) transition and its projection onto the
//! modelled `LanguageModel` shape — the Call Analytics jobs ([`CallAnalyticsJobRecord`]), each
//! a record of the media, output location and settings a start named, with its own
//! `IN_PROGRESS` -> `COMPLETED` transition and its projections onto `CallAnalyticsJob`
//! and `CallAnalyticsJobSummary` — and the Medical Scribe (AWS HealthScribe) jobs, each a
//! record of the media, settings and channel definitions it was started with, with
//! its own `IN_PROGRESS` -> `COMPLETED` transition and its projections onto
//! `MedicalScribeJob` and `MedicalScribeJobSummary`.
//! Several operations read each record, so
//! the projections are single-homed here rather than copied into each handler; the
//! three tagging operations reach every family's tag set through one lookup
//! ([`ComAmazonawsTranscribeTranscribeState::tags_mut`]), and every operation that
//! WRITES one — `TagResource` and each create/start whose input carries `Tags` — writes
//! it through one merge ([`merge_tags`]), so a key holds one value whichever path stored
//! it.
//!
//! EVERY family is REGION-SCOPED ([`RegionalStore`]): Amazon Transcribe is a regional
//! service, so a record belongs to the one Region whose endpoint created it and every
//! lookup, listing, uniqueness check and quota count is made in the Region the calling
//! request was signed for ([`request_region`]). That Region is caller-supplied and
//! unauthenticated, so a family's store is bounded by its per-Region ceiling TOGETHER
//! WITH [`MAX_REGIONS`], the cap on how many Regions hold records of it at all.
//!
//! WHAT THIS EMULATES, AND WHAT IT DOES NOT: the job RECORD — its name, its
//! settings, its status, its timestamps and the location its output is written to.
//! No audio is transcribed, and nothing invents transcript content: no
//! word, sentiment or summary of a call appears anywhere, because no engine produced
//! one. The LOCATION a finished job reports is nonetheless one this emulator SERVES:
//! the output document is stored at the reported
//! `Transcript.TranscriptFileUri` / `Transcript.RedactedTranscriptFileUri` /
//! `SubtitlesOutput.SubtitleFileUris`, in this emulator's own object store over the
//! in-process callback, so the canonical workflow — start a job, poll it until it is
//! finished, then fetch the location it reports — completes (see
//! [`store_job_outputs`]). What that document holds is the envelope the service itself
//! owns (the job, the account, the status) with an EMPTY transcript and no caption,
//! because no engine read the audio: a transcription that produced no words, never
//! invented speech ([`transcript_document`], [`subtitle_document`]).
//! A member the service itself DETERMINES is reported where the request states
//! what it is determined from, and withheld where only the media could say. The
//! language a job asked the service to IDENTIFY is of the first kind: `LanguageOptions`
//! restricts what identification may answer with, so a finished job reports the
//! identification it asked for — the single `LanguageCode` with its
//! `IdentifiedLanguageScore` ([`identified_language`]), or the per-language
//! `LanguageCodes` breakdown of a multi-language request, which is every code the
//! options allowed, because narrowing them to a subset would take the audio
//! ([`identified_languages`]) — and the Call Analytics family determines its language,
//! media format and sample rate the same way, from the request and this family's own
//! constants (see [`CallAnalyticsJobRecord`]). The FORMAT of the media a job processed
//! is of the first kind in EVERY family that reports one: the name the media is stored
//! under evidences it ([`media_format_of`]) and the request states that name, so a job
//! that FINISHES reports that determination wherever the request stated no format of
//! its own, and the format a request did state stands as the service accepted it — a
//! completed job's media cannot contradict it, because a job whose media does ends
//! `FAILED` instead ([`determined_media_format`], [`media_failure_reason`]).
//! `LanguageCodeItem.DurationInSeconds` is
//! of the second kind and stays absent: how long each language is SPOKEN is a
//! measurement of media nothing reads.
//! A custom vocabulary is NOT the same in one respect: its terms are held in this
//! state, where a `StartTranscriptionJob` that names the vocabulary would read them
//! from, and no operation of this service reports the terms themselves — none of their
//! output shapes has a member for them — but the `DownloadUri` it reports is a
//! location this emulator SERVES. The text file of the terms a create or an update
//! supplied is written to the service-managed object store that URI names, over the
//! in-process callback into this same emulator, so a client that fetches the reported
//! location reads back the terms it stored and a later update replaces them (see
//! [`store_vocabulary_content`] and [`vocabulary_download_uri`]). A vocabulary whose
//! terms come from a FILE instead of from the request is built from the very same
//! seam, in the other direction: the caller uploaded that file to Amazon S3 before
//! calling, this emulator holds an Amazon S3 of its own, so the file is READ from it in
//! process ([`resolve_vocabulary_file`]) and its lines are the terms the build
//! produces. Resolving that object is part of VALIDATING the request, so a request
//! whose object the store answers for with nothing there is refused outright and no
//! record is stored at all ([`URI_NAMES_NO_OBJECT`]). Past that, the build ends the way
//! the read decides: a file the store serves settles the record `READY`, with those
//! terms stored at the `DownloadUri` the reader reports, and a file nothing read — a
//! read the store refuses, no Amazon S3 to ask at all, an object holding no terms or no
//! text — ends the build in the service's own modelled `FAILED` state, with a
//! `FailureReason` grounded in that answer (see [`VocabularyRecord::advance_clock`] and
//! [`vocabulary_file_not_read_reason`]); the custom MEDICAL vocabulary family, whose
//! terms can ONLY come from such a file, is built exactly the same way (see
//! [`MedicalVocabularyRecord`]). The `DownloadUri` is where the BUILT vocabulary is
//! stored, so only a vocabulary whose build has FINISHED reports one: it is absent
//! while the service is still processing the terms (`PENDING`) and absent after a build
//! that failed, in both families alike ([`VocabularyRecord::download_uri`],
//! [`MedicalVocabularyRecord::download_uri`]). A vocabulary FILTER whose words come
//! from such a file is built from it the same way ([`VocabularyFilterRecord`]), and
//! that family models no processing state at all, so the only thing a filter whose
//! words nothing read does not report is its own `DownloadUri`, the file
//! this service would have built.
//! A Call Analytics category is the same kind of record: its
//! rules are held here, where a Call Analytics job that applies the category would
//! read them from, and nothing evaluates a rule against a call, because no audio is
//! analysed. A Call Analytics JOB is the job record again
//! ([`CallAnalyticsJobRecord`]): no call is transcribed, no call characteristic,
//! sentiment or summary is derived and no category is matched, so the
//! `Transcript.TranscriptFileUri` — and, for a job that asked for redaction, the
//! `RedactedTranscriptFileUri` beside it — a completed one reports names a location
//! the output document with none of that in it is stored at, like every other
//! family's ([`store_job_outputs`]). That family's start request carries no language,
//! media-format or sample-rate member at all, because the service DETERMINES each of
//! them, so a finished job of it reports the determination this emulator can make
//! without audio: the language the request narrowed identification to
//! ([`identified_language`]), the format the media URI it was given names
//! ([`media_format_of`]) and this family's own sample rate
//! ([`CALL_AUDIO_SAMPLE_RATE_HERTZ`]). A custom
//! language model is NOT that case: its training and tuning data come from the Amazon
//! S3 locations a create names, this emulator's own Amazon S3 answers for them in
//! process ([`resolve_training_data`]), and no operation of that family reports
//! anything derived from the data itself — so a model whose data the object store
//! answers for finishes `COMPLETED` and is usable, and only a location that store says
//! cannot be read ends a model `FAILED`, with the reason its answer grounds (see
//! [`LanguageModelRecord`] and [`TrainingDataVerdict`]). A Medical Scribe job is the job
//! RECORD too, and the strongest case of the same limit: no conversation is
//! transcribed, no speaker role is identified and no clinical note is generated, so
//! the `MedicalScribeOutput.TranscriptFileUri` / `ClinicalDocumentUri` a completed
//! job reports name locations the transcript document and a clinical note of no
//! sections are stored at (see [`medical_scribe_output_uri`] and
//! [`clinical_document`]), and a `FailureReason` stays absent because nothing
//! this emulator does can fail a job it accepted. A MEDICAL transcription job is the
//! transcription-job RECORD again, under its own name: no audio is transcribed and no
//! personal health information is identified, so the
//! `MedicalTranscript.TranscriptFileUri` a completed job reports names a location
//! holding that same contentless transcript (see [`MedicalTranscriptionJobRecord`]).
//!
//! The INPUT media is the other half of that: a job's audio is an object in Amazon S3
//! the caller uploaded before starting the job, and this emulator holds an Amazon S3 of
//! its own, so the location a start names is RESOLVED against it before any record is
//! stored ([`resolve_media_objects`]) — a start pointed at an object that store says is
//! not there is refused, and a job whose media the store does answer for is accepted
//! and then judged by what that media is ([`media_failure_reason`]): the one per-media
//! failure this emulator can establish without decoding audio is a `MediaFormat` the
//! stored object's own name contradicts, and a job of it ends in the service's own
//! modelled `FAILED` state with that `FailureReason`. With no Amazon S3 mounted to ask,
//! nothing is known about the media and nothing is refused or failed on an assumption.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AbsoluteTimeRange, CallAnalyticsJob, CallAnalyticsJobSettings, CallAnalyticsJobSummary,
    CategoryProperties, ChannelDefinition, ContentRedaction, EncryptionConfiguration,
    InputDataConfig, JobExecutionSettings, LanguageCodeItem, LanguageIdSettings, LanguageModel,
    Media, MedicalScribeChannelDefinition, MedicalScribeJob, MedicalScribeJobSummary,
    MedicalScribeOutput, MedicalScribeSettings, MedicalTranscript, MedicalTranscriptionJob,
    MedicalTranscriptionJobSummary, MedicalTranscriptionSetting, ModelSettings, RelativeTimeRange,
    Rule, Settings, SubtitlesOutput, Tag, ToxicityDetectionSettings, Transcript, TranscriptionJob,
    TranscriptionJobSummary, VocabularyFilterInfo, VocabularyInfo, MEDIA_FORMAT_VALUES,
    MEDICAL_SCRIBE_LANGUAGE_CODE_VALUES, REDACTION_OUTPUT_VALUES, SUBTITLE_FORMAT_VALUES,
};

/// `TranscriptionJobStatus` values, from the model's enum
/// (`smithy_query(query="shape", shape="TranscriptionJobStatus")`).
pub const STATUS_QUEUED: &str = "QUEUED";
/// See [`STATUS_QUEUED`].
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`STATUS_QUEUED`].
pub const STATUS_COMPLETED: &str = "COMPLETED";
/// See [`STATUS_QUEUED`]. Reached by a job the service found something wrong with in
/// the media it was pointed at — the one per-media failure this emulator can establish
/// (see [`media_failure_reason`]) — and by no other job, because nothing else it does
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

/// Whether a job in `status` is one the service is WORKING ON: it has neither finished
/// ([`is_terminal_job_status`]) nor is it merely waiting to be picked up
/// ([`STATUS_QUEUED`]).
///
/// This — not "has not finished" — is what a family's CONCURRENCY ceiling bounds, since
/// what that ceiling limits is how much work runs at once: the model says so from the
/// queueing side, where a request past the ceiling waits "until the number of jobs falls
/// below the concurrent request limit"
/// (`smithy_query(query="shape", shape="JobExecutionSettings")`), so a job still waiting
/// for that number to fall cannot itself be part of it. One predicate for every job
/// family, as [`is_terminal_job_status`] is: `QUEUED` is modelled by each of their status
/// enums, so none of them may count a waiting job as running.
pub fn is_processing_job_status(status: &str) -> bool {
    !is_terminal_job_status(status) && status != STATUS_QUEUED
}

/// The one lifecycle fact every stored record of this service holds whose status the
/// CLOCK moves on its own: the transitions that status still has to catch up with.
///
/// One trait rather than one spelling per family, so the rule "materialise what the
/// clock already owed, then report" is written once ([`record_mut`],
/// [`materialised_newest_first`], [`jobs_being_processed`]) over any family's store: the
/// records differ in what a resource IS, not in how a due transition is caught up
/// with. A family added later reaches those helpers only by implementing this, so it
/// cannot be served by a lookup or a listing that forgot the clock step.
pub trait ClockedRecord {
    /// Materialise every status transition the clock already owes this record — the
    /// record's own `advance_clock`.
    fn materialise_due_transitions(&mut self, now: SystemTime);
}

/// Give each family's record the [`ClockedRecord`] view of its own `advance_clock`.
macro_rules! clocked_records {
    ($($record:ident;)+) => {
        $(
            impl ClockedRecord for $record {
                fn materialise_due_transitions(&mut self, now: SystemTime) {
                    self.advance_clock(now);
                }
            }
        )+
    };
}

// Every family whose records the model gives a status or state the clock moves. A
// custom vocabulary FILTER has no `VocabularyState` in the model and a Call Analytics
// category no status at all, so neither family has a transition to materialise and
// neither appears here.
clocked_records! {
    TranscriptionJobRecord;
    MedicalTranscriptionJobRecord;
    MedicalScribeJobRecord;
    CallAnalyticsJobRecord;
    LanguageModelRecord;
    VocabularyRecord;
    MedicalVocabularyRecord;
}

/// The record `region` holds under `name`, with every transition the clock already
/// owed it materialised, or `None` when that Region holds none — whatever any other
/// Region holds under the same name.
///
/// THE SINGLE-RECORD LOOKUP of this service, written once over [`ClockedRecord`]
/// rather than once per family, so every operation that reads, updates or deletes a
/// resource of any family sees the same status: the one the clock already owed it.
/// The transitions are materialised FIRST, before the caller decides anything from
/// that status, so a resource whose transition was already due is reported — and
/// refused, or deleted — in the state it has reached rather than the one it had left.
pub fn record_mut<'store, R: ClockedRecord>(
    store: &'store mut RegionalStore<R>,
    region: &str,
    name: &str,
    now: SystemTime,
) -> Option<&'store mut R> {
    let record = store.get_mut(region, name)?;
    record.materialise_due_transitions(now);
    Some(record)
}

/// `region`'s records, newest first.
///
/// THE ORDERING of every sequence-paged listing this service serves ("ordered by
/// creation date, with the newest ... first", as each of their docs words it), written
/// once over the creation sequence [`ListedRecord`] gives every family's records: a
/// listing that put the oldest first would page a caller through history before
/// reaching what it just created. Only `region`'s records, because a listing is served
/// by one Region's endpoint and reports what that Region holds.
pub fn newest_first<'store, R: ListedRecord>(
    store: &'store RegionalStore<R>,
    region: &str,
) -> Vec<&'store R> {
    let mut records: Vec<&R> = store.values_in(region).collect();
    records.sort_by_key(|record| std::cmp::Reverse(record.sequence()));
    records
}

/// Materialise every clock-owed transition of the records `region` holds, then those
/// records [`newest_first`] — what a listing of a family whose records have a
/// transition at all answers with.
///
/// The two rules composed in one place for every such family: the status a listing
/// reports is the one the clock already owed each record, and the order is that
/// family's creation order reversed. A family whose records carry no transition
/// (a custom vocabulary filter) orders itself with [`newest_first`] alone.
pub fn materialised_newest_first<'store, R: ClockedRecord + ListedRecord>(
    store: &'store mut RegionalStore<R>,
    region: &str,
    now: SystemTime,
) -> Vec<&'store R> {
    for record in store.values_in_mut(region) {
        record.materialise_due_transitions(now);
    }
    newest_first(store, region)
}

/// How many of `region`'s `jobs` the service is actually WORKING ON at `now`: the
/// records whose status is one it processes in ([`is_processing_job_status`]), counted
/// after every clock-owed transition has been materialised, so a job the clock already
/// finished — or already took out of its family's queue — is counted as what it has
/// become.
///
/// This — not how many records the account holds, and not how many of them are only
/// waiting ([`jobs_waiting_in_queue`]) — is what a job family's concurrency ceiling
/// bounds (see [`MAX_CONCURRENT_TRANSCRIPTION_JOBS`]), so a finished job releases its
/// slot while staying listable and a job still waiting for a slot never held one. The
/// ceiling is published per account per REGION, so only the jobs of the Region the start
/// was signed for are counted.
///
/// One spelling for every job family, bounded by the two traits that already state
/// what it needs of a record — the clock-owed transitions ([`ClockedRecord`]) and the
/// status ([`ListedRecord`]) — rather than by a third trait of its own: the four job
/// records differ in what a job IS, not in how its status moves. A family whose model
/// gives its records no status at all is processing nothing.
pub fn jobs_being_processed<R: ClockedRecord + ListedRecord>(
    jobs: &mut RegionalStore<R>,
    region: &str,
    now: SystemTime,
) -> usize {
    let mut processing = 0;
    for record in jobs.values_in_mut(region) {
        record.materialise_due_transitions(now);
        if record.status().is_some_and(is_processing_job_status) {
            processing += 1;
        }
    }
    processing
}

/// How many of `region`'s `jobs` are only WAITING to be processed at `now`: the records
/// in [`STATUS_QUEUED`], counted after every clock-owed transition has been
/// materialised, so a job the clock has already taken out of the queue is not counted
/// as waiting.
///
/// A queued job is unfinished without occupying one of the slots a family's concurrency
/// ceiling bounds ([`jobs_being_processed`] does not count it), so for a family whose
/// start documents a queue it
/// is this count, not that ceiling, that caps how many records one caller can hold
/// unfinished at once (see [`MAX_QUEUED_TRANSCRIPTION_JOBS`] and
/// [`MAX_QUEUED_CALL_ANALYTICS_JOBS`]).
pub fn jobs_waiting_in_queue<R: ClockedRecord + ListedRecord>(
    jobs: &mut RegionalStore<R>,
    region: &str,
    now: SystemTime,
) -> usize {
    let mut waiting = 0;
    for record in jobs.values_in_mut(region) {
        record.materialise_due_transitions(now);
        if record.status() == Some(STATUS_QUEUED) {
            waiting += 1;
        }
    }
    waiting
}

/// What a start operation does with the request in front of it: run the job, park it
/// in its family's queue, or refuse it for the family's concurrency quota.
///
/// See [`admit_start`], which decides between the three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartAdmission {
    /// The family is below its ceiling, so the job is processing from the moment it is
    /// started.
    Process,
    /// The ceiling is reached and the request opted into the queue its family's start
    /// documents, so the job waits in [`STATUS_QUEUED`].
    Queue,
    /// The ceiling is reached with no queue for this request to enter, so the start
    /// answers the `LimitExceededException` its operation models for an exceeded quota.
    Refuse,
}

/// Whether a start made at `now` runs, queues or is refused, given `region`'s share of
/// this family's `jobs`.
///
/// The ceiling a start is refused for bounds the jobs of the family the service is
/// actually WORKING ON ([`jobs_being_processed`]) — never the ones only waiting in its
/// queue ([`jobs_waiting_in_queue`]), which hold no slot — so reaching it is not by
/// itself a refusal:
/// where a family's start models an opt-in into job queueing, a start carrying it is
/// placed in a queue until the number of jobs falls below that ceiling, and only a
/// start without it gets the quota error. `queue` is `Some(depth)` exactly when this
/// request opted in, carrying how many jobs that queue holds; `None` when it opted into
/// nothing, or when the family models no such member at all.
///
/// Below the ceiling there is nothing to wait for, so an opt-in queues nothing. A
/// waiting job holds no running slot, so the ceiling cannot cap how many of them one
/// caller accumulates: `depth` does, and a start that would make the queue deeper is
/// refused in the ceiling's own words (see [`MAX_QUEUED_TRANSCRIPTION_JOBS`]).
pub fn admit_start<R: ClockedRecord + ListedRecord>(
    jobs: &mut RegionalStore<R>,
    region: &str,
    now: SystemTime,
    ceiling: usize,
    queue: Option<usize>,
) -> StartAdmission {
    if jobs_being_processed(jobs, region, now) < ceiling {
        return StartAdmission::Process;
    }
    match queue {
        Some(depth) if jobs_waiting_in_queue(jobs, region, now) < depth => StartAdmission::Queue,
        _ => StartAdmission::Refuse,
    }
}

/// The lifecycle facts a job record starts life with, as [`start_lifecycle`] decides
/// them.
#[derive(Debug, Clone)]
pub struct StartLifecycle {
    /// The `*JobStatus` the job is in when the start answers.
    pub status: String,
    /// When the job began processing; `None` while it is still waiting.
    pub start_time: Option<SystemTime>,
    /// When `QUEUED` -> `IN_PROGRESS` falls due; `None` for a job that is processing
    /// already.
    pub queued_until: Option<SystemTime>,
    /// When `IN_PROGRESS` -> `COMPLETED` falls due.
    pub completion_due: SystemTime,
}

/// The status and the clock-owed instants a job accepted at `now` starts life with.
///
/// A job that went into its family's queue ([`StartAdmission::Queue`]) is not
/// processing yet: it has no start time, and the clock owes it `QUEUED` ->
/// `IN_PROGRESS` before the completion every job gets. One spelling for both families
/// whose start documents a queue, since what differs between them is what a job IS and
/// not how long it waits ([`JOB_TRANSITION_DELAY`]).
pub fn start_lifecycle(now: SystemTime, queued: bool) -> StartLifecycle {
    if queued {
        let starts_at = now + JOB_TRANSITION_DELAY;
        StartLifecycle {
            status: STATUS_QUEUED.to_string(),
            start_time: None,
            queued_until: Some(starts_at),
            completion_due: starts_at + JOB_TRANSITION_DELAY,
        }
    } else {
        StartLifecycle {
            status: STATUS_IN_PROGRESS.to_string(),
            start_time: Some(now),
            queued_until: None,
            completion_due: now + JOB_TRANSITION_DELAY,
        }
    }
}

/// The most job records of one family this emulator KEEPS for one Region.
///
/// Not a quota and not a refusal: a start is never refused for it. It is the bound on
/// this process's memory, since the quota above counts only the jobs being processed and
/// so
/// cannot cap how many records a caller that keeps starting jobs accumulates; together
/// with [`MAX_REGIONS`], which caps how many Regions hold job records at all, it bounds
/// a family's whole store. A
/// CHOSEN figure, several times the concurrency quota so recent history stays
/// readable and listable; the real service keeps a job record far longer than any
/// emulator run.
pub const MAX_RETAINED_JOB_RECORDS: usize = 500;

/// Keep a family's store for `region` at [`MAX_RETAINED_JOB_RECORDS`] records by
/// dropping the OLDEST ones the service has already finished with, as a start that adds
/// a record does.
///
/// Only `region`'s records are counted and only `region`'s records are dropped: the
/// store is Region-scoped, so one Region's traffic can never evict a job another Region
/// holds.
///
/// Only a terminal record is ever dropped, so no unfinished job and no job a caller
/// could still be polling to completion disappears; a record is dropped exactly as a
/// delete of that job would drop it. With the concurrency quota capping how many jobs
/// can be processed at once and, for a family whose start documents a queue, that
/// queue's own depth capping how many wait, they together bound a family's store for
/// one Region at
/// [`MAX_RETAINED_JOB_RECORDS`] plus those bounds.
pub fn evict_oldest_finished_jobs<R: ClockedRecord + ListedRecord>(
    jobs: &mut RegionalStore<R>,
    region: &str,
) {
    let held = jobs.len_in(region);
    if held <= MAX_RETAINED_JOB_RECORDS {
        return;
    }
    let excess = held - MAX_RETAINED_JOB_RECORDS;
    let mut finished: Vec<(u64, String)> = jobs
        .iter_in(region)
        .filter(|(_, record)| record.status().is_some_and(is_terminal_job_status))
        .map(|(name, record)| (record.sequence(), name.to_string()))
        .collect();
    finished.sort_unstable();
    for (_, name) in finished.into_iter().take(excess) {
        jobs.remove(region, &name);
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

/// The template an AWS HealthScribe job's clinical note summary is generated from when
/// the start asked for none: "The default is HISTORY_AND_PHYSICAL" (model doc for
/// `ClinicalNoteGenerationSettings$NoteTemplate`, also one of
/// [`crate::ops::types::MEDICAL_SCRIBE_NOTE_TEMPLATE_VALUES`]). The default is RECORDED
/// on the job, so every operation that reports its settings reports the same template
/// rather than each one re-defaulting — the same treatment [`DEFAULT_INPUT_TYPE`] gets.
pub const DEFAULT_NOTE_TEMPLATE: &str = "HISTORY_AND_PHYSICAL";

/// The `MedicalScribeSettings` a Medical Scribe job is RECORDED with: the ones the
/// start asked for, with the documented default of every member that states one filled
/// in — which for this shape is the clinical-note template ([`DEFAULT_NOTE_TEMPLATE`]).
///
/// The template is materialised even for a start that named no
/// `ClinicalNoteGenerationSettings` at all, because clinical note generation is not
/// opt-in for this family: the operation "generates clinical notes" for every job it
/// accepts (`smithy_query(query="operation", operation="StartMedicalScribeJob")`), so
/// a template always applies to a recorded job and the record says which one.
pub fn recorded_medical_scribe_settings(
    mut settings: MedicalScribeSettings,
) -> MedicalScribeSettings {
    let mut clinical_note = settings
        .clinical_note_generation_settings
        .unwrap_or_default();
    if clinical_note.note_template.is_none() {
        clinical_note.note_template = Some(DEFAULT_NOTE_TEMPLATE.to_string());
    }
    settings.clinical_note_generation_settings = Some(clinical_note);
    settings
}

/// The most Medical Scribe jobs this emulator runs AT ONCE for an account.
///
/// A CHOSEN figure, not a published one — AWS gives no per-account number for
/// HealthScribe jobs in the model — applying the same one the other families use. It
/// bounds the jobs BEING PROCESSED, like every other family's
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
/// Reported AND served — exactly as a transcription job's
/// `Transcript.TranscriptFileUri` is (see this module's header): the document a
/// finished job owes this location is stored at it ([`store_job_outputs`]), and it
/// holds no transcribed conversation and no generated note, because nothing read the
/// audio. `OutputBucketName` is REQUIRED for this family
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

/// The most transcription jobs this emulator runs AT ONCE for an account in one Region.
///
/// The quota Amazon Transcribe publishes for this family is a CONCURRENCY one — 100
/// concurrent batch transcription jobs per account per Region — and the
/// `LimitExceededException` a start past it answers documents its own remedy as
/// waiting rather than deleting ("You've either sent too many requests or your input
/// file is too long. Wait before retrying your request",
/// `smithy_query(query="shape", shape="LimitExceededException")`); the model says the
/// same from the other side, since `JobExecutionSettings$AllowDeferredExecution`
/// queues a request "until the number of jobs falls below the concurrent request
/// limit". So this figure bounds the jobs the service is actually working on
/// ([`jobs_being_processed`]), never the records an account holds and never the ones
/// only waiting for a slot: a job that reached a
/// terminal status releases its slot and stays readable and listable, which is what
/// makes `ListTranscriptionJobs`' documented `COMPLETED` status filter meaningful.
pub const MAX_CONCURRENT_TRANSCRIPTION_JOBS: usize = 100;

/// The family [`concurrency_limit_message`] names in a transcription start's refusal.
pub const FAMILY_TRANSCRIPTION_JOBS: &str = "transcription jobs";

/// The most transcription jobs this emulator keeps WAITING in the queue a start that
/// deferred its execution enters at the ceiling above, for one Region.
///
/// Not a published quota: `JobExecutionSettings$AllowDeferredExecution` describes the
/// queue without a depth ("transcription job requests are placed in a queue until the
/// number of jobs falls below the concurrent request limit"). It is the bound on this
/// process's memory, needed because a queued job is NOT terminal: the ceiling does not
/// count it against a running slot and [`evict_oldest_finished_jobs`] cannot drop it.
/// A CHOSEN figure, this family's own ceiling, exactly as the sibling family whose
/// start documents a queue is bounded ([`MAX_QUEUED_CALL_ANALYTICS_JOBS`]); a start
/// made while that many are already waiting is refused the `LimitExceededException`
/// the ceiling itself answers, in the same words ([`concurrency_limit_message`]),
/// whose documented remedy — waiting for a running job to finish — is exactly what
/// empties the queue.
pub const MAX_QUEUED_TRANSCRIPTION_JOBS: usize = MAX_CONCURRENT_TRANSCRIPTION_JOBS;

/// The most MEDICAL transcription jobs this emulator runs AT ONCE for an account.
///
/// Amazon Transcribe Medical answers the same `LimitExceededException` for an
/// exceeded quota, so this family is bounded the same way the non-medical one is
/// ([`MAX_CONCURRENT_TRANSCRIPTION_JOBS`]): by how many of its jobs are being processed,
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
/// the same figure the other families use bounds the jobs being processed
/// ([`MAX_CONCURRENT_TRANSCRIPTION_JOBS`]), and a terminal job releases its slot.
pub const MAX_CONCURRENT_CALL_ANALYTICS_JOBS: usize = MAX_CONCURRENT_TRANSCRIPTION_JOBS;

/// The family [`concurrency_limit_message`] names in a Call Analytics start's refusal.
pub const FAMILY_CALL_ANALYTICS_JOBS: &str = "Call Analytics jobs";

/// The most Call Analytics jobs this emulator keeps WAITING in the queue a start enters
/// past the ceiling above, for one Region.
///
/// Not a published quota: the queue this family's start documents is described without
/// a depth ("your job will automatically be added to a queue to be processed once your
/// concurrent job count is below the limit"). It is the bound on this process's memory,
/// and it is needed because a queued job is NOT terminal: the ceiling above does not
/// count it against a running slot and [`evict_oldest_finished_jobs`] cannot drop it,
/// so without this cap one caller could hold unboundedly many unfinished records at
/// once. A CHOSEN figure, the family's own ceiling, so the queue is never deeper than
/// the number of jobs it feeds and a Region holds at most twice that ceiling in
/// unfinished records; a start made while that many are already waiting is refused the
/// `LimitExceededException` the ceiling itself answers, in the same words
/// ([`concurrency_limit_message`]), whose documented remedy — waiting for a running job
/// to finish — is exactly what empties the queue.
pub const MAX_QUEUED_CALL_ANALYTICS_JOBS: usize = MAX_CONCURRENT_CALL_ANALYTICS_JOBS;

/// `VocabularyState` values, from the model's enum
/// (`smithy_query(query="shape", shape="VocabularyState")`).
pub const VOCABULARY_STATE_PENDING: &str = "PENDING";
/// See [`VOCABULARY_STATE_PENDING`].
pub const VOCABULARY_STATE_READY: &str = "READY";
/// See [`VOCABULARY_STATE_PENDING`]. Where a vocabulary whose terms were to be read
/// from a file nothing could read them out of ends up, in EITHER family: the record is
/// kept and the build carries a `FailureReason` grounded in what this emulator
/// established about the file ([`vocabulary_file_not_read_reason`]).
pub const VOCABULARY_STATE_FAILED: &str = "FAILED";

/// How long after the call that starts it a custom vocabulary's next state
/// transition is owed (`PENDING` -> `READY` for a build that loaded its terms,
/// `PENDING` -> `FAILED` for one whose file nothing read any out of).
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

/// How many distinct Regions one resource family keeps records for.
///
/// A per-Region quota alone would not bound a store: the Region keying it is the
/// request's own SigV4 credential-scope Region, which this emulator does not
/// authenticate, so without this cap a caller could open a fresh per-Region bucket —
/// each with its own quota — on every request. With it a family holds at most
/// `MAX_REGIONS` buckets, and so at most `MAX_REGIONS` times its own per-Region
/// ceiling ([`MAX_VOCABULARIES`], [`MAX_LANGUAGE_MODELS`],
/// [`MAX_CALL_ANALYTICS_CATEGORIES`], [`MAX_RETAINED_JOB_RECORDS`] plus that family's
/// concurrency quota, and for a family whose start queues the overflow the depth of
/// that queue too, [`MAX_QUEUED_TRANSCRIPTION_JOBS`] and
/// [`MAX_QUEUED_CALL_ANALYTICS_JOBS`]) in total. AWS operates on the
/// order of 36 commercial Regions, so
/// this figure is this emulator's own and comfortably above the real surface; a create
/// or start past it answers the same modelled `LimitExceededException` AWS answers when
/// a create would exceed an allowance ([`region_capacity_message`]).
pub const MAX_REGIONS: usize = 64;

/// The refusal a create or start carries when its Region would be an
/// `MAX_REGIONS + 1`-th one for `family`: the Region is not one this emulator holds
/// records of that family for, and it has no room for another.
///
/// One spelling for every family, as [`concurrency_limit_message`] is: the condition is
/// the same in each, and it is the emulator's own bound rather than a published quota,
/// so the text says so instead of naming an AWS allowance.
pub fn region_capacity_message(family: &str, region: &str) -> String {
    format!(
        "This emulator holds {family} for at most {MAX_REGIONS} Regions, and {region} \
         is not one of them. Delete the {family} of a Region you no longer need and try \
         your request again."
    )
}

/// The most custom vocabularies this emulator holds for an account IN ONE REGION.
///
/// Amazon Transcribe publishes a default quota of 100 custom vocabularies per account
/// per Region, and the same figure for custom medical vocabularies and for custom
/// vocabulary filters. Applying the published figure to each family, per Region, keeps
/// the store bounded TOGETHER WITH [`MAX_REGIONS`], which caps how many Regions hold
/// records at all: a caller that never deletes cannot grow it without end, and
/// `DeleteVocabulary` / `DeleteMedicalVocabulary` / `DeleteVocabularyFilter` is what
/// makes room again.
pub const MAX_VOCABULARIES: usize = 100;

/// The family [`region_capacity_message`] names in a custom vocabulary's Region
/// refusal.
pub const FAMILY_VOCABULARIES: &str = "custom vocabularies";
/// See [`FAMILY_VOCABULARIES`].
pub const FAMILY_MEDICAL_VOCABULARIES: &str = "custom medical vocabularies";
/// See [`FAMILY_VOCABULARIES`].
pub const FAMILY_VOCABULARY_FILTERS: &str = "custom vocabulary filters";
/// See [`FAMILY_VOCABULARIES`].
pub const FAMILY_LANGUAGE_MODELS: &str = "custom language models";
/// See [`FAMILY_VOCABULARIES`].
pub const FAMILY_CALL_ANALYTICS_CATEGORIES: &str = "Call Analytics categories";

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

/// The most Call Analytics categories this emulator holds for an account IN ONE REGION.
///
/// A CHOSEN ceiling, not a published one: AWS gives no per-account figure for Call
/// Analytics categories in the model, and the only count the model bounds is the
/// RULES of one category (`RuleList` is `length {min 1, max 20}`, which the boundary
/// validator already enforces). Applying the same figure the other families use keeps
/// the store bounded together with [`MAX_REGIONS`] — a caller that never deletes cannot
/// grow it without end, and `DeleteCallAnalyticsCategory` is what makes room again.
pub const MAX_CALL_ANALYTICS_CATEGORIES: usize = 100;

/// `ModelStatus` values, from the model's enum
/// (`smithy_query(query="shape", shape="ModelStatus")`): a custom language model is
/// being trained, has finished training, or failed.
pub const MODEL_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`MODEL_STATUS_IN_PROGRESS`]: the status a model whose training data the object
/// store answered for settles in, and the one that makes it usable ("When the status
/// displays as COMPLETED, your model is ready to use").
pub const MODEL_STATUS_COMPLETED: &str = "COMPLETED";
/// See [`MODEL_STATUS_IN_PROGRESS`]: the status a model settles in when the object
/// store says a location its create named cannot be read (see
/// [`TrainingDataVerdict`]).
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
/// transition is owed (`IN_PROGRESS` -> the status its training data settles on,
/// `COMPLETED` or `FAILED` — see [`TrainingDataVerdict`]).
///
/// A chosen span, not a documented one: Amazon Transcribe gives no training-time
/// guarantee for a custom language model. It is deliberately non-zero, so a caller
/// that describes the model straight back still observes `IN_PROGRESS` (and the
/// state errors that go with it), and short enough that a poll-until-done script
/// terminates quickly. The due instant is stored on the record; nothing sleeps for it.
pub const MODEL_TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The most custom language models this emulator holds for an account IN ONE REGION.
///
/// A CHOSEN ceiling, not a published one: the model carries no per-account figure for
/// custom language models. Applying the same figure the other families use keeps the
/// store bounded together with [`MAX_REGIONS`] — a caller that never deletes cannot grow
/// it without end, and `DeleteLanguageModel` is what makes room again.
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
/// The ARN resource type of a Call Analytics JOB, spelled as this family's published
/// `<type>/${JobName}` segment
/// (`arn:${Partition}:transcribe:${Region}:${Account}:analytics-job/${JobName}`) — the
/// same hyphenation the sibling Call Analytics family uses
/// ([`RESOURCE_TYPE_CALL_ANALYTICS_CATEGORY`]). A resource type of its own, so a Call
/// Analytics job and a transcription job of the same name are two resources. See
/// [`RESOURCE_TYPE_TRANSCRIPTION_JOB`].
pub const RESOURCE_TYPE_CALL_ANALYTICS_JOB: &str = "analytics-job";

/// The word this service's own client-error messages use for the kind of resource an
/// ARN's `<type>` segment names, for each type this emulator stores records of.
///
/// The kinds are the ones the service's recorded messages name ("vocabulary",
/// "vocabulary filter", "job"), so a kind is what SELECTS the refusal a family words
/// for a condition ([`not_found_message`], [`name_already_exists_message`]) rather
/// than each handler spelling its own. An ARN whose type names nothing this emulator
/// holds gets the neutral `KIND_RESOURCE`.
pub const KIND_JOB: &str = "job";
/// See [`KIND_JOB`].
pub const KIND_VOCABULARY: &str = "vocabulary";
/// See [`KIND_JOB`].
pub const KIND_MEDICAL_VOCABULARY: &str = "medical vocabulary";
/// See [`KIND_JOB`].
pub const KIND_VOCABULARY_FILTER: &str = "vocabulary filter";
/// See [`KIND_JOB`].
pub const KIND_LANGUAGE_MODEL: &str = "language model";
/// See [`KIND_JOB`]: the word the Call Analytics category family's own refusals name
/// it by — [`CATEGORY_NOT_FOUND_MESSAGE`] and [`CATEGORY_NAME_EXISTS_MESSAGE`], which
/// are that family's wording for the two conditions a name can be refused for.
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
    /// [`RESOURCE_TYPE_CALL_ANALYTICS_JOB`].
    CallAnalyticsJob,
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
            RESOURCE_TYPE_CALL_ANALYTICS_JOB => Some(Self::CallAnalyticsJob),
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
            Self::CallAnalyticsJob => RESOURCE_TYPE_CALL_ANALYTICS_JOB,
        }
    }

    /// The word this service's own client-error messages name this family by.
    ///
    /// A Medical Scribe job, a Call Analytics job and a medical transcription job are
    /// each a "job" like the transcription-job family: the recorded not-found response
    /// of every job family reads "The requested job couldn't be found."
    pub fn kind(self) -> &'static str {
        match self {
            Self::TranscriptionJob
            | Self::MedicalTranscriptionJob
            | Self::MedicalScribeJob
            | Self::CallAnalyticsJob => KIND_JOB,
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
///
/// The wording is a property of the FAMILY, not a template over every family: most
/// families share the sentence above, and the Call Analytics category family words the
/// same condition its own way ([`CATEGORY_NOT_FOUND_MESSAGE`]), so the text is looked
/// up per family here rather than composed from the kind word at each call site.
pub fn not_found_message(kind: &str) -> String {
    if kind == KIND_CATEGORY {
        return CATEGORY_NOT_FOUND_MESSAGE.to_string();
    }
    format!(
        "The requested {kind} couldn't be found. Check the {kind} name and try your \
         request again."
    )
}

/// How this service words "there is no such Call Analytics category", for every
/// operation of that family that looks one up — the read, the update and the delete.
///
/// Entirely FIXED text: no part of it comes from the request, so the name the caller
/// asked about appears nowhere in it. Both apostrophes are U+2019 (RIGHT SINGLE
/// QUOTATION MARK), as the service writes them, not the ASCII `'`.
pub const CATEGORY_NOT_FOUND_MESSAGE: &str = "The category that you\u{2019}ve specified \
     couldn\u{2019}t be found. Check the category name and try your request again.";

/// How this service words "that Call Analytics category name is already taken", for the
/// create that claims a category name.
///
/// Entirely FIXED text, like [`CATEGORY_NOT_FOUND_MESSAGE`], with the same U+2019
/// apostrophe and no request value in it.
pub const CATEGORY_NAME_EXISTS_MESSAGE: &str = "The category name that you\u{2019}ve \
     requested already exists. Change the category name and try your request again.";

/// How this service refuses a request that would APPLY a custom resource whose
/// processing has not finished successfully: the resource of `kind` named `name` is in
/// `state`, and `usable` is the one value of that family's own processing member a job
/// may apply the resource in — [`VOCABULARY_STATE_READY`] for a custom vocabulary ("If
/// the state is READY, you can use the custom vocabulary" — the model's own doc for
/// `VocabularyState`), [`MODEL_STATUS_COMPLETED`] for a custom language model ("When
/// the status displays as COMPLETED the model is ready for use" — the model's own doc
/// for `LanguageModel$ModelStatus`).
///
/// Worded like the state refusals the deletes of every family carry ("... is in the
/// {state} state and can't be deleted. Wait until ..."), because it is the same kind of
/// refusal: a resource whose processing has not produced something usable. The
/// variable parts are the name the request asked for, the state this emulator holds
/// the resource in, and the value that family's own model documents as the usable one.
pub fn resource_not_usable_message(kind: &str, name: &str, state: &str, usable: &str) -> String {
    format!(
        "The custom {kind} {name} is in the {state} state and can't be used. A custom \
         {kind} can only be used when its state is {usable}. Check the \
         state of the {kind} and try your request again."
    )
}

/// Whether `state` is one a custom vocabulary's build has FINISHED in — the terminal
/// states of the `VocabularyState` enum.
///
/// [`VOCABULARY_STATE_PENDING`] is the one value of that enum a build is still running
/// in, so everything else is terminal. Single-homed here because every operation that
/// may only act on a FINISHED vocabulary — the deletes of both vocabulary families and
/// the updates that overwrite one — has to agree about which states those are.
pub fn is_terminal_vocabulary_state(state: &str) -> bool {
    state != VOCABULARY_STATE_PENDING
}

/// How this service refuses a request that would OVERWRITE a custom resource whose
/// build has not finished: the resource of `kind` named `name` is in `state`, which is
/// not one of the states its build has finished in
/// ([`is_terminal_vocabulary_state`] for a custom vocabulary,
/// [`is_terminal_model_status`] for a custom language model).
///
/// Worded like the state refusals the deletes of every family carry ("... is in the
/// {state} state and can't be deleted. Wait until it is no longer being processed ..."),
/// because it is the same refusal about the same records, for the one verb this
/// operation performs instead. The variable parts are the name the request asked for
/// and the state this emulator holds the resource in.
pub fn resource_not_updatable_message(kind: &str, name: &str, state: &str) -> String {
    format!(
        "The custom {kind} {name} is in the {state} state and can't be updated. Wait \
         until it is no longer being processed and try your request again."
    )
}

/// How this service words "that name is already taken", for a resource of `kind`.
///
/// Same shape of text as [`not_found_message`], and the same reason for naming only
/// the kind: "The requested vocabulary filter name already exists. Use a different
/// vocabulary filter name." Per family for the same reason too: the Call Analytics
/// category family words a taken name its own way
/// ([`CATEGORY_NAME_EXISTS_MESSAGE`]).
pub fn name_already_exists_message(kind: &str) -> String {
    if kind == KIND_CATEGORY {
        return CATEGORY_NAME_EXISTS_MESSAGE.to_string();
    }
    format!("The requested {kind} name already exists. Use a different {kind} name.")
}

/// How this service words "that name is already being claimed by a create this
/// emulator has not finished serving" — a collision with another CREATE rather than
/// with a resource that is already there.
///
/// The service words the two collisions differently: a create that loses the name to a
/// request still in flight gets this service's GENERIC sentence, the one its job
/// families carry, whichever family the create belongs to, while a create that
/// collides with a resource whose own create has already been answered gets that
/// family's own sentence ([`name_already_exists_message`]). Which of the two a refusal
/// carries is therefore decided by whether the claim on the name is still open
/// ([`ComAmazonawsTranscribeTranscribeState::name_conflict_message`]), never by the
/// family alone.
///
/// Entirely FIXED text: no part of it comes from the request.
pub fn contended_name_claim_message() -> String {
    name_already_exists_message(KIND_JOB)
}

/// How this service refuses a DELETE of a resource of `kind` whose processing has not
/// finished — "This can occur when the entity you're trying to delete doesn't exist or
/// if it's in a non-terminal state (such as IN PROGRESS)" (`BadRequestException`'s own
/// documentation).
///
/// Entirely FIXED text, like the other refusals of this service: neither the name the
/// caller asked about nor the state the resource is in appears in it. The wording is a
/// property of the FAMILY, as it is for [`not_found_message`]: the vocabulary families
/// name the two states a build has finished in — the terminal values of the
/// `VocabularyState` enum ([`is_terminal_vocabulary_state`]), which both of those
/// families' records carry — while the job families say only that the job is not
/// finished.
pub fn not_deletable_message(kind: &str) -> String {
    if kind == KIND_VOCABULARY || kind == KIND_MEDICAL_VOCABULARY {
        return format!(
            "The {kind} that you\u{2019}re trying to delete isn't in the {ready} or \
             {failed} state. Try your request again when the {kind} state is {ready} \
             or {failed}.",
            ready = VOCABULARY_STATE_READY,
            failed = VOCABULARY_STATE_FAILED,
        );
    }
    format!("The {kind} couldn't be deleted. Make sure it exists and isn't pending.")
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

/// The refusal for a start whose channel definitions do not each name a different
/// audio channel and a different participant, or `None` when they do.
///
/// A channel definition "makes it possible to specify which speaker is on which
/// channel", one definition per channel and one participant per definition: "if your
/// agent is the first participant to speak, you would set `ChannelId` to 0 (to indicate
/// the first channel) and `ParticipantRole` to `AGENT`", and for the Medical Scribe
/// family "set `ChannelId` of the first `ChannelDefinition` in the list to 0 ... and
/// `ParticipantRole` to `CLINICIAN` ... the `ChannelId` of the second ... to 1 ... and
/// `ParticipantRole` to `PATIENT`" (`smithy_query(query="shape",
/// shape="ChannelDefinition,MedicalScribeChannelDefinition")`). A list naming one
/// channel twice, or one participant twice, says nothing about which speaker is where,
/// so it is refused rather than stored. The modelled list length and the `ChannelId`
/// range are `@length` and `@range` constraints the request boundary already refuses,
/// so distinctness is all that is left to decide.
///
/// Single-homed here for the reason [`co_required_pair_error`] is: every family whose
/// start accepts channel definitions applies this same rule and is refused with this
/// same sentence, and Smithy has no constraint trait for distinctness over a member of
/// a structure in a list, so no boundary validation can refuse one. Each definition is
/// read as its channel id and the participant it NAMES, `None` for a definition that
/// names none: `ChannelDefinition$ParticipantRole` is optional ("Omitting this
/// parameter is equivalent to specifying both participants") while the Medical Scribe
/// family models it `@required`, so only the roles a request actually named are
/// compared and a definition that omits the role names no participant to clash with.
pub fn channel_definition_distinctness_error<'a>(
    definitions: impl IntoIterator<Item = (i32, Option<&'a str>)>,
) -> Option<String> {
    let mut channels = ::std::collections::BTreeSet::new();
    let mut roles = ::std::collections::BTreeSet::new();
    let (mut defined, mut named_roles) = (0usize, 0usize);
    for (channel_id, role) in definitions {
        defined += 1;
        channels.insert(channel_id);
        if let Some(role) = role {
            named_roles += 1;
            roles.insert(role);
        }
    }
    (channels.len() != defined || roles.len() != named_roles).then(|| {
        "Each ChannelDefinition must name a different ChannelId and a different \
         ParticipantRole. Correct your request and try again."
            .to_string()
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

/// The refusal for a request whose per-language `LanguageIdSettings` names a custom
/// vocabulary filter while the request names no method to apply it with, or `None` when
/// no entry of the map names a filter or the method is in the request.
///
/// `LanguageIdSettings$VocabularyFilterName` carries the very sentence its top-level
/// sibling does — "Note that if you include `VocabularyFilterName` in your request, you
/// must also include `VocabularyFilterMethod`" (`smithy_query(query="shape",
/// shape="LanguageIdSettings,Settings")`) — so a filter named in the map is the same
/// one-way co-requirement [`missing_companion_error`] words for the top-level member,
/// and is refused rather than stored. The map itself models no method member, so what
/// satisfies the requirement is the settings shape's `VocabularyFilterMethod`, the one
/// member of either start request that says how a filter is applied; the caller spells
/// both paths, as `method_member` and `map_member` are given here. Single-homed beside
/// [`extend_language_id_references`] for its reason: the operations that accept the map
/// cannot end up checking different members of it. The entries are walked in
/// language-code order rather than the map's own, so which entry a refusal names is the
/// same on every run.
pub fn language_id_filter_method_error(
    map_member: &str,
    per_language: Option<&std::collections::HashMap<String, LanguageIdSettings>>,
    method_member: &str,
    method_present: bool,
) -> Option<String> {
    let language = per_language?
        .iter()
        .filter(|(_, settings)| settings.vocabulary_filter_name.is_some())
        .map(|(language, _)| language)
        .min()?;
    // Reaching here IS the member being present: the walk above answered `None` unless
    // some entry named a filter, so the one side of the pair left to decide is the
    // method.
    missing_companion_error(
        &format!("{map_member}[{language}].VocabularyFilterName"),
        true,
        method_member,
        method_present,
    )
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

/// The tag-key prefix Amazon Web Services reserves for the tags it applies itself: a
/// caller may not create or edit a tag whose key starts with it, in any service.
///
/// `TagKey` is modelled as a bare string, `length {min 1, max 128}` with no `@pattern`
/// (`smithy_query(query="shape", shape="TagKey")`), so the request boundary cannot
/// refuse such a key and the handlers that WRITE tags are where the reservation is
/// enforced.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` claims the reserved namespace.
///
/// Compared case-INSENSITIVELY: what is reserved is the namespace, not one spelling of
/// it. The comparison is over BYTES, because a tag key may be any UTF-8 text and
/// slicing it by a character offset could fall inside a multi-byte character; a key
/// whose first bytes are not that prefix simply does not claim the namespace.
fn claims_reserved_tag_prefix(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}

/// Why a request's `Tags` cannot be written, or `None` when every key in it is a
/// caller's to name.
///
/// One rule for every operation that persists caller-supplied tags — `TagResource` and
/// each create/start whose input carries `Tags` — so none of them can store a key the
/// standard tagging API never accepts. The refusal is computed BEFORE anything is
/// written, so a request carrying a reserved key leaves the stored tag set, and the
/// resource it named, exactly as they were.
pub fn reserved_tag_key_refusal(tags: &[Tag]) -> Option<String> {
    let key = tags
        .iter()
        .find(|tag| claims_reserved_tag_prefix(&tag.key))?;
    Some(format!(
        "Your request includes the tag key {key}, which uses the reserved \
         {RESERVED_TAG_KEY_PREFIX} prefix. The {RESERVED_TAG_KEY_PREFIX} prefix is \
         reserved for the tags Amazon Web Services applies itself. Rename the tag and \
         try your request again.",
        key = key.key,
    ))
}

/// One write of `requested` onto the tag set `stored`, as the set the resource then
/// holds.
///
/// A tag is "metadata, in the form of a key:value pair" (`Tag`'s own model doc) and a
/// resource's tags are keyed by that key: a resource holds AT MOST ONE value for a key,
/// so a write replaces the value of a key already on the resource rather than adding a
/// second entry beside it, and a request that names one key more than once leaves the
/// LAST value it gave for that key. Keys are compared exactly, because a tag key is
/// case-sensitive: `Env` and `env` are two keys.
///
/// One helper for EVERY writer of a tag set — `TagResource` and each create/start whose
/// input carries `Tags` — so no path can store a key twice and leave two readers of the
/// record disagreeing about which of the two values is in force. What is already stored
/// keeps its order and a key new to the resource is appended, so the set reads back in
/// the order its keys were first written.
pub fn merge_tags(stored: &[Tag], requested: impl IntoIterator<Item = Tag>) -> Vec<Tag> {
    let mut merged = stored.to_vec();
    for tag in requested {
        match merged.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value,
            None => merged.push(tag),
        }
    }
    merged
}

/// The tag set a create or start stores for the record it establishes, from its
/// request's optional `Tags`.
///
/// [`merge_tags`] onto a resource that holds no tag yet: a repeated key is collapsed to
/// its last value, exactly as a later `TagResource` write of the same list would
/// collapse it, and a request carrying no `Tags` at all establishes an empty set.
pub fn created_tag_set(requested: Option<Vec<Tag>>) -> Vec<Tag> {
    merge_tags(&[], requested.unwrap_or_default())
}

/// One resource family's store, Region by Region.
///
/// Amazon Transcribe is a REGIONAL service: a request reaches the endpoint of exactly
/// one Region, and the resources that endpoint serves are the ones created through it.
/// So the key of a record is the Region it was created in TOGETHER with its name, and
/// every lookup, listing, uniqueness check and quota count is scoped to the Region the
/// calling request was signed for ([`request_region`]) — two Regions hold two
/// independent sets of resources that may use the same names, and neither can read,
/// mutate, delete or crowd out the other's.
///
/// One generic store rather than a region dimension bolted onto each of the nine
/// families: they differ in what a record IS, not in how a Region scopes it.
#[derive(Debug)]
pub struct RegionalStore<R> {
    /// Keyed `(region, name)`, so the records of one Region are a contiguous range and
    /// a name is unique only within its Region.
    records: BTreeMap<(String, String), R>,
}

impl<R> Default for RegionalStore<R> {
    fn default() -> Self {
        Self {
            records: BTreeMap::new(),
        }
    }
}

impl<R> RegionalStore<R> {
    /// The record `region` holds under `name`, or `None` when that Region holds none —
    /// whatever any other Region holds under the same name.
    pub fn get(&self, region: &str, name: &str) -> Option<&R> {
        self.records.get(&(region.to_string(), name.to_string()))
    }

    /// [`Self::get`] for a caller that mutates the record it finds.
    pub fn get_mut(&mut self, region: &str, name: &str) -> Option<&mut R> {
        self.records
            .get_mut(&(region.to_string(), name.to_string()))
    }

    /// Whether `region` holds a record under `name` — the uniqueness check a create of
    /// any family makes, which is why the same name may be created once per Region.
    pub fn contains(&self, region: &str, name: &str) -> bool {
        self.get(region, name).is_some()
    }

    /// Store `record` as `region`'s record under `name`, answering the record it
    /// replaced, if any.
    pub fn insert(&mut self, region: &str, name: &str, record: R) -> Option<R> {
        self.records
            .insert((region.to_string(), name.to_string()), record)
    }

    /// Drop `region`'s record under `name`, answering it when there was one.
    pub fn remove(&mut self, region: &str, name: &str) -> Option<R> {
        self.records.remove(&(region.to_string(), name.to_string()))
    }

    /// How many records `region` holds — what a per-account-per-Region ceiling counts.
    pub fn len_in(&self, region: &str) -> usize {
        self.iter_in(region).count()
    }

    /// `region`'s records with the name each is stored under, in name order.
    pub fn iter_in(&self, region: &str) -> impl Iterator<Item = (&str, &R)> {
        let owned = region.to_string();
        self.records
            .range((owned.clone(), String::new())..)
            .take_while(move |((stored, _), _)| *stored == owned)
            .map(|((_, name), record)| (name.as_str(), record))
    }

    /// `region`'s records, in name order.
    pub fn values_in(&self, region: &str) -> impl Iterator<Item = &R> {
        self.iter_in(region).map(|(_, record)| record)
    }

    /// `region`'s records for mutation, in name order — how a listing materialises
    /// every clock-owed transition of the Region it is reporting on.
    pub fn values_in_mut(&mut self, region: &str) -> impl Iterator<Item = &mut R> {
        let owned = region.to_string();
        self.records
            .range_mut((owned.clone(), String::new())..)
            .take_while(move |((stored, _), _)| *stored == owned)
            .map(|(_, record)| record)
    }

    /// Whether this store holds any record for `region`.
    ///
    /// A Region that held records and no longer does answers `false`, so a deleted
    /// Region stops counting against [`MAX_REGIONS`] — which is what keeps that cap a
    /// bound on what is actually stored rather than on what was ever asked for.
    pub fn holds_region(&self, region: &str) -> bool {
        self.values_in(region).next().is_some()
    }

    /// How many Regions this store holds records for, which [`MAX_REGIONS`] bounds.
    ///
    /// The keys are ordered by Region first, so one walk counts the distinct ones.
    pub fn regions_held(&self) -> usize {
        let mut held = 0;
        let mut previous: Option<&str> = None;
        for (region, _) in self.records.keys() {
            if previous != Some(region.as_str()) {
                held += 1;
                previous = Some(region.as_str());
            }
        }
        held
    }

    /// Whether a record for `region` cannot be stored because this store already holds
    /// records for [`MAX_REGIONS`] OTHER Regions.
    ///
    /// The per-Region quotas bound one Region's records; this bounds how many Regions
    /// have records at all. Both are needed, because the Region a record is keyed by is
    /// the request's own SigV4 credential-scope Region, which this emulator does not
    /// authenticate: without this cap a caller could open a fresh per-Region bucket,
    /// each with its own quota, on every request.
    pub fn region_bucket_exhausted(&self, region: &str) -> bool {
        !self.holds_region(region) && self.regions_held() >= MAX_REGIONS
    }
}

/// One family of records this store mints creation sequences for, and so pages a
/// listing of by that sequence.
///
/// Each family mints from its OWN counter
/// ([`ComAmazonawsTranscribeTranscribeState::next_sequence`]) and exactly one listing
/// reports it ([`Self::listing`]), which is what makes a page cursor belong to that one
/// listing: the sequences a cursor of a listing can carry are the ones its own family
/// has minted, so a cursor another listing handed out is not one of them (see
/// [`CursorScope`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SequencedFamily {
    /// [`ComAmazonawsTranscribeTranscribeState::transcription_jobs`].
    TranscriptionJobs,
    /// [`ComAmazonawsTranscribeTranscribeState::medical_transcription_jobs`].
    MedicalTranscriptionJobs,
    /// [`ComAmazonawsTranscribeTranscribeState::medical_scribe_jobs`].
    MedicalScribeJobs,
    /// [`ComAmazonawsTranscribeTranscribeState::call_analytics_jobs`].
    CallAnalyticsJobs,
    /// [`ComAmazonawsTranscribeTranscribeState::language_models`].
    LanguageModels,
    /// [`ComAmazonawsTranscribeTranscribeState::vocabularies`].
    Vocabularies,
    /// [`ComAmazonawsTranscribeTranscribeState::medical_vocabularies`].
    MedicalVocabularies,
    /// [`ComAmazonawsTranscribeTranscribeState::vocabulary_filters`].
    VocabularyFilters,
}

impl SequencedFamily {
    /// The one operation that lists this family: the listing whose pages carry a cursor
    /// minted in it, and the operation its refusal of any other cursor names
    /// ([`invalid_next_token_message`]).
    pub const fn listing(self) -> &'static str {
        match self {
            Self::TranscriptionJobs => "ListTranscriptionJobs",
            Self::MedicalTranscriptionJobs => "ListMedicalTranscriptionJobs",
            Self::MedicalScribeJobs => "ListMedicalScribeJobs",
            Self::CallAnalyticsJobs => "ListCallAnalyticsJobs",
            Self::LanguageModels => "ListLanguageModels",
            Self::Vocabularies => "ListVocabularies",
            Self::MedicalVocabularies => "ListMedicalVocabularies",
            Self::VocabularyFilters => "ListVocabularyFilters",
        }
    }
}

/// The open claim a create holds on the name it recorded, released however that create
/// ends.
///
/// A create takes the claim under the same lock as the insert
/// ([`ComAmazonawsTranscribeTranscribeState::claim_name`]) and hands the guard the rest
/// of its own work: dropping it releases the claim, so a create whose future is
/// abandoned before it reaches its answer — a caller that went away mid-request — leaves
/// no claim behind, and the store holds at most one entry per create actually in
/// flight.
pub struct NameClaim {
    state: ::engine::ServiceState<ComAmazonawsTranscribeTranscribeState>,
    family: ResourceFamily,
    region: String,
    name: String,
}

impl NameClaim {
    /// Take ownership of the claim [`ComAmazonawsTranscribeTranscribeState::claim_name`]
    /// has just recorded for `name` of `family` in `region`.
    ///
    /// Called with the store lock RELEASED and with no await in between, so the claim
    /// is never left unowned.
    pub fn held(
        state: &::engine::ServiceState<ComAmazonawsTranscribeTranscribeState>,
        family: ResourceFamily,
        region: &str,
        name: &str,
    ) -> Self {
        Self {
            state: state.clone(),
            family,
            region: region.to_string(),
            name: name.to_string(),
        }
    }
}

impl Drop for NameClaim {
    fn drop(&mut self) {
        self.state
            .lock()
            .release_name_claim(self.family, &self.region, &self.name);
    }
}

/// Per-service state for com-amazonaws-transcribe-transcribe. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTranscribeTranscribeState {
    /// Every transcription job this account has started, keyed by the Region it was
    /// started in and its name (which `StartTranscriptionJob` requires to be unique).
    pub transcription_jobs: RegionalStore<TranscriptionJobRecord>,
    /// Every MEDICAL transcription job this account has started, keyed by Region and
    /// name (which `StartMedicalTranscriptionJob` requires to be unique within an
    /// account). A family of its own: a medical transcription job and a transcription
    /// job may share a name.
    pub medical_transcription_jobs: RegionalStore<MedicalTranscriptionJobRecord>,
    /// Every custom vocabulary this account holds, keyed by Region and name (which
    /// `CreateVocabulary` requires to be unique within an account).
    pub vocabularies: RegionalStore<VocabularyRecord>,
    /// Every custom MEDICAL vocabulary this account holds, keyed by Region and name
    /// (which `CreateMedicalVocabulary` requires to be unique within an account). A
    /// family of its own: a medical vocabulary and a custom vocabulary may share a name.
    pub medical_vocabularies: RegionalStore<MedicalVocabularyRecord>,
    /// Every custom vocabulary filter this account holds, keyed by Region and name
    /// (which `CreateVocabularyFilter` requires to be unique within an account).
    pub vocabulary_filters: RegionalStore<VocabularyFilterRecord>,
    /// Every Call Analytics category this account holds, keyed by Region and name
    /// ("Category names are case sensitive and must be unique within an Amazon Web
    /// Services account" — model doc for `CategoryProperties$CategoryName`).
    pub call_analytics_categories: RegionalStore<CallAnalyticsCategoryRecord>,
    /// Every custom language model this account holds, keyed by Region and name (which
    /// `CreateLanguageModel` requires to be unique within an account: "If you try to
    /// create a new custom language model with the same name as an existing custom
    /// language model, you get a `ConflictException` error").
    pub language_models: RegionalStore<LanguageModelRecord>,
    /// Every Medical Scribe job this account has started, keyed by Region and name
    /// (which `StartMedicalScribeJob` requires to be "unique within an Amazon Web
    /// Services account").
    ///
    /// A job of this family carries the tags its start request asked for, reports them
    /// through its own operations, and is reachable from [`Self::tags`] /
    /// [`Self::tags_mut`] under [`RESOURCE_TYPE_MEDICAL_SCRIBE_JOB`], like every other
    /// family this state holds records of.
    pub medical_scribe_jobs: RegionalStore<MedicalScribeJobRecord>,
    /// Every Call Analytics job this account has started, keyed by Region and name
    /// ("Job names are case sensitive and must be unique within an Amazon Web Services
    /// account" — model doc for `CallAnalyticsJob$CallAnalyticsJobName`).
    ///
    /// A family of its own: a Call Analytics job and a transcription job may share a
    /// name. A job of this family carries the tags its start request asked for, reports
    /// them through its own operations, and is reachable from [`Self::tags`] /
    /// [`Self::tags_mut`] under [`RESOURCE_TYPE_CALL_ANALYTICS_JOB`], like every other
    /// family this state holds records of.
    pub call_analytics_jobs: RegionalStore<CallAnalyticsJobRecord>,
    /// Source of every family's record sequence (e.g.
    /// [`TranscriptionJobRecord::sequence`]): one monotonic counter PER FAMILY, so
    /// creation order is total within a family even when two of its records are
    /// established in the same clock tick, and the sequences one family mints are
    /// exactly `1..=`[`Self::minted_sequences`] of that family.
    next_sequence: BTreeMap<SequencedFamily, u64>,
    /// The name claims of creates this emulator has not FINISHED serving, keyed by the
    /// claiming family's ARN `<type>` segment, the Region and the name.
    ///
    /// A create that claims a free name is not done once the record is in the store: it
    /// still has the work the resource it just recorded needs before the create is
    /// answered (the text file a custom vocabulary's reported download location
    /// serves). Another create of the same name arriving inside that window collides
    /// with a request still being served rather than with a resource that is already
    /// there, and this service words those two refusals differently
    /// ([`contended_name_claim_message`] against [`name_already_exists_message`]), so
    /// the open claims are held here for exactly as long as the create holding one is
    /// still being served — [`Self::claim_name`] on the insert, released by the
    /// [`NameClaim`] guard that create then owns.
    open_name_claims: BTreeSet<(&'static str, String, String)>,
}

impl ComAmazonawsTranscribeTranscribeState {
    /// The creation sequence number for the next record of `family`.
    ///
    /// Counted per family rather than once for the whole store: a page cursor carries
    /// the creation sequence of the last record on the page it was handed out with
    /// ([`page_cursor`]) and is only ever presented to the listing of ONE family, so
    /// "a sequence that listing could have handed out" is decidable only when the
    /// counter it was minted from is that family's own (see [`parse_page_cursor`]).
    pub fn next_sequence(&mut self, family: SequencedFamily) -> u64 {
        let minted = self.next_sequence.entry(family).or_default();
        *minted += 1;
        *minted
    }

    /// The highest creation sequence this store has minted for `family` — 0 before the
    /// first record of it was created.
    ///
    /// Every sequence page cursor a listing hands out carries the sequence of a record
    /// on the page it was handed out with ([`page_cursor`]), so a cursor naming a
    /// sequence past this one is a token that listing could not have written
    /// ([`parse_page_cursor`]). A family's counter only ever grows, so deleting a
    /// record cannot invalidate a cursor already handed out.
    pub fn minted_sequences(&self, family: SequencedFamily) -> u64 {
        self.next_sequence.get(&family).copied().unwrap_or_default()
    }

    /// Record that the create being served is claiming `name` for `family` in `region`,
    /// and is not finished yet.
    ///
    /// Called under the same lock as the insert of the record itself, so the claim and
    /// the record become visible to another caller together. The create then owns a
    /// [`NameClaim`] that releases it; see [`Self::open_name_claims`] for why the window
    /// is held at all.
    pub fn claim_name(&mut self, family: ResourceFamily, region: &str, name: &str) {
        self.open_name_claims.insert((
            family.resource_type(),
            region.to_string(),
            name.to_string(),
        ));
    }

    /// Record that the create which claimed `name` for `family` in `region` has
    /// finished: the name is now held by a resource that is simply there.
    pub fn release_name_claim(&mut self, family: ResourceFamily, region: &str, name: &str) {
        self.open_name_claims.remove(&(
            family.resource_type(),
            region.to_string(),
            name.to_string(),
        ));
    }

    /// How a create that finds `name` of `family` already taken in `region` is refused:
    /// the service's generic sentence while the claim on that name belongs to a create
    /// still being served, and the family's own sentence once that create has finished.
    ///
    /// Read under the lock that found the name taken, so the refusal a racing create
    /// gets and the state that decided it are one observation.
    pub fn name_conflict_message(
        &self,
        family: ResourceFamily,
        region: &str,
        name: &str,
    ) -> String {
        if self.open_name_claims.contains(&(
            family.resource_type(),
            region.to_string(),
            name.to_string(),
        )) {
            return contended_name_claim_message();
        }
        name_already_exists_message(family.kind())
    }

    /// The transcription job named `name`, through the shared lookup ([`record_mut`]),
    /// so every operation that reads or deletes a transcription job sees the status the
    /// clock already owed it.
    pub fn transcription_job_mut(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut TranscriptionJobRecord> {
        record_mut(&mut self.transcription_jobs, region, name, now)
    }

    /// This family's share of `region`, through the shared listing projection
    /// ([`materialised_newest_first`]) — the order `ListTranscriptionJobs` documents
    /// ("Jobs are ordered by creation date, with the newest job first").
    pub fn jobs_newest_first(
        &mut self,
        region: &str,
        now: SystemTime,
    ) -> Vec<&TranscriptionJobRecord> {
        materialised_newest_first(&mut self.transcription_jobs, region, now)
    }

    /// The MEDICAL transcription job named `name`, through the shared lookup
    /// ([`record_mut`]).
    pub fn medical_transcription_job_mut(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut MedicalTranscriptionJobRecord> {
        record_mut(&mut self.medical_transcription_jobs, region, name, now)
    }

    /// This family's share of `region`, through the shared listing projection
    /// ([`materialised_newest_first`]) — the order `ListMedicalTranscriptionJobs`
    /// documents ("Jobs are ordered by creation date, with the newest job first").
    pub fn medical_transcription_jobs_newest_first(
        &mut self,
        region: &str,
        now: SystemTime,
    ) -> Vec<&MedicalTranscriptionJobRecord> {
        materialised_newest_first(&mut self.medical_transcription_jobs, region, now)
    }

    /// The Medical Scribe job named `name`, through the shared lookup ([`record_mut`]).
    pub fn medical_scribe_job_mut(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut MedicalScribeJobRecord> {
        record_mut(&mut self.medical_scribe_jobs, region, name, now)
    }

    /// This family's share of `region`, through the shared listing projection
    /// ([`materialised_newest_first`]) — the order `ListMedicalScribeJobs` documents
    /// ("Jobs are ordered by creation date, with the newest job first").
    pub fn medical_scribe_jobs_newest_first(
        &mut self,
        region: &str,
        now: SystemTime,
    ) -> Vec<&MedicalScribeJobRecord> {
        materialised_newest_first(&mut self.medical_scribe_jobs, region, now)
    }

    /// The Call Analytics job named `name`, through the shared lookup ([`record_mut`]).
    pub fn call_analytics_job_mut(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut CallAnalyticsJobRecord> {
        record_mut(&mut self.call_analytics_jobs, region, name, now)
    }

    /// This family's share of `region`, through the shared listing projection
    /// ([`materialised_newest_first`]) — the order `ListCallAnalyticsJobs` documents
    /// ("Jobs are ordered by creation date, with the newest job first").
    pub fn call_analytics_jobs_newest_first(
        &mut self,
        region: &str,
        now: SystemTime,
    ) -> Vec<&CallAnalyticsJobRecord> {
        materialised_newest_first(&mut self.call_analytics_jobs, region, now)
    }

    /// The custom vocabulary named `name`, through the shared lookup ([`record_mut`]).
    pub fn vocabulary_mut(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut VocabularyRecord> {
        record_mut(&mut self.vocabularies, region, name, now)
    }

    /// This family's share of `region`, through the shared listing projection
    /// ([`materialised_newest_first`]).
    pub fn vocabularies_newest_first(
        &mut self,
        region: &str,
        now: SystemTime,
    ) -> Vec<&VocabularyRecord> {
        materialised_newest_first(&mut self.vocabularies, region, now)
    }

    /// The custom MEDICAL vocabulary named `name`, through the shared lookup
    /// ([`record_mut`]).
    pub fn medical_vocabulary_mut(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut MedicalVocabularyRecord> {
        record_mut(&mut self.medical_vocabularies, region, name, now)
    }

    /// This family's share of `region`, through the shared listing projection
    /// ([`materialised_newest_first`]) — the order `ListMedicalVocabularies` documents
    /// ("Custom vocabularies are ordered by creation date, with the newest vocabulary
    /// first").
    pub fn medical_vocabularies_newest_first(
        &mut self,
        region: &str,
        now: SystemTime,
    ) -> Vec<&MedicalVocabularyRecord> {
        materialised_newest_first(&mut self.medical_vocabularies, region, now)
    }

    /// This family's share of `region`, through the shared ordering alone
    /// ([`newest_first`]): a filter has no `VocabularyState` in the model, so none of
    /// them has a clock-owed transition to materialise first.
    pub fn vocabulary_filters_newest_first(&self, region: &str) -> Vec<&VocabularyFilterRecord> {
        newest_first(&self.vocabulary_filters, region)
    }

    /// The custom language model named `name`, through the shared lookup
    /// ([`record_mut`]).
    pub fn language_model_mut(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut LanguageModelRecord> {
        record_mut(&mut self.language_models, region, name, now)
    }

    /// This family's share of `region`, through the shared listing projection
    /// ([`materialised_newest_first`]) — the order `ListLanguageModels` documents
    /// ("Language models are ordered by creation date, with the newest model first").
    pub fn language_models_newest_first(
        &mut self,
        region: &str,
        now: SystemTime,
    ) -> Vec<&LanguageModelRecord> {
        materialised_newest_first(&mut self.language_models, region, now)
    }

    /// Whether `region` holds a resource of `family` named `name`.
    ///
    /// Reads the same per-family lookup [`Self::tags`] does, so "this Region holds
    /// it" cannot mean one thing to a tagging call and another to a job start. Borrows
    /// the name rather than building a [`ResourceRef`], so a start checking several
    /// references allocates nothing.
    pub fn holds(&self, family: ResourceFamily, region: &str, name: &str) -> bool {
        self.tags_of(family, region, name).is_some()
    }

    /// The refusal for the first resource `references` names that the Region this
    /// request is served in cannot apply — because it holds no such resource at all, or
    /// because the one it holds is not in a state its terms can be applied in — or
    /// `None` when every named one is held and usable.
    ///
    /// A job's settings may NAME a custom vocabulary, custom vocabulary filter or
    /// custom language model, and a name no create of that family ever recorded IN THIS
    /// REGION names nothing this emulator could apply — so a start that carries one is
    /// refused with the family's own not-found text ([`not_found_message`]) rather than
    /// stored on a job that would report the dangling name back. Each reference is the
    /// family the member's target resource belongs to and the name the request put
    /// there, or `None` where the request left the member out; the families are distinct
    /// stores, so a resource of one family never stands in for another's, and so is each
    /// Region, so a resource of another Region never stands in either.
    ///
    /// A resource that IS held must also be one a job can apply: a family whose records
    /// carry a processing member carries it precisely so that only processing which
    /// finished successfully is usable — a custom vocabulary's `VocabularyState` ("If
    /// the state is READY, you can use the custom vocabulary") and a custom language
    /// model's `ModelStatus` ("When the status displays as COMPLETED the model is ready
    /// for use") — so a resource still being built or trained, or one whose build or
    /// training failed, is refused with [`resource_not_usable_message`] — see
    /// [`Self::unusable_reference_state`] for which families carry such a member here.
    /// Takes `&mut self` and `now` for that
    /// reason: the state compared against is the one the CLOCK already owes the
    /// resource, materialised first exactly as every reader of that family materialises
    /// it, so a build that fell due before this request is not mistaken for one still
    /// running.
    pub fn unheld_reference_error(
        &mut self,
        region: &str,
        now: SystemTime,
        references: &[(ResourceFamily, Option<&str>)],
    ) -> Option<String> {
        for (family, name) in references {
            let Some(name) = *name else {
                continue;
            };
            if !self.holds(*family, region, name) {
                return Some(not_found_message(family.kind()));
            }
            if let Some((state, usable)) =
                self.unusable_reference_state(*family, region, name, now)
            {
                return Some(resource_not_usable_message(
                    family.kind(),
                    name,
                    &state,
                    usable,
                ));
            }
        }
        None
    }

    /// The state the resource of `family` named `name` is in when that state is NOT one
    /// a job may apply the resource in, paired with the value that family's own model
    /// documents as the usable one — and `None` when the resource is usable, or when
    /// this family has no such state here.
    ///
    /// THE RULE: a family whose records carry a processing member has exactly one value
    /// of it a job may apply the resource in, so a resource held in any other value of
    /// that member is refused ([`resource_not_usable_message`]):
    ///
    /// * a custom vocabulary of EITHER family — the plain one ([`VocabularyRecord`]) and
    ///   the MEDICAL one ([`MedicalVocabularyRecord`]) each carry the same modelled
    ///   `VocabularyState` for the same reason ("If the state is READY, you can use the
    ///   custom vocabulary in a `StartMedicalTranscriptionJob` request" — the model's own
    ///   doc for `GetMedicalVocabularyResponse$VocabularyState`) — reaches
    ///   [`VOCABULARY_STATE_READY`] when its build succeeded, and a request can find one
    ///   `PENDING` while that build runs or `FAILED` once it produced no terms;
    /// * a custom LANGUAGE model reaches [`MODEL_STATUS_COMPLETED`] when its training
    ///   ended on data the object store answered for ("When the status displays as
    ///   COMPLETED the model is ready for use" — the model's own doc for
    ///   `LanguageModel$ModelStatus`), and a request can find one `IN_PROGRESS` while
    ///   that training runs or `FAILED` once the store said its training data could not
    ///   be read ([`is_terminal_model_status`] is the same enum's finished/unfinished
    ///   split, which the operations that DELETE or UPDATE a model read).
    ///
    /// The other families a settings reference can name carry no such member at all, so
    /// they are checked for existence only:
    ///
    /// * a custom vocabulary FILTER models no processing state anywhere (see
    ///   [`VocabularyFilterRecord`]), so a held filter is always applicable;
    /// * the job families are never the target of a settings reference.
    fn unusable_reference_state(
        &mut self,
        family: ResourceFamily,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<(String, &'static str)> {
        match family {
            ResourceFamily::Vocabulary => {
                let record = self.vocabulary_mut(region, name, now)?;
                (record.state != VOCABULARY_STATE_READY)
                    .then(|| (record.state.clone(), VOCABULARY_STATE_READY))
            }
            ResourceFamily::MedicalVocabulary => {
                let record = self.medical_vocabulary_mut(region, name, now)?;
                (record.state != VOCABULARY_STATE_READY)
                    .then(|| (record.state.clone(), VOCABULARY_STATE_READY))
            }
            ResourceFamily::LanguageModel => {
                let record = self.language_model_mut(region, name, now)?;
                (record.status != MODEL_STATUS_COMPLETED)
                    .then(|| (record.status.clone(), MODEL_STATUS_COMPLETED))
            }
            _ => None,
        }
    }

    /// The tag set of the resource `resource` names to a request served in `region`, or
    /// `None` when that ARN names no resource this call can reach.
    ///
    /// The read-only half of [`Self::tags_mut`], so a reporting operation
    /// (`ListTagsForResource`) takes no mutable borrow for a read. The two cannot
    /// disagree about which ARN names which stored record: they read the same ARN
    /// qualifiers ([`ResourceRef::is_served_in`]), the same `<type>`-segment mapping
    /// ([`ResourceFamily::from_resource_type`]) and the same per-family dispatch, whose
    /// two borrow kinds ([`Self::tags_of`] and [`Self::tags_of_mut`]) are generated from
    /// a single table of family-to-store pairs.
    pub fn tags(&self, resource: &ResourceRef, region: &str) -> Option<&Vec<Tag>> {
        if !resource.is_served_in(region) {
            return None;
        }
        let family = ResourceFamily::from_resource_type(&resource.resource_type)?;
        self.tags_of(family, region, &resource.name)
    }

    /// The tag set of the resource `resource` names, for reading and writing, or
    /// `None` when that Region holds no such resource.
    ///
    /// One lookup for the two tagging operations that WRITE, so `TagResource` and
    /// `UntagResource` cannot disagree about which ARN names which stored record. It
    /// reaches EVERY family this emulator holds records of — each of their creates
    /// accepts `Tags`, so each of their records carries a tag set the standard tagging
    /// API owns. An ARN whose `<type>` segment names no family this emulator stores, or
    /// whose `<name>` names no stored record of that family, answers `None`, which the
    /// caller renders as the modelled not-found error — never as an empty tag set. So
    /// does an ARN qualified for another partition, Region or account than the request
    /// is served in ([`ResourceRef::is_served_in`]): the record a local ARN of the same
    /// `<type>/<name>` would reach is a DIFFERENT resource from the one that ARN names,
    /// so a write through it would mutate a resource the caller did not address.
    ///
    /// Which family's store answers for the ARN's `<type>` segment is decided by the
    /// generated [`Self::tags_of_mut`], the write-borrow half of the very table the read
    /// [`Self::tags`] dispatches through.
    pub fn tags_mut(&mut self, resource: &ResourceRef, region: &str) -> Option<&mut Vec<Tag>> {
        if !resource.is_served_in(region) {
            return None;
        }
        let family = ResourceFamily::from_resource_type(&resource.resource_type)?;
        self.tags_of_mut(family, region, &resource.name)
    }
}

/// Give the state BOTH borrow kinds of the per-family tag-set lookup from ONE table of
/// `ResourceFamily => store` pairs.
///
/// Which family's store answers for an ARN's `<type>` segment is a single rule, and its
/// read half (`tags_of`, which `ListTagsForResource` reads through
/// [`ComAmazonawsTranscribeTranscribeState::tags`] and every job start through
/// [`ComAmazonawsTranscribeTranscribeState::holds`]) and its write half (`tags_of_mut`,
/// which `TagResource` and `UntagResource` write through
/// [`ComAmazonawsTranscribeTranscribeState::tags_mut`]) differ in nothing but the borrow
/// they take — so both are emitted from the one table below rather than written twice,
/// as the `clocked_records!` and `listed_records!` tables of this module emit a family's
/// per-record views. Neither generated match has a catch-all arm, so a family added to
/// [`ResourceFamily`] and forgotten here fails to compile in both halves at once: a
/// resource whose tags can be read but not written is no longer expressible.
macro_rules! tag_sets {
    ($($family:ident => $store:ident;)+) => {
        impl ComAmazonawsTranscribeTranscribeState {
            /// The tag set of the resource `family` names `name` in `region`, or `None`
            /// when that Region holds no such resource.
            ///
            /// The lookup [`Self::tags`] and [`Self::holds`] both read, by family,
            /// Region and borrowed name: one table decides which family's store answers
            /// for which resource.
            pub fn tags_of(
                &self,
                family: ResourceFamily,
                region: &str,
                name: &str,
            ) -> Option<&Vec<Tag>> {
                match family {
                    $(
                        ResourceFamily::$family => {
                            self.$store.get(region, name).map(|record| &record.tags)
                        }
                    )+
                }
            }

            /// The same lookup for writing, which [`Self::tags_mut`] reaches every
            /// family's tag set through: the same family-to-store pairing, under a
            /// mutable borrow.
            pub fn tags_of_mut(
                &mut self,
                family: ResourceFamily,
                region: &str,
                name: &str,
            ) -> Option<&mut Vec<Tag>> {
                match family {
                    $(
                        ResourceFamily::$family => {
                            self.$store.get_mut(region, name).map(|record| &mut record.tags)
                        }
                    )+
                }
            }
        }
    };
}

// Every family this emulator holds records of, paired with the store those records live
// in — the one place that says which ARN `<type>` segment names which stored record.
tag_sets! {
    TranscriptionJob => transcription_jobs;
    MedicalTranscriptionJob => medical_transcription_jobs;
    Vocabulary => vocabularies;
    MedicalVocabulary => medical_vocabularies;
    VocabularyFilter => vocabulary_filters;
    LanguageModel => language_models;
    CallAnalyticsCategory => call_analytics_categories;
    MedicalScribeJob => medical_scribe_jobs;
    CallAnalyticsJob => call_analytics_jobs;
}

/// The resource an Amazon Transcribe ARN names: the partition, Region and account that
/// QUALIFY it, plus its type and its name.
///
/// Every segment is kept, because every segment is part of what the ARN names: the
/// `<type>/<name>` pair identifies a resource only within one partition, one Region and
/// one account. Which of those an ARN has to carry to name a resource a given request
/// can reach is [`Self::is_served_in`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRef {
    /// The ARN's partition segment (`aws`, `aws-cn`, `aws-us-gov`, …).
    pub partition: String,
    /// The ARN's Region segment: the one Region the resource it names lives in.
    pub region: String,
    /// The ARN's account segment: the one account that holds the resource it names.
    pub account: String,
    /// The ARN's resource type, e.g. [`RESOURCE_TYPE_VOCABULARY`].
    pub resource_type: String,
    /// The resource's name, which together with the Region keys the store that holds it.
    pub name: String,
}

impl ResourceRef {
    /// Whether this ARN names a resource of the partition, Region and account a request
    /// served in `region` addresses.
    ///
    /// An AWS resource ARN is partition-, Region- and account-qualified, so a resource
    /// is identified by those three segments TOGETHER with its type and name: an ARN
    /// that changes any of them names a different resource — one that lives where this
    /// request is not served and whose tags are therefore not this store's to report or
    /// to write. True exactly when the ARN is the one [`resource_arn`] hands out for the
    /// same type and name to a request served in `region`, so what a caller may ask
    /// about and what this service would hand it are the same rule rather than two.
    pub fn is_served_in(&self, region: &str) -> bool {
        parse_resource_arn(&resource_arn(&self.resource_type, &self.name, region))
            .is_some_and(|served| served == *self)
    }
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
///
/// This is the SHAPE of the ARN only: an ARN of this form is still refused when its
/// qualifiers are not the ones the request is served in (see
/// [`ResourceRef::is_served_in`]), because a well-formed ARN of another Region or
/// account names a resource no request served here holds.
pub fn parse_resource_arn(arn: &str) -> Option<ResourceRef> {
    let fields: Vec<&str> = arn.split(':').collect();
    let [prefix, partition, service, region, account, resource] = fields.as_slice() else {
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
        partition: (*partition).to_string(),
        region: (*region).to_string(),
        account: (*account).to_string(),
        resource_type: resource_type.to_string(),
        name: name.to_string(),
    })
}

/// The ARN of one Amazon Transcribe resource, built through the framework's own ARN
/// helper so the partition and account segments are the canonical emulator ones.
///
/// Also the authority on which ARN names that resource at all: a caller-supplied ARN
/// resolves only when it is this one ([`ResourceRef::is_served_in`]), so the ARN this
/// service would hand out and the ARN it accepts cannot drift apart.
pub fn resource_arn(resource_type: &str, name: &str, region: &str) -> String {
    ::engine::emulator_arn("transcribe", region, &format!("{resource_type}/{name}"))
}

/// One stored custom vocabulary: where its terms come from, plus the lifecycle facts
/// the service itself owns (its processing state, why that state is what it is, and
/// when it was last modified).
///
/// The two content members `CreateVocabulary` publishes are alternatives — the terms
/// in `Phrases`, or the file at `VocabularyFileUri` — and EITHER produces the same
/// record: the terms of a request that listed them inline, or the terms read from the
/// file the object store served ([`resolve_vocabulary_file`]). What the build loaded is
/// what decides the one transition the clock owes it (see [`Self::advance_clock`]).
#[derive(Debug, Clone)]
pub struct VocabularyRecord {
    /// `VocabularyName`, the second half of this record's key in the store (the Region
    /// it was created in is the first).
    pub name: String,
    /// Creation order, newest = largest. Also the cursor `ListVocabularies` pages
    /// with.
    pub sequence: u64,
    /// `LanguageCode`: the one language this vocabulary's terms are in.
    pub language_code: String,
    /// The terms this vocabulary holds: the list the request carried in `Phrases`, or
    /// the lines of the file at `VocabularyFileUri` the object store served — EMPTY
    /// only for a build whose file it did not serve, which loaded none.
    pub phrases: Vec<String>,
    /// `VocabularyFileUri` when the request named a file as the source of its terms,
    /// and `None` when it listed them inline. Held so the failure reason can name it
    /// and an update can replace it.
    pub file_uri: Option<String>,
    /// `VocabularyState`: [`VOCABULARY_STATE_PENDING`], [`VOCABULARY_STATE_READY`]
    /// or [`VOCABULARY_STATE_FAILED`].
    pub state: String,
    /// `FailureReason`, set only on a `FAILED` vocabulary.
    pub failure_reason: Option<String>,
    /// The `FailureReason` the clock owes this vocabulary INSTEAD of a `READY` state,
    /// established when the create or update resolved the file its terms were to come
    /// from ([`resolve_vocabulary_file`]); `None` for a build that loaded its terms.
    /// Processing is what fails, so the record is accepted and reports this only once
    /// the clock has finished with it ([`Self::advance_clock`]).
    pub failure_due: Option<String>,
    /// When the build's own transition falls due — `PENDING` -> `READY` for a build
    /// that loaded its terms, `PENDING` -> `FAILED` for one whose file the object store
    /// did not serve; `None` once it has been materialised. A `PENDING` record always
    /// carries it.
    pub transition_due: Option<SystemTime>,
    pub last_modified_time: SystemTime,
    /// The region the request that created or last updated it was signed for; the
    /// service-managed `DownloadUri` is in that region.
    pub region: String,
    pub data_access_role_arn: Option<String>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub tags: Vec<Tag>,
}

/// The `VocabularyState` a custom vocabulary's clock-owed build settles in, with the
/// `FailureReason` a failed one carries ("If `VocabularyState` is `FAILED`,
/// `FailureReason` contains information about why the custom vocabulary request
/// failed").
///
/// THE RULE both vocabulary families settle by, single-homed so the two cannot
/// disagree: a build that loaded its terms — from the list the request carried or from
/// the file the object store served — has produced the vocabulary and is `READY`, which
/// is the state that makes it usable ("If the state is READY, you can use the custom
/// vocabulary in a StartTranscriptionJob request"), while a build that loaded none ends
/// in the service's own modelled failure state carrying the reason the store's answer
/// grounded ([`vocabulary_file_not_read_reason`]).
fn settled_build(failure_due: Option<&str>) -> (&'static str, Option<String>) {
    match failure_due {
        None => (VOCABULARY_STATE_READY, None),
        Some(reason) => (VOCABULARY_STATE_FAILED, Some(reason.to_string())),
    }
}

impl VocabularyRecord {
    /// Materialise the state transition the CLOCK already owes this vocabulary: it
    /// was due at its stored instant whether or not anybody called. A reporting
    /// operation (`Get`, `List`) runs this before projecting the record, and that is
    /// the only write it performs.
    ///
    /// WHICH state it lands in was decided when the create or update built the
    /// vocabulary ([`settled_build`]), so a read neither reads a file nor changes what
    /// the next read sees. The medical family's single transition settles the same way
    /// ([`MedicalVocabularyRecord::advance_clock`]).
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.state != VOCABULARY_STATE_PENDING {
            return;
        }
        if let Some(due) = self.transition_due.filter(|due| now >= *due) {
            let (state, failure_reason) = settled_build(self.failure_due.as_deref());
            self.state = state.to_string();
            self.failure_reason = failure_reason;
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

    /// Where this vocabulary's own file is reported to live, for one this service has
    /// FINISHED building — and `None` for every other record.
    ///
    /// The reported location is that of the BUILT vocabulary, so it exists only once
    /// the build that produces it has completed successfully: [`VOCABULARY_STATE_READY`]
    /// is the state the model gives that outcome, and while a build is still
    /// [`VOCABULARY_STATE_PENDING`] — or after it ended
    /// [`VOCABULARY_STATE_FAILED`] — there is nothing built to download and the member
    /// is absent. That one condition also covers where the terms came from: a
    /// vocabulary whose file the object store did not serve loaded no terms and never
    /// reaches `READY` ([`Self::advance_clock`]). The medical
    /// family gates the same member on the same state
    /// ([`MedicalVocabularyRecord::download_uri`]). See [`vocabulary_download_uri`].
    pub fn download_uri(&self) -> Option<String> {
        (self.state == VOCABULARY_STATE_READY)
            .then(|| vocabulary_download_uri(RESOURCE_TYPE_VOCABULARY, &self.name, &self.region))
    }
}

/// One stored custom MEDICAL vocabulary: the file the request named as the source of
/// its terms, the terms that file holds, plus the lifecycle facts the service itself
/// owns (its processing state, why that state is what it is, and when it was last
/// modified).
///
/// HOW THIS FAMILY IS BUILT: a custom medical vocabulary's terms can only come from the
/// table file at `VocabularyFileUri`, which `CreateMedicalVocabulary` and
/// `UpdateMedicalVocabulary` both model `@required` with no inline alternative
/// (`smithy_query(query="operation", operation="CreateMedicalVocabulary")`) — unlike
/// `CreateVocabulary`, which also takes the terms in `Phrases`. That file is an Amazon
/// S3 object the caller uploaded before calling ("you must first upload a text file
/// that contains your vocabulary table into an Amazon S3 bucket"), and this emulator
/// holds an Amazon S3 of its own, so the file is READ from it in process
/// ([`resolve_vocabulary_file`]) and the table's lines are the terms the build
/// produces. The build therefore ends the way the object store's answer decides:
/// `PENDING` at the create, then [`VOCABULARY_STATE_READY`] — with those terms stored
/// at the `DownloadUri` the reader reports ([`store_vocabulary_content`]) — for a file
/// the store served, and [`VOCABULARY_STATE_FAILED`] with a grounded `FailureReason`
/// only for one it did not ([`vocabulary_file_not_read_reason`]). A NON-medical
/// vocabulary whose request named a file instead of listing its terms is built from
/// exactly the same read ([`VocabularyRecord::advance_clock`]).
#[derive(Debug, Clone)]
pub struct MedicalVocabularyRecord {
    /// `VocabularyName`, with the Region, this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor
    /// `ListMedicalVocabularies` pages with.
    pub sequence: u64,
    /// `LanguageCode`: the one language this vocabulary's terms are in.
    pub language_code: String,
    /// `VocabularyFileUri`: where the terms are read from. Held so the failure reason
    /// can name it and an update can replace it.
    pub file_uri: String,
    /// The terms this medical vocabulary holds: the lines of the table file the object
    /// store served — EMPTY for a build whose file it did not serve, which loaded none.
    pub terms: Vec<String>,
    /// `VocabularyState`: [`VOCABULARY_STATE_PENDING`] until the clock makes the
    /// build due, then the state that build settles in.
    pub state: String,
    /// `FailureReason`, set only on a `FAILED` vocabulary.
    pub failure_reason: Option<String>,
    /// The `FailureReason` the clock owes this vocabulary INSTEAD of a `READY` state,
    /// established when the create or update resolved its table file
    /// ([`resolve_vocabulary_file`]); `None` for a build that loaded its terms.
    pub failure_due: Option<String>,
    /// When the build's own transition falls due; `None` once it has been materialised.
    /// A `PENDING` record always carries it.
    pub transition_due: Option<SystemTime>,
    pub last_modified_time: SystemTime,
    /// The region the request that created or last updated it was signed for.
    pub region: String,
    pub tags: Vec<Tag>,
}

impl MedicalVocabularyRecord {
    /// The record a create of `name` from the file at `file_uri` stores: `PENDING`,
    /// with its one clock-owed transition already due at a fixed span from `now`, the
    /// `terms` the read of that file produced and the reason the build owes instead of
    /// a `READY` state when it produced none ([`resolve_vocabulary_file`]).
    ///
    /// Single-homed so the create and any later update cannot disagree about the
    /// invariant that a `PENDING` record always carries the instant its transition
    /// falls due.
    #[allow(clippy::too_many_arguments)]
    pub fn pending(
        name: String,
        sequence: u64,
        language_code: String,
        file_uri: String,
        terms: Vec<String>,
        failure_due: Option<String>,
        region: String,
        tags: Vec<Tag>,
        now: SystemTime,
    ) -> Self {
        Self {
            name,
            sequence,
            language_code,
            file_uri,
            terms,
            state: VOCABULARY_STATE_PENDING.to_string(),
            failure_reason: None,
            failure_due,
            transition_due: Some(now + VOCABULARY_TRANSITION_DELAY),
            last_modified_time: now,
            region,
            tags,
        }
    }

    /// Materialise the state transition the CLOCK already owes this medical
    /// vocabulary: it was due at its stored instant whether or not anybody called. A
    /// reporting operation (`Get`, `List`) runs this before projecting the record,
    /// and that is the only write it performs.
    ///
    /// WHICH state it lands in was decided when the create or update read the table
    /// file ([`settled_build`]), the same rule the non-medical family settles by, so a
    /// read neither reads a file nor changes what the next read sees.
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.state != VOCABULARY_STATE_PENDING {
            return;
        }
        if let Some(due) = self.transition_due.filter(|due| now >= *due) {
            let (state, failure_reason) = settled_build(self.failure_due.as_deref());
            self.state = state.to_string();
            self.failure_reason = failure_reason;
            // The resource changed when the build ended, so the modification instant
            // is that of the transition, not of the request that started it.
            self.last_modified_time = due;
            self.transition_due = None;
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
    /// service FINISHED building — and `None` for every other record: a build still
    /// `PENDING` has produced nothing to download yet, and one that ended `FAILED`
    /// loaded no terms at all, so the member stays absent rather than naming a location
    /// holding nothing. The non-medical family gates the same member on the same state
    /// ([`VocabularyRecord::download_uri`]). See [`vocabulary_download_uri`].
    pub fn download_uri(&self) -> Option<String> {
        (self.state == VOCABULARY_STATE_READY).then(|| {
            vocabulary_download_uri(RESOURCE_TYPE_MEDICAL_VOCABULARY, &self.name, &self.region)
        })
    }
}

/// The member of a create or update that names the file a custom vocabulary's terms are
/// read from, as a refusal or a failure reason spells it.
pub const VOCABULARY_FILE_MEMBER: &str = "VocabularyFileUri";

/// See [`VOCABULARY_FILE_MEMBER`]: the member the vocabulary FILTER family names its
/// own file with.
pub const VOCABULARY_FILTER_FILE_MEMBER: &str = "VocabularyFilterFileUri";

/// What this emulator's own Amazon S3 answered about the file a request named as the
/// source of a custom vocabulary's, medical vocabulary's or vocabulary filter's terms
/// (see [`resolve_vocabulary_file`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VocabularyFileVerdict {
    /// The store served the file: these are the terms it holds, in the order it lists
    /// them.
    Read(Vec<String>),
    /// The object store ITSELF answered that there is no object at that address, or the
    /// URI is of the Amazon S3 scheme and addresses no object of it at all
    /// ([`s3_uri_addresses_no_object`]): the request named a file that is not there, so
    /// it is refused and nothing is recorded ([`URI_NAMES_NO_OBJECT`]).
    NoSuchObject,
    /// The file holds more bytes than a custom vocabulary may
    /// ([`MAX_VOCABULARY_CONTENT_BYTES`]), so it is refused rather than built from —
    /// the request's own modelled `LimitExceededException`, whose documentation covers
    /// exactly this ("You've either sent too many requests or your input file is too
    /// long ... use a smaller file and try your request again"). Nothing of it is
    /// stored.
    TooLarge(usize),
    /// The terms were NOT read, and the store did not answer that the object is absent:
    /// it refused the read, there was no object store to ask, the location is not one an
    /// object store can be asked about, or the object it served holds no terms this
    /// service could build from. The text is the `FailureReason` the record carries once
    /// its build ends ([`vocabulary_file_not_read_reason`]).
    NotRead(String),
}

/// Why a create or update that names a file is REFUSED outright rather than recorded:
/// what [`VocabularyFileVerdict::into_build`] hands back instead of a build.
///
/// Each operation answers it with its own modelled error — the request named a file
/// that is not there, or one holding more than a custom vocabulary may — so the
/// decision is single-homed here and only the error channel differs between the six.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocabularyFileRefusal {
    /// The object store answered that nothing is at that address
    /// ([`VocabularyFileVerdict::NoSuchObject`]): a `BadRequestException` carrying
    /// [`URI_NAMES_NO_OBJECT`].
    NoSuchObject,
    /// The file holds this many bytes, past the published ceiling: a
    /// `LimitExceededException` carrying [`vocabulary_file_too_large_message`].
    TooLarge(usize),
}

impl VocabularyFileVerdict {
    /// What a create or update records from this verdict: the terms the build loaded
    /// and the `FailureReason` the clock owes the record instead of a `READY` state —
    /// or the [`VocabularyFileRefusal`] that makes the request fail before anything is
    /// recorded.
    ///
    /// Single-homed so the six operations that take a file (the two vocabulary
    /// operations, the two medical ones and the two filter ones) cannot disagree about
    /// what a verdict means for the record they store.
    pub fn into_build(self) -> Result<(Vec<String>, Option<String>), VocabularyFileRefusal> {
        match self {
            Self::Read(terms) => Ok((terms, None)),
            Self::NotRead(reason) => Ok((Vec::new(), Some(reason))),
            Self::NoSuchObject => Err(VocabularyFileRefusal::NoSuchObject),
            Self::TooLarge(bytes) => Err(VocabularyFileRefusal::TooLarge(bytes)),
        }
    }
}

/// Read the file a request named as the source of a custom vocabulary's (or vocabulary
/// filter's) terms from this emulator's own Amazon S3, and report what that read
/// established.
///
/// A caller uploads that file BEFORE calling — "you must first upload a text file that
/// contains your vocabulary table into an Amazon S3 bucket" — so the file is an object
/// of the object store, and this emulator holds an Amazon S3 of its own: it is read the
/// way any emulated service reaches another, through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`], see [`s3_client`] — no
/// socket, no outbound request, no host resolution). The read happens when the resource
/// is CREATED or UPDATED, which is when the service's build reads it, and what it
/// established is stored on the record, so the clock-owed transition settles on it
/// without any read having to resolve anything ([`settled_build`]).
///
/// The address is RESOLVED as part of validating the request: a location the object
/// store itself answers for with an absent object ([`ABSENT_OBJECT_CODES`], the same
/// list the media resolution refuses on), or an `s3://` URI that addresses no object at
/// all ([`s3_uri_addresses_no_object`]), names a file that is not there, so the request
/// is refused ([`VocabularyFileVerdict::NoSuchObject`]) and no resource is created or
/// rebuilt. Everything else keeps the build: a refusal has to be GROUNDED in what the
/// store answered, so an answer this emulator could not get (no object store to ask, a
/// location no object store can be asked about) and an object whose CONTENT yields no
/// terms leave the record to the one transition the clock owes it, carrying what this
/// emulator established about the file ([`vocabulary_file_not_read_reason`]) — unlike a
/// custom language model's training data, which only has to be there
/// ([`resolve_training_data`]), the terms a built vocabulary holds are its content, so a
/// file nothing read leaves the build with nothing to produce.
///
/// HOW MUCH IS READ IS BOUNDED, and bounded TWICE, because the size of this read is the
/// one thing a caller chooses: the published ceiling a custom vocabulary may hold
/// ([`MAX_VOCABULARY_CONTENT_BYTES`]) is checked against the length the store's own
/// answer declares — which an object read always carries — BEFORE a byte of the body is
/// taken into this process, and again against the bytes themselves, so an answer that
/// declared no length cannot get past it either. A file over the ceiling is refused
/// ([`VocabularyFileVerdict::TooLarge`]) rather than truncated, so nothing is built from
/// part of a file.
pub async fn resolve_vocabulary_file(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    member: &str,
    uri: &str,
) -> VocabularyFileVerdict {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    // `Uri`'s own `@pattern` admits an `http(s)://` location too, which addresses a
    // HOST rather than an object, and this emulator resolves no hosts: there is no
    // object store to ask about it. A URI of the S3 scheme that addresses no object of
    // it needs no store to judge, exactly as a media location does not
    // ([`s3_uri_addresses_no_object`]): it names no file for any store to hold.
    let Some((bucket, key)) = s3_object_address(uri) else {
        if s3_uri_addresses_no_object(uri) {
            return VocabularyFileVerdict::NoSuchObject;
        }
        return VocabularyFileVerdict::NotRead(vocabulary_file_not_read_reason(
            member,
            uri,
            NOT_AN_OBJECT_DETAIL,
        ));
    };
    let client = s3_client(ctx, region);
    let read = match client.get_object().bucket(bucket).key(key).send().await {
        Ok(read) => read,
        Err(error) => {
            // S3 itself saying it holds nothing at that address is what makes the file
            // one the request may not name: the vocabulary is not created and the
            // caller is told so synchronously.
            if object_is_absent(&error) {
                return VocabularyFileVerdict::NoSuchObject;
            }
            let code = error.code().unwrap_or_default();
            if location_is_unreadable(code) {
                // The store answered about the file without saying it is absent — it
                // refused the read — so the failure is grounded in that answer and the
                // build is what carries it.
                return VocabularyFileVerdict::NotRead(vocabulary_file_not_read_reason(
                    member,
                    uri,
                    &format!("Amazon S3 answered {code}"),
                ));
            }
            // Any other answer is this emulator failing to ask its own object store,
            // not the store reporting an unreadable file — and it is worth a warning,
            // because a deployment with no S3 mounted to ask is one where no file is
            // ever read. The caller's own value is rendered with its escaped `Debug`
            // form, so a URI carrying a newline cannot forge a log line.
            ::tracing::warn!(
                member = member,
                uri = ?uri,
                code = ?error.code(),
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a custom vocabulary's file could not be read from the emulator's Amazon S3"
            );
            return VocabularyFileVerdict::NotRead(vocabulary_file_not_read_reason(
                member,
                uri,
                NO_ANSWER_DETAIL,
            ));
        }
    };
    // The published ceiling is checked against what the store SAYS the file holds,
    // before any of it is read into this process.
    if let Some(declared) = read
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
    {
        if declared > MAX_VOCABULARY_CONTENT_BYTES {
            return VocabularyFileVerdict::TooLarge(declared);
        }
    }
    let Ok(body) = read.body.collect().await else {
        return VocabularyFileVerdict::NotRead(vocabulary_file_not_read_reason(
            member,
            uri,
            NO_ANSWER_DETAIL,
        ));
    };
    let body = body.into_bytes();
    // The same ceiling again, now against the bytes themselves: a store that declared
    // no length cannot smuggle a file past it.
    if body.len() > MAX_VOCABULARY_CONTENT_BYTES {
        return VocabularyFileVerdict::TooLarge(body.len());
    }
    let Ok(text) = std::str::from_utf8(&body) else {
        return VocabularyFileVerdict::NotRead(vocabulary_file_not_read_reason(
            member,
            uri,
            NOT_TEXT_DETAIL,
        ));
    };
    let terms = vocabulary_file_terms(text);
    if terms.is_empty() {
        return VocabularyFileVerdict::NotRead(vocabulary_file_not_read_reason(
            member,
            uri,
            NO_TERMS_DETAIL,
        ));
    }
    ::tracing::info!(
        member = member,
        bucket = ?bucket,
        key = ?key,
        terms = terms.len(),
        "a custom vocabulary's file was read from the emulator's Amazon S3"
    );
    VocabularyFileVerdict::Read(terms)
}

/// The terms the text file of a custom vocabulary, medical vocabulary or vocabulary
/// filter holds: its non-empty lines, in the order the file lists them.
///
/// One entry per line is the form every member that names such a file documents — a
/// list of "entries, phrases, and terms", a vocabulary table, or the words a filter
/// masks — and it is the same form this service WRITES the built file in
/// ([`vocabulary_content_file`]), so a file this emulator built round-trips through a
/// create that names it. A table file's line carries its columns, which travel with the
/// line; blank lines separate nothing and are dropped.
pub fn vocabulary_file_terms(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Why a build loaded no terms, when the location is not an Amazon S3 object this
/// emulator can read: see [`vocabulary_file_not_read_reason`].
const NOT_AN_OBJECT_DETAIL: &str = "it does not address an Amazon S3 object";
/// Why a build loaded no terms, when no object store answered for the location at all.
const NO_ANSWER_DETAIL: &str = "no Amazon S3 answered for that location";
/// Why a build loaded no terms, when the object the store served is not text.
const NOT_TEXT_DETAIL: &str = "it is not a UTF-8 text file";
/// Why a build loaded no terms, when the text the store served holds none.
const NO_TERMS_DETAIL: &str = "it holds no terms";

/// Why a custom vocabulary this emulator recorded ends `FAILED`: the file its terms
/// were to come from was not read, and `detail` is what this emulator established about
/// it — the code Amazon S3 answered, or the reason there was no answer to go on (see
/// [`resolve_vocabulary_file`]).
///
/// The wording is this emulator's own — AWS publishes no text for a custom vocabulary's
/// `FailureReason` — and it names the member and the URI the request gave, so a caller
/// can see which file was not read and why. One text for all three families, because
/// the reason they fail is the same one.
pub fn vocabulary_file_not_read_reason(member: &str, uri: &str, detail: &str) -> String {
    format!(
        "The {member} {uri} could not be read: {detail}. No terms were loaded, so there \
         was nothing to build from. Upload your file and try your request again."
    )
}

/// The refusal a create or update carries when the file it names holds more than the
/// published ceiling a custom vocabulary of `kind` may ([`MAX_VOCABULARY_CONTENT_BYTES`]).
///
/// The same ceiling, and the same remedy, as the refusal a request carrying its terms
/// inline gets — "use a smaller file and try your request again"
/// (`LimitExceededException`'s own documentation).
pub fn vocabulary_file_too_large_message(member: &str, uri: &str, bytes: usize, kind: &str) -> String {
    format!(
        "The {member} {uri} is {bytes} bytes, more than the {max} bytes a custom {kind} \
         may hold. Use a smaller file and try your request again.",
        max = MAX_VOCABULARY_CONTENT_BYTES,
    )
}

/// One stored custom language model: the base model, language and training-data
/// location a create named, plus the lifecycle facts the service itself owns (its
/// training status and its timestamps).
///
/// WHAT IS REAL HERE: the record, and the one thing the training run has to establish
/// about the data it is given — whether that data is THERE. A custom language model's
/// training and tuning data come from the Amazon S3 locations at
/// `InputDataConfig$S3Uri` and `InputDataConfig$TuningDataS3Uri`, and this emulator's
/// own Amazon S3 answers for them in process ([`resolve_training_data`]), so a create
/// resolves them exactly as the service's training run would. Nothing of a model's
/// content is reported anywhere — the `LanguageModel` shape carries the name, the
/// timestamps, the language, the base model, the status, the upgrade availability, the
/// failure reason, the input data config and the encryption configuration, and no
/// member of any operation of this family reports a trained artifact — so a model
/// whose training data the object store answers for finishes:
/// [`MODEL_STATUS_IN_PROGRESS`] at the create, then [`MODEL_STATUS_COMPLETED`] once
/// the clock makes the transition due, which is the status that makes the model usable
/// ("When the status displays as COMPLETED, your model is ready to use" — the model's
/// own doc for `ModelStatus`). A model settles in [`MODEL_STATUS_FAILED`], with the
/// `FailureReason` the store's answer grounds, only where that answer says a location
/// cannot be read: nothing is failed on an assumption, so a location no object store
/// answered for at all fails nothing ([`TrainingDataVerdict`]).
#[derive(Debug, Clone)]
pub struct LanguageModelRecord {
    /// `ModelName`, with the Region, this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor a listing of this family
    /// would page with.
    pub sequence: u64,
    /// `LanguageCode`: the one language this model's terms are in.
    pub language_code: String,
    /// `BaseModelName`: the standard Amazon Transcribe model this one was built on.
    pub base_model_name: String,
    /// `InputDataConfig`: where the training (and optionally tuning) data is read
    /// from, and the role that would read it. Held so every reader can report it and
    /// so a failure reason can name the location that could not be read.
    pub input_data_config: InputDataConfig,
    /// `EncryptionConfiguration`, as the create named it, or `None`.
    pub encryption_configuration: Option<EncryptionConfiguration>,
    /// `ModelStatus`: [`MODEL_STATUS_IN_PROGRESS`] until the clock makes the training
    /// due, then the status [`Self::training_data`] settles on.
    pub status: String,
    /// `FailureReason`, set only on a `FAILED` model.
    pub failure_reason: Option<String>,
    /// What the object store answered about this model's training and tuning data when
    /// the create resolved them — which is what the clock-owed transition settles on.
    pub training_data: TrainingDataVerdict,
    /// When the `IN_PROGRESS` -> terminal transition falls due; `None` once it has
    /// been materialised.
    pub settles_due: Option<SystemTime>,
    /// `CreateTime`: when the create that stored this record was served.
    pub create_time: SystemTime,
    /// `LastModifiedTime`: when the record last changed.
    pub last_modified_time: SystemTime,
    pub tags: Vec<Tag>,
}

impl LanguageModelRecord {
    /// The record a create of `name` stores: `IN_PROGRESS`, with its one clock-owed
    /// transition already due at a fixed span from `now` and the verdict that
    /// transition settles on — what the object store answered about the training data
    /// the request named ([`resolve_training_data`]).
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
        training_data: TrainingDataVerdict,
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
            training_data,
            settles_due: Some(now + MODEL_TRANSITION_DELAY),
            create_time: now,
            last_modified_time: now,
            tags,
        }
    }

    /// Materialise the status transition the CLOCK already owes this model: it was due
    /// at its stored instant whether or not anybody called. A reporting operation
    /// (`DescribeLanguageModel`, a listing) runs this before projecting the record,
    /// and that is the only write it performs.
    ///
    /// WHICH status it moves to was decided when the create resolved the training data
    /// ([`TrainingDataVerdict::settles`]), so a read neither resolves anything nor
    /// changes what the next read sees.
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.status != MODEL_STATUS_IN_PROGRESS {
            return;
        }
        if let Some(due) = self.settles_due.filter(|due| now >= *due) {
            let (status, failure_reason) = self.training_data.settles();
            self.status = status.to_string();
            self.failure_reason = failure_reason;
            // The resource changed when the training ended, so the modification
            // instant is that of the transition, not of the request that started it.
            self.last_modified_time = due;
            self.settles_due = None;
        }
    }

    /// Re-encrypt this model's artifacts in place, the one change
    /// `UpdateLanguageModel` makes to a stored model.
    ///
    /// "The model artifacts are re-encrypted in place using the specified KMS key or
    /// with an AWS-owned key if a key is not supplied" (the model's doc for
    /// `UpdateLanguageModelRequest$EncryptionConfiguration`), so the configuration the
    /// request carries REPLACES the stored one and a request that carries none leaves
    /// the artifacts under the AWS-owned key — which this record spells as no
    /// configuration of its own, exactly as a create that named none does.
    /// `DataAccessRoleArn` is optional on that request, so a request naming no role
    /// keeps the one the model already reads its data with.
    ///
    /// The re-encryption is work the service performs AFTER answering — "You cannot
    /// submit another update while a previous update is in progress. Use [the
    /// describe operation] to check the current state of your model" (the operation's
    /// own doc) — so the record goes back to [`MODEL_STATUS_IN_PROGRESS`] carrying the
    /// instant its one clock-owed transition falls due, the same invariant
    /// [`Self::in_progress`] establishes for a create. "No model training is
    /// required" (same doc): nothing resolves the training data again, so the status
    /// this record settles back in is the one that data already decided
    /// ([`TrainingDataVerdict`]) and an update cannot change which terminal status a
    /// model ends in.
    pub fn re_encrypt(
        &mut self,
        data_access_role_arn: Option<String>,
        encryption_configuration: Option<EncryptionConfiguration>,
        now: SystemTime,
    ) {
        self.encryption_configuration = encryption_configuration;
        if let Some(role) = data_access_role_arn {
            self.input_data_config.data_access_role_arn = role;
        }
        self.status = MODEL_STATUS_IN_PROGRESS.to_string();
        // A model the service is working on reports no failure yet; the one its
        // training data grounds is set again when the transition falls due
        // ([`Self::advance_clock`]).
        self.failure_reason = None;
        self.settles_due = Some(now + MODEL_TRANSITION_DELAY);
        self.last_modified_time = now;
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

/// What the object store answered about the data a custom language model's training
/// run reads (see [`LanguageModelRecord`]).
///
/// The verdict is read from Amazon S3's own answer and nothing else: a refusal has to
/// be GROUNDED in what the store said, so a location no store answered for at all is
/// [`Self::Unknown`] rather than a failure, and nothing is failed on an assumption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrainingDataVerdict {
    /// Every location the create named holds objects to train from.
    Readable,
    /// The store answered that a location cannot be read — nothing is stored under it,
    /// the bucket is not there, or the read is not permitted. The text is the
    /// `FailureReason` the model carries once its training ends
    /// ([`training_data_unreadable_reason`]).
    Unreadable(String),
    /// Nothing is known about the data: there was no object store to ask, or the
    /// location is not one an object store can be asked about.
    Unknown,
}

impl TrainingDataVerdict {
    /// The `ModelStatus` a model whose training data this verdict is about settles in,
    /// with the `FailureReason` a failed one carries ("If `ModelStatus` is `FAILED`,
    /// `FailureReason` contains information about why the custom language model request
    /// failed" — the model's doc for `LanguageModel$FailureReason`).
    ///
    /// Nothing of a model's content is reported by any operation of this family, so a
    /// training run that was given data has nothing left to establish and the model is
    /// ready to use; only a location the store refused ends the run in the modelled
    /// failure state. [`Self::Unknown`] settles the same way as [`Self::Readable`] —
    /// deliberately, because it is the absence of an answer and not evidence that the
    /// data is missing: a failure this emulator could not ground would refuse a caller
    /// the real service serves.
    pub fn settles(&self) -> (&'static str, Option<String>) {
        match self {
            Self::Readable | Self::Unknown => (MODEL_STATUS_COMPLETED, None),
            Self::Unreadable(reason) => (MODEL_STATUS_FAILED, Some(reason.clone())),
        }
    }
}

/// The member of `InputDataConfig` that names a model's training data.
pub const TRAINING_DATA_MEMBER: &str = "InputDataConfig.S3Uri";
/// The member of `InputDataConfig` that names a model's tuning data.
pub const TUNING_DATA_MEMBER: &str = "InputDataConfig.TuningDataS3Uri";

/// Resolve the data a custom language model would be trained and tuned from against
/// this emulator's own Amazon S3, and report what that read established.
///
/// The locations are resolved when the model is CREATED, which is when the service's
/// training run reads them, and the verdict is stored on the record so the clock-owed
/// transition settles on it without any read having to resolve anything
/// ([`LanguageModelRecord::advance_clock`]). S3 is reached the way any emulated
/// service reaches another: through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`], which dispatches inside
/// this process — see [`s3_client`]: no socket, no outbound request, no host
/// resolution).
///
/// Both locations are resolved, in the order the operation documents them: training
/// data is `@required` and tuning data optional, and a run cannot tune from data that
/// is not there either, so the FIRST location the store refuses is the one the failure
/// reason names. A location the store refuses nothing about contributes no failure.
pub async fn resolve_training_data(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    input_data_config: &InputDataConfig,
) -> TrainingDataVerdict {
    let client = s3_client(ctx, region);
    let mut verdict = TrainingDataVerdict::Readable;
    for (member, uri) in [
        (TRAINING_DATA_MEMBER, Some(input_data_config.s3_uri.as_str())),
        (
            TUNING_DATA_MEMBER,
            input_data_config.tuning_data_s3_uri.as_deref(),
        ),
    ] {
        let Some(uri) = uri else { continue };
        match read_training_location(&client, member, uri).await {
            TrainingDataVerdict::Readable => {}
            // The first refusal decides: it is the one the model reports.
            unreadable @ TrainingDataVerdict::Unreadable(_) => return unreadable,
            TrainingDataVerdict::Unknown => verdict = TrainingDataVerdict::Unknown,
        }
    }
    verdict
}

/// What Amazon S3 answers about the objects under ONE location of a create's input
/// data config.
///
/// The read is a LISTING, not a fetch: a training location is a prefix holding the
/// text files a model is trained from ("Here's an example URI path:
/// `s3://DOC-EXAMPLE-BUCKET/my-model-training-data/`" — the model's doc for
/// `InputDataConfig$S3Uri`), so what the run has to establish is that the prefix holds
/// files at all, and no byte of them is read or stored: no member of this family
/// reports anything derived from a model's training data.
///
/// A key ending in the delimiter is a folder marker rather than a file, so a location
/// holding only markers holds no training data. The walk therefore stops at the FIRST
/// file it sees, and pages on only while every key so far has been a marker and the
/// listing says more keys follow — so the answer does not depend on how many markers a
/// caller staged ahead of its files, while the number of pages a create reads is
/// bounded by [`MAX_TRAINING_DATA_PAGES`] whatever is under the prefix.
async fn read_training_location(
    client: &::aws_sdk_s3::Client,
    member: &str,
    uri: &str,
) -> TrainingDataVerdict {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    // `Uri`'s own `@pattern` admits an `http(s)://` location too, which is not an
    // address this emulator asks an object store about: nothing is known about it.
    let Some((bucket, prefix)) = s3_bucket_and_path(uri) else {
        return TrainingDataVerdict::Unknown;
    };
    let mut continuation: Option<String> = None;
    for _ in 0..MAX_TRAINING_DATA_PAGES {
        match client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .set_continuation_token(continuation.take())
            .send()
            .await
        {
            Ok(page) => {
                let holds_data = page
                    .contents()
                    .iter()
                    .any(|object| object.key().is_some_and(|key| !key.ends_with('/')));
                if holds_data {
                    return TrainingDataVerdict::Readable;
                }
                match page.next_continuation_token() {
                    // Only markers so far, and the store says there are more keys:
                    // the files may be behind them.
                    Some(token) => continuation = Some(token.to_string()),
                    // The store has shown every key under the location and not one of
                    // them is a file.
                    None => {
                        return TrainingDataVerdict::Unreadable(training_data_absent_reason(
                            member, uri,
                        ))
                    }
                }
            }
            Err(error) => {
                let code = error.code().unwrap_or_default();
                if location_is_unreadable(code) {
                    // The store says the location is not there, or not readable by this
                    // account: the failure is grounded in its answer.
                    return TrainingDataVerdict::Unreadable(training_data_unreadable_reason(
                        member, uri, code,
                    ));
                }
                // Any other answer is this emulator failing to ask its own object store,
                // not the store reporting an unreadable location, so it fails nothing —
                // and it is worth a warning, because a deployment with no S3 mounted to
                // ask is one where this resolution does not run. The caller's own value
                // is rendered with its escaped `Debug` form, so a URI carrying a newline
                // cannot forge a log line.
                ::tracing::warn!(
                    member = member,
                    uri = ?uri,
                    code = ?error.code(),
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a custom language model's data could not be resolved against the \
                     emulator's Amazon S3"
                );
                return TrainingDataVerdict::Unknown;
            }
        }
    }
    // Every key the walk was allowed to look at is a folder marker and more follow:
    // whether a file sits behind them is not something this read established, so it
    // grounds no failure.
    ::tracing::warn!(
        member = member,
        uri = ?uri,
        "a custom language model's data holds more than the folder markers this \
         emulator walks, so whether it holds files was not established"
    );
    TrainingDataVerdict::Unknown
}

/// How many listing pages one location of a create's input data config is walked for
/// before the read gives up (see [`read_training_location`]).
///
/// The walk only pages while every key it has seen is a folder marker, so this bounds
/// the pathological case — a location staged with nothing but markers — rather than the
/// ordinary one, which the first page answers. Each page is one in-process listing of
/// at most the object store's own page size, so a create reads a bounded number of them
/// however much a caller staged.
const MAX_TRAINING_DATA_PAGES: usize = 8;

/// Whether `code` is Amazon S3 saying a location a create or update named cannot be
/// READ: either one of the codes in which it reports that an address holds nothing
/// ([`ABSENT_OBJECT_CODES`], the same list the media resolution refuses on) or
/// one of the codes in which it reports that the read is not permitted
/// ([`FORBIDDEN_READ_CODES`]). Every other answer is this emulator failing to ask,
/// which names no store answer (see [`read_training_location`] and
/// [`resolve_vocabulary_file`]).
fn location_is_unreadable(code: &str) -> bool {
    ABSENT_OBJECT_CODES.contains(&code) || FORBIDDEN_READ_CODES.contains(&code)
}

/// The Amazon S3 error codes in which S3 reports that a read is NOT PERMITTED, as
/// opposed to that the address holds nothing ([`ABSENT_OBJECT_CODES`]). A custom
/// language model's training data, and a custom vocabulary's own file, are read with the
/// role the request named, so "you may not read this" is as much a grounded reason for
/// the build to fail as "there is nothing there".
const FORBIDDEN_READ_CODES: &[&str] = &["AccessDenied", "AllAccessDisabled"];

/// Why a custom language model ends `FAILED` when the object store holds nothing under
/// a location the create named: there is no data to train (or tune) from.
///
/// The wording is this emulator's own — AWS publishes no text for a custom language
/// model's `FailureReason` — and it names the member and the location the store
/// answered for, so a caller can see which of the two locations was empty.
pub fn training_data_absent_reason(member: &str, uri: &str) -> String {
    format!(
        "The {member} {uri} holds no files: Amazon S3 reports no object under that \
         location, so there was nothing to train this custom language model from. \
         Upload your data and create the model again."
    )
}

/// Why a custom language model ends `FAILED` when the object store refuses a location
/// the create named: `code` is the answer S3 gave, so the reason says which location
/// could not be read and what the store said about it. See
/// [`training_data_absent_reason`].
pub fn training_data_unreadable_reason(member: &str, uri: &str, code: &str) -> String {
    format!(
        "The {member} {uri} could not be read: Amazon S3 answered {code}. Check that \
         the location exists and that the DataAccessRoleArn you specified has \
         permission to read it, then create the model again."
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
/// `VocabularyFilterFileUri` — and EITHER produces the same record, exactly as in the
/// vocabulary family ([`VocabularyRecord`]): the words the request listed, or the words
/// the file the object store served holds ([`resolve_vocabulary_file`]). What the build
/// loaded decides only what this record can report: a filter whose file the store did
/// not serve has no words loaded and so no file of its own to download
/// ([`Self::download_uri`]).
#[derive(Debug, Clone)]
pub struct VocabularyFilterRecord {
    /// `VocabularyFilterName`, with the Region, this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor `ListVocabularyFilters`
    /// pages with.
    pub sequence: u64,
    pub language_code: String,
    /// The words this filter masks, deletes or flags: the list the request carried in
    /// `Words`, or the lines of the file at `VocabularyFilterFileUri` the object store
    /// served — EMPTY only for a filter whose file it did not serve, which loaded none.
    pub words: Vec<String>,
    /// `VocabularyFilterFileUri` when the request named a file as the source of its
    /// words, and `None` when it listed them inline. Held so an update can replace it.
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

    /// Where this filter's own file is reported to live, for one this service BUILT —
    /// and `None` for one whose words the object store did not serve, which loaded none:
    /// a filter no words were loaded for has no file to download, exactly as in the
    /// vocabulary family ([`VocabularyRecord::download_uri`]). This family models no
    /// processing state at all, so the words the record holds are what says whether
    /// there is a built file at that location. See [`vocabulary_download_uri`].
    pub fn download_uri(&self) -> Option<String> {
        (!self.words.is_empty()).then(|| {
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
    /// `CategoryName`, with the Region, this record's key in the store.
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

/// What separates the parts of a page cursor: the scope it was minted in
/// ([`CursorScope`]) from the position within that scope, and — for a sequence cursor
/// — that position's own two halves. Part of what tells a token this service handed
/// out from one it never wrote: no name this service keys a record by can hold it
/// (every such name is modelled `pattern "^[0-9a-zA-Z._-]+$"`), and neither can the
/// Region a cursor is scoped to ([`request_region`] answers a Region identifier or
/// nothing).
const CURSOR_SEPARATOR: char = '|';

/// The longest name this service keys a record by.
///
/// Every one of those names is modelled `@length(min: 1, max: 200)`: `CategoryName`,
/// `VocabularyName`, `VocabularyFilterName`, `TranscriptionJobName`,
/// `CallAnalyticsJobName` and `ModelName` alike (`smithy_query(query="shape",
/// shape="CategoryName")`), so the boundary validator has already held every name a
/// request established a record under to this bound.
pub const MAX_RECORD_NAME_LENGTH: usize = 200;

/// Whether `name` could be a name this service keys a record by: no longer than
/// [`MAX_RECORD_NAME_LENGTH`] and spelled out of the characters alone that every one of
/// those name shapes is modelled `@pattern "^[0-9a-zA-Z._-]+$"` with.
///
/// This is what makes the POSITION a page cursor carries decidable the way its scope
/// and its sequence already are ([`parse_page_cursor`], [`parse_name_page_cursor`]):
/// every cursor of this service carries the NAME of the record it resumes after, minted
/// from a stored name and so from a name within those constraints, so a token whose
/// name part is outside them names a record no Region of this store could ever hold and
/// is a token this service never wrote — whatever else about it parses.
pub fn is_record_name(name: &str) -> bool {
    (1..=MAX_RECORD_NAME_LENGTH).contains(&name.chars().count())
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

/// The listing a page cursor was minted by and is valid for: ONE operation over ONE
/// Region's records.
///
/// A `NextToken` names a position in a total order, and the orders this service pages
/// are per listing: each sequence-paged listing reports the records of its own family,
/// ordered by a sequence that family alone mints ([`SequencedFamily`]), and every
/// listing reports only the Region its endpoint serves ([`RegionalStore`]). So a
/// position one listing handed out names nothing in another listing's order: presented
/// elsewhere it would resume below every record that listing holds (an empty page, the
/// caller told the collection is exhausted) or above all of them (the FIRST page again,
/// with a fresh cursor). Every cursor therefore carries the scope it was minted in and
/// is read back only in that same scope ([`page_cursor`], [`name_page_cursor`]), which
/// is what makes "a cursor THIS listing handed out" decidable; any other token is
/// refused ([`invalid_next_token_message`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorScope<'a> {
    /// The listing that mints and reads cursors of this scope.
    listing: &'a str,
    /// The Region whose records that listing is reporting.
    region: &'a str,
}

impl<'a> CursorScope<'a> {
    /// The scope of the listing `listing` over `region`'s records.
    pub fn new(listing: &'a str, region: &'a str) -> Self {
        Self { listing, region }
    }

    /// The scope of the one listing that reports `family`, over `region`'s records —
    /// so a sequence-paged listing names its family alone and the operation its cursors
    /// belong to follows from it ([`SequencedFamily::listing`]).
    pub fn of(family: SequencedFamily, region: &'a str) -> Self {
        Self::new(family.listing(), region)
    }

    /// The operation whose previous answer a cursor of this scope came from — what a
    /// refusal of any other token names ([`invalid_next_token_message`]).
    pub fn listing(&self) -> &'a str {
        self.listing
    }

    /// What every cursor minted in this scope begins with, and the only prefix a token
    /// read back in it may carry.
    fn prefix(&self) -> String {
        format!(
            "{listing}{CURSOR_SEPARATOR}{region}{CURSOR_SEPARATOR}",
            listing = self.listing,
            region = self.region,
        )
    }
}

/// The `NextToken` the listing `scope` hands out to resume after the record with this
/// creation sequence and name.
///
/// The token is opaque to clients: this service writes
/// `{listing}|{region}|{sequence}|{name}` and reads back the sequence
/// ([`parse_page_cursor`]) in that same scope alone. The name travels with it, which is
/// what makes a token readable in a trace; only the sequence is interpreted, so a
/// record deleted between pages does not stop the next page from resuming strictly past
/// it. Single-homed so every listing of this service pages the same way.
pub fn page_cursor(scope: CursorScope<'_>, sequence: u64, name: &str) -> String {
    format!(
        "{prefix}{sequence}{CURSOR_SEPARATOR}{name}",
        prefix = scope.prefix(),
    )
}

/// The creation sequence a `NextToken` resumes after, or `None` for a token the listing
/// `scope` never handed out — which that listing refuses rather than silently
/// restarting from the top or answering an empty page.
///
/// A token this listing wrote carries the WHOLE of what [`page_cursor`] hands out: the
/// scope that listing mints in, a creation sequence, the separator, and a name this
/// service could key a record by ([`is_record_name`]), so neither a bare number
/// carrying no name, nor a name no record of this store could be called, nor a cursor
/// another listing (or another Region's endpoint) handed out is one of this
/// listing's. Creation sequences are minted from 1 upwards, per family, by
/// [`ComAmazonawsTranscribeTranscribeState::next_sequence`], so a cursor is only one
/// this listing could have handed out when its sequence has actually been minted for
/// the family that listing reports: `minted_through` is the highest sequence this store
/// has minted for it ([`ComAmazonawsTranscribeTranscribeState::minted_sequences`]), and
/// a sequence of 0, or one past it, belongs to no record any page of that listing ever
/// reported.
///
/// Carrying the WHOLE of what [`page_cursor`] hands out is decided by re-minting it: a
/// token is one this listing wrote only when [`page_cursor`] applied to the position the
/// token decodes to writes that very token back, byte for byte. A sequence is written
/// by `u64`'s `Display`, so the one spelling the encoder ever writes is the canonical
/// decimal one, and any other spelling of the same number (a leading `+`, leading
/// zeros) is a position this listing never handed out, however well it parses.
pub fn parse_page_cursor(
    token: &str,
    scope: CursorScope<'_>,
    minted_through: u64,
) -> Option<u64> {
    let (sequence, name) = token
        .strip_prefix(scope.prefix().as_str())?
        .split_once(CURSOR_SEPARATOR)?;
    if !is_record_name(name) {
        return None;
    }
    sequence
        .parse::<u64>()
        .ok()
        .filter(|sequence| (1..=minted_through).contains(sequence))
        .filter(|sequence| page_cursor(scope, *sequence, name) == token)
}

/// The `NextToken` a listing of records keyed by NAME ALONE hands out to resume
/// strictly after `name`.
///
/// The Call Analytics categories are stored without a creation sequence — their
/// record holds no such member (see [`CallAnalyticsCategoryRecord`]) and their listing
/// is documented with no order — so `ListCallAnalyticsCategories` pages over the
/// store's own key order (the name, ascending) and its cursor carries a name rather
/// than a sequence ([`page_cursor`]). Resuming strictly past a NAME is what makes
/// paging stable under concurrent writes: a category created or deleted between two
/// pages cannot shift the position the next page resumes from. The scope it carries is
/// the sequence cursor's own ([`CursorScope`]): the names of one Region's categories
/// are the order THAT endpoint's listing pages, so a cursor minted over another
/// Region's categories names a position in no order this one pages. Single-homed here
/// beside the sequence cursor so both token formats of this service live in one place.
pub fn name_page_cursor(scope: CursorScope<'_>, name: &str) -> String {
    format!("{prefix}{name}", prefix = scope.prefix())
}

/// The record name a `NextToken` resumes after, or `None` for a token the listing
/// `scope` never handed out — which that listing refuses rather than silently
/// restarting from the top.
///
/// A token this listing wrote carries the WHOLE of what [`name_page_cursor`] hands out:
/// the scope that listing mints in, and behind it a name this service could key a record
/// by ([`is_record_name`] — `CategoryName` is modelled `@length(min: 1, max: 200)` and
/// `@pattern "^[0-9a-zA-Z._-]+$"`, and this codec only ever mints from a stored name).
/// So a scope prefix with nothing behind it, one carrying a name longer than that
/// `@length` allows, and one carrying characters that `@pattern` forbids are all tokens
/// this service never wrote, and are refused rather than honoured as a resume point —
/// just as the sequence codec refuses a position its family never minted
/// ([`parse_page_cursor`]). The name is bounded by the constraint rather than by the
/// names the Region holds right now, because resuming strictly past a name is what keeps
/// paging stable when the record that name belongs to is deleted between two pages.
pub fn parse_name_page_cursor<'t>(token: &'t str, scope: CursorScope<'_>) -> Option<&'t str> {
    token
        .strip_prefix(scope.prefix().as_str())
        .filter(|name| is_record_name(name))
}

/// What a listing refuses a `NextToken` this service never handed out with, naming
/// the `operation` whose previous answer the caller should have copied it from.
///
/// Single-homed beside the token codec it belongs to, so every listing of this
/// service refuses an alien cursor in the same words.
pub fn invalid_next_token_message(operation: &str) -> String {
    format!(
        "The NextToken in your request is not valid. Use the NextToken returned by \
         your previous {operation} request."
    )
}

/// The creation sequence a listing's `NextToken` resumes after — `None` when the
/// request carried no token — or that listing's own refusal for a token it never
/// handed out.
///
/// `scope` is the listing reading the token ([`CursorScope`]) and `minted_through` what
/// the store has actually minted for the family that listing reports
/// ([`ComAmazonawsTranscribeTranscribeState::minted_sequences`]), which together make
/// "a cursor THIS listing handed out" decidable rather than assumed; see
/// [`parse_page_cursor`]. Generic over the error channel because each listing models
/// its own: `refuse` is what lifts the single-homed refusal text
/// ([`invalid_next_token_message`]) into it, so no handler spells that text or the
/// token format itself.
pub fn resume_after_sequence<E>(
    token: Option<&str>,
    scope: CursorScope<'_>,
    minted_through: u64,
    refuse: impl FnOnce(String) -> E,
) -> Result<Option<u64>, E> {
    match token {
        None => Ok(None),
        Some(token) => match parse_page_cursor(token, scope, minted_through) {
            Some(sequence) => Ok(Some(sequence)),
            None => Err(refuse(invalid_next_token_message(scope.listing()))),
        },
    }
}

/// The record NAME a listing's `NextToken` resumes strictly after — `None` when the
/// request carried no token — or that listing's own refusal for a token it never handed
/// out.
///
/// The name-keyed counterpart of [`resume_after_sequence`], for the one listing of
/// this service whose records carry no creation sequence (see [`name_page_cursor`]).
pub fn resume_after_name<'t, E>(
    token: Option<&'t str>,
    scope: CursorScope<'_>,
    refuse: impl FnOnce(String) -> E,
) -> Result<Option<&'t str>, E> {
    match token {
        None => Ok(None),
        Some(token) => match parse_name_page_cursor(token, scope) {
            Some(name) => Ok(Some(name)),
            None => Err(refuse(invalid_next_token_message(scope.listing()))),
        },
    }
}

/// How many records one page of a listing carries: what the request's `MaxResults`
/// asked for, or `default` where it asked for nothing.
///
/// Every listing of this service models `MaxResults` `@range(min: 1, max: 100)`, so
/// the boundary validator has already refused anything outside it; the clamp states
/// that dependency rather than relying on it silently. Single-homed so the nine
/// listings cannot disagree about what a page size is — only about the default their
/// own model documents, which each one passes in.
pub fn page_size(max_results: Option<i32>, default: i32) -> usize {
    usize::try_from(max_results.unwrap_or(default))
        .unwrap_or(MAX_LIST_MAX_RESULTS)
        .clamp(1, MAX_LIST_MAX_RESULTS)
}

/// A stored record one of this service's sequence-paged listings reports.
///
/// The three facts such a listing needs of any family's record, whatever shape it is
/// projected onto: the creation sequence it is ordered and paged by, the name its
/// `*NameContains` filter searches and its cursor carries, and the status its status
/// filter selects on.
pub trait ListedRecord {
    /// Creation order, newest = largest. The cursor a listing of this family pages
    /// with ([`page_cursor`]).
    fn sequence(&self) -> u64;
    /// The name this record is keyed by.
    fn name(&self) -> &str;
    /// The record's own status or state, or `None` for a family the model gives no
    /// such member at all (a vocabulary filter has no `VocabularyState`), whose
    /// listing publishes no status filter either.
    fn status(&self) -> Option<&str> {
        None
    }
}

/// Give each family's record the [`ListedRecord`] view of itself, naming the member
/// that family's status lives on where it has one.
macro_rules! listed_records {
    ($($record:ident $(, status = $status:ident)?;)+) => {
        $(
            impl ListedRecord for $record {
                fn sequence(&self) -> u64 {
                    self.sequence
                }

                fn name(&self) -> &str {
                    &self.name
                }

                $(
                    fn status(&self) -> Option<&str> {
                        Some(&self.$status)
                    }
                )?
            }
        )+
    };
}

listed_records! {
    TranscriptionJobRecord, status = status;
    MedicalTranscriptionJobRecord, status = status;
    MedicalScribeJobRecord, status = status;
    CallAnalyticsJobRecord, status = status;
    LanguageModelRecord, status = status;
    VocabularyRecord, status = state;
    MedicalVocabularyRecord, status = state;
    VocabularyFilterRecord;
}

/// What a listing selects the records of its page on: the two filters every
/// sequence-paged listing of this service publishes, however it spells their members
/// (`Status` / `StateEquals` / `StatusEquals`, and `JobNameContains` / `NameContains`).
///
/// A filter the request left out is `None`, which selects everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct ListFilter<'a> {
    /// Keep only records whose own status or state is exactly this.
    pub status_equals: Option<&'a str>,
    /// Keep only records whose name contains this, case-insensitively.
    pub name_contains: Option<&'a str>,
}

/// One page of a listing: the records that belong on it, already projected onto the
/// shape that listing reports, and the cursor to the rest.
#[derive(Debug, Clone)]
pub struct ListPage<T> {
    /// The page itself, in the order the records were given in.
    pub items: Vec<T>,
    /// The `NextToken` the caller resumes with, or `None` when this page is the last:
    /// "If NextToken is present in your response, it indicates that not all results
    /// are displayed" (the doc of every listing's `NextToken` output member).
    pub next_token: Option<String>,
}

/// One page of `records` — already ordered, newest first — resuming strictly past
/// `resume_below`, narrowed by `filter`, at most `page_size` long, with each kept
/// record projected by `project`.
///
/// THE PAGING RULE, single-homed here beside the token codec so every listing of this
/// service pages the same way: a record at or above the resumed cursor was on an
/// earlier page and is skipped; the filters select what the page may hold; and a
/// `NextToken` is emitted only when a FURTHER MATCHING record was found after the
/// page filled, minted in the SCOPE of the listing handing it out ([`CursorScope`]) and
/// built from the last record placed ON the page rather than from the one that
/// overflowed it — which is what makes the next page resume exactly where this one
/// stopped, and only for this listing.
pub fn list_page<'r, R, T>(
    records: impl IntoIterator<Item = &'r R>,
    scope: CursorScope<'_>,
    resume_below: Option<u64>,
    page_size: usize,
    filter: ListFilter<'_>,
    project: impl Fn(&R) -> T,
) -> ListPage<T>
where
    R: ListedRecord + 'r,
{
    // "The search is not case sensitive" (the doc of every `*NameContains` member of
    // this service's listings), so the needle is folded once here and each candidate
    // name folded as it is tested.
    let needle = filter.name_contains.map(str::to_lowercase);
    let mut items = Vec::new();
    let mut next_token = None;
    let mut last_on_page: Option<(u64, String)> = None;
    for record in records {
        if resume_below.is_some_and(|sequence| record.sequence() >= sequence) {
            continue;
        }
        if filter
            .status_equals
            .is_some_and(|wanted| record.status() != Some(wanted))
        {
            continue;
        }
        if needle
            .as_deref()
            .is_some_and(|needle| !record.name().to_lowercase().contains(needle))
        {
            continue;
        }
        if items.len() == page_size {
            // A further match exists, so the caller gets a cursor to it.
            next_token = last_on_page
                .as_ref()
                .map(|(sequence, name)| page_cursor(scope, *sequence, name));
            break;
        }
        items.push(project(record));
        last_on_page = Some((record.sequence(), record.name().to_string()));
    }
    ListPage { items, next_token }
}

/// What a request's two content members say about where a vocabulary's terms come
/// from: a list in the request, or a file the service reads them from.
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
    /// The terms are in the file at this URI, which the service reads them from
    /// ([`resolve_vocabulary_file`]).
    File(&'a str),
    /// Neither member carries content.
    Neither,
    /// Both members carry content, which the two operations document as an error.
    Both,
}

/// How this service words "your request says nowhere the terms come from", for the
/// operation whose file member is `file_member` ([`VOCABULARY_FILE_MEMBER`] or
/// [`VOCABULARY_FILTER_FILE_MEMBER`]).
///
/// Entirely FIXED text per member pair: no part of either sentence comes from the
/// request, so neither carries the name the caller asked for. The two pairs word the
/// condition differently, so the text is looked up per pair here rather than composed
/// from the member names at each call site — the same reason [`not_found_message`]
/// looks its text up per family. Single-homed because the create and the update of
/// each family publish the SAME pair of alternatives and refuse a request that names
/// neither of them alike.
pub fn missing_content_source_message(file_member: &str) -> &'static str {
    match file_member {
        VOCABULARY_FILTER_FILE_MEMBER => FILTER_WORDS_REQUIRED_MESSAGE,
        _ => VOCABULARY_TERMS_REQUIRED_MESSAGE,
    }
}

/// How this service refuses a `CreateVocabulary`/`UpdateVocabulary` naming neither
/// `Phrases` nor `VocabularyFileUri`. See [`missing_content_source_message`].
pub const VOCABULARY_TERMS_REQUIRED_MESSAGE: &str =
    "Either Phrases or VocabularyFileUri field should be provided.";

/// How this service refuses a `CreateVocabularyFilter`/`UpdateVocabularyFilter` naming
/// neither `Words` nor `VocabularyFilterFileUri`. See
/// [`missing_content_source_message`].
pub const FILTER_WORDS_REQUIRED_MESSAGE: &str = "You need to provide either the Words \
     field or the VocabularyFilterFileUri field. Provide one of the fields and try \
     your request again.";

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
/// S3 location its text file is stored at.
///
/// REPORTED AND SERVED: the file of terms is written to that very bucket and key when
/// a create or an update supplies them ([`store_vocabulary_content`]), so "use this
/// URI to view or download the custom vocabulary filter" holds — a client that fetches
/// the location reads the terms back. The URI is built from the same two pieces the
/// write addresses ([`service_managed_bucket`], [`vocabulary_content_key`]), so the
/// reported location and the stored object cannot drift apart. The bucket is
/// [`service_managed_bucket`], the one [`output_uri`]'s service-managed branch names as
/// well, so both URIs this service reports really do name the same bucket.
pub fn vocabulary_download_uri(resource_type: &str, name: &str, region: &str) -> String {
    format!(
        "https://s3.{region}.amazonaws.com/{bucket}/{key}",
        bucket = service_managed_bucket(region),
        key = vocabulary_content_key(resource_type, name),
    )
}

/// The bucket a Region's service-managed files live in: the built vocabulary and
/// vocabulary-filter files the reported `DownloadUri` names
/// ([`vocabulary_download_uri`]), and the transcript of a job that named no output
/// bucket of its own ([`output_uri`]). ONE home for the name, so the two cannot drift.
pub fn service_managed_bucket(region: &str) -> String {
    format!("aws-transcribe-{region}-prod")
}

/// The key the built file of the resource `name` of `resource_type` is stored under in
/// [`service_managed_bucket`] — the path part of its `DownloadUri`.
pub fn vocabulary_content_key(resource_type: &str, name: &str) -> String {
    format!(
        "{account}/{resource_type}/{name}",
        account = ::engine::emulator_account_id(),
    )
}

/// The text file a custom vocabulary's or vocabulary filter's terms are stored as: one
/// term per line, in the order the request listed them.
///
/// That is the form the two operations document for the file a caller may supply the
/// same terms in — "the Amazon S3 location of the text file that contains your custom
/// vocabulary filter terms", with one entry per line — so the file this service builds
/// and the file a caller would upload are the same kind of text.
pub fn vocabulary_content_file(terms: &[String]) -> String {
    terms.iter().map(|term| format!("{term}\n")).collect()
}

/// The `Content-Type` the built text file is stored with.
const VOCABULARY_CONTENT_TYPE: &str = "text/plain";

/// The HTTP status Amazon S3 answers a `CreateBucket` for a name its one bucket
/// namespace already holds: `BucketAlreadyOwnedByYou` and `BucketAlreadyExists`, the
/// two errors that operation models, are both 409.
const BUCKET_EXISTS_STATUS: u16 = 409;

/// The Region a `CreateBucket` carrying NO location constraint places its bucket in,
/// from `CreateBucketConfiguration$LocationConstraint`: "If you don't specify a Region,
/// the bucket is created in the US East (N. Virginia) Region (us-east-1) by default."
/// It is also why `BucketLocationConstraint` enumerates every other Region code but not
/// this one, so a bucket in this Region is created with no configuration at all.
const UNCONSTRAINED_BUCKET_REGION: &str = "us-east-1";

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The ONE client this service reaches object storage with: a job start resolving the
/// media location it was given, the listings that resolve a custom language model's
/// training and tuning data ([`resolve_training_data`]), the read of the file a custom
/// vocabulary's terms come from ([`resolve_vocabulary_file`]) and the writes that store a
/// custom vocabulary's or filter's built text file ([`store_vocabulary_content`]) all
/// go through it, so none of them can reach the object store in a way the others
/// would not.
///
/// The callback is the framework's documented seam for calling another emulated
/// service: it routes in process, so no network client is configured here and this
/// crate names no other service's crate. The `endpoint_url` below is therefore an
/// ADDRESS the in-process router reads, never a host any resolver looks up — and the
/// one value that reaches its authority is narrowed HERE, in every build: a region
/// outside the Region-identifier grammar ([`is_region_like`]) falls back to
/// [`DEFAULT_REGION`], exactly as a request carrying no credential scope does. Otherwise
/// `region` is the request's own signing region, so a file is written where the caller's
/// own Amazon S3 requests go — which is the Region the reported `DownloadUri` names —
/// and the signing scope and the address always agree.
///
/// The credentials are the emulator's own: they only sign this operation's in-process
/// calls and are never put in a response or a log.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let region = if is_region_like(region) {
        region
    } else {
        DEFAULT_REGION
    };
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a
        // virtual-host bucket name is not a host this emulator resolves, so the
        // request addresses its bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch
        // for a stalled stream, and each of those would need a sleep implementation
        // this client is not given. The same goes for caching the identity: the
        // credentials are handed over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Store `terms` as the text file the `DownloadUri` of the resource `name` of
/// `resource_type` names — or WITHDRAW that file when there are no terms.
///
/// The reported location is the one the content really lives at: a create or an update
/// that supplies the terms writes the file ([`vocabulary_content_file`]) at the bucket
/// and key the URI is built from, over the in-process callback, so the location a
/// reader reports serves this resource's CURRENT terms and a rebuild from other terms
/// replaces them. An EMPTY list of terms is the other direction of the same rule:
/// nothing was built (the file the terms were to be read from was not served, or the
/// resource is being deleted), so nothing is served there and any file an earlier
/// build left is withdrawn.
///
/// Called from the mutating operations only: a read reports the location, it never
/// writes one.
///
/// Object storage answering nothing says nothing about this service's record — the
/// control plane is real whether or not the emulator was assembled with Amazon S3 — so
/// a failed write is logged and the operation still answers; it never turns a
/// successful create or update into a refusal.
pub async fn store_vocabulary_content(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_type: &str,
    name: &str,
    terms: &[String],
) {
    let bucket = service_managed_bucket(region);
    let key = vocabulary_content_key(resource_type, name);
    let client = s3_client(ctx, region);

    if terms.is_empty() {
        // Nothing is built, so nothing is served there. An answer saying the bucket or
        // the object is not there already means exactly that, which is why this is not
        // a warning.
        if let Err(error) = client
            .delete_object()
            .bucket(&bucket)
            .key(&key)
            .send()
            .await
        {
            ::tracing::debug!(
                bucket = %bucket,
                key = %key,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "no file was withdrawn for this resource; its reported location serves nothing"
            );
        }
        return;
    }

    // The file needs a bucket to live in, and this one is the service's own rather
    // than anything a caller created, so the write stands it up first.
    create_service_managed_bucket(&client, &bucket, region).await;

    let file = vocabulary_content_file(terms);
    match client
        .put_object()
        .bucket(&bucket)
        .key(&key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            file.into_bytes(),
        ))
        .content_type(VOCABULARY_CONTENT_TYPE)
        .send()
        .await
    {
        Ok(_) => ::tracing::info!(
            bucket = %bucket,
            key = %key,
            terms = terms.len(),
            "the file this resource's DownloadUri names was stored"
        ),
        Err(error) => ::tracing::warn!(
            bucket = %bucket,
            key = %key,
            detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the file this resource's DownloadUri names was not stored"
        ),
    }
}

/// Stand up the Region's service-managed bucket, so a write into it has a bucket to
/// live in.
///
/// Idempotent in effect: a name the one bucket namespace already holds answers 409
/// ([`BUCKET_EXISTS_STATUS`]), which is this bucket already being there. Every write
/// into that bucket calls it — a custom vocabulary's built text file
/// ([`store_vocabulary_content`]) and the output document of a job that named no bucket
/// of its own ([`store_job_outputs`]) — so neither depends on the other having gone
/// first.
async fn create_service_managed_bucket(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    region: &str,
) {
    let configuration = (region != UNCONSTRAINED_BUCKET_REGION).then(|| {
        ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
            .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(region))
            .build()
    });
    if let Err(error) = client
        .create_bucket()
        .bucket(bucket)
        .set_create_bucket_configuration(configuration)
        .send()
        .await
    {
        use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

        let status = error
            .raw_response()
            .map(|response| response.status().as_u16());
        if status != Some(BUCKET_EXISTS_STATUS) {
            ::tracing::warn!(
                bucket = %bucket,
                code = error.code().unwrap_or_default(),
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the service-managed bucket for this service's own files was not created"
            );
        }
    }
}

/// One stored transcription job: exactly what the request asked for, plus the
/// lifecycle facts the service itself owns (status, timestamps, transcript location).
///
/// This family has job QUEUEING, and the model names the request member that opts into
/// it: "Makes it possible to enable job queuing when your concurrent request limit is
/// exceeded. When AllowDeferredExecution is set to true, transcription job requests are
/// placed in a queue until the number of jobs falls below the concurrent request limit.
/// If AllowDeferredExecution is set to false and the number of transcription job
/// requests exceed the concurrent request limit, you get a LimitExceededException
/// error" (`smithy_query(query="shape", shape="JobExecutionSettings")`). So a start
/// that sets that member while the family's concurrency ceiling is already reached
/// ([`MAX_CONCURRENT_TRANSCRIPTION_JOBS`]) is ACCEPTED in [`STATUS_QUEUED`] with no
/// start time and has two clock-owed transitions, `QUEUED` -> `IN_PROGRESS` ->
/// `COMPLETED`, exactly as a queued Call Analytics job does ([`CallAnalyticsJobRecord`]);
/// a start at that ceiling that deferred nothing is refused, as is one that would make
/// the queue deeper than this emulator keeps it
/// ([`MAX_QUEUED_TRANSCRIPTION_JOBS`]), and every accepted job below the ceiling is
/// processing from the moment it is started ([`admit_start`]).
#[derive(Debug, Clone)]
pub struct TranscriptionJobRecord {
    /// `TranscriptionJobName`, with the Region, this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor `ListTranscriptionJobs`
    /// pages with.
    pub sequence: u64,
    /// `TranscriptionJobStatus`: one of [`STATUS_QUEUED`], [`STATUS_IN_PROGRESS`],
    /// [`STATUS_COMPLETED`], [`STATUS_FAILED`].
    pub status: String,
    pub language_code: Option<String>,
    pub media: Media,
    /// `MediaFormat`, the format of this job's input media. What the start request
    /// declared it to be, if anything, until the job FINISHES, when the service's own
    /// determination fills it in where the request declared nothing
    /// ([`determined_media_format`]).
    pub media_format: Option<String>,
    pub media_sample_rate_hertz: Option<i32>,
    pub output_bucket_name: Option<String>,
    pub output_key: Option<String>,
    /// `OutputEncryptionKMSKeyId`: the key the transcript would be encrypted with.
    /// Recorded as asked for; nothing encrypts anything, because this emulator holds
    /// no key to encrypt the document it writes with.
    pub output_encryption_kms_key_id: Option<String>,
    /// `KMSEncryptionContext`, recorded for the same reason as
    /// [`Self::output_encryption_kms_key_id`].
    pub kms_encryption_context: Option<std::collections::HashMap<String, String>>,
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
    /// `IdentifiedLanguageScore`, the confidence the single language this job
    /// identified was reported with; `None` until a job that asked for
    /// single-language identification completes, and for a job that named its own
    /// language, which the service identified nothing for
    /// ([`Self::materialise_identified_languages`]).
    pub identified_language_score: Option<f32>,
    /// `LanguageCodes`, one entry per language a MULTI-language identification job
    /// identified; `None` until such a job completes, and for every job that did not
    /// ask for it ([`identified_languages`]).
    pub language_codes: Option<Vec<LanguageCodeItem>>,
    pub language_id_settings: Option<std::collections::HashMap<String, LanguageIdSettings>>,
    pub toxicity_detection: Option<Vec<ToxicityDetectionSettings>>,
    /// The subtitle request, echoed back as `SubtitlesOutput`; its
    /// `SubtitleFileUris` are filled in when the job completes.
    pub subtitles: Option<SubtitlesOutput>,
    /// Where the transcript was written; `None` until the job completes.
    pub transcript: Option<Transcript>,
    /// Whether the documents the locations this job reports serve have already been
    /// stored there ([`due_output_objects`]).
    pub outputs_stored: bool,
    pub failure_reason: Option<String>,
    /// The `FailureReason` the clock owes this job INSTEAD of a transcript, established
    /// from the media the start was pointed at ([`media_failure_reason`]); `None` for a
    /// job the service found nothing wrong with. Processing is what fails, so the job is
    /// accepted and reports this only once the clock has finished with it
    /// ([`Self::advance_clock`]).
    pub failure_due: Option<String>,
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
    /// Completing a job records WHERE each output it asked for is written. Storing the
    /// document at those locations is the one step this cannot take — it is a call into
    /// object storage, which only an operation can make — so the operation that finds
    /// the job finished makes it ([`due_output_objects`], [`store_job_outputs`]).
    ///
    /// It also records what the service determined while it processed the media: the
    /// language identification the request asked for
    /// ([`Self::materialise_identified_languages`]) and the format of the input audio
    /// ([`determined_media_format`]). Both are a function of the stored record, so a job
    /// reports the same ones at every later read.
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
            self.completion_time = Some(self.completion_due);
            // A job whose media the service found wrong ends FAILED, reporting why and
            // no transcript: there is no output to name for a job that produced none.
            if let Some(reason) = self.failure_due.clone() {
                self.status = STATUS_FAILED.to_string();
                self.failure_reason = Some(reason);
                return;
            }
            self.status = STATUS_COMPLETED.to_string();
            self.materialise_identified_languages();
            self.media_format = determined_media_format(self.media_format.take(), &self.media);
            let transcript_uri = self.output_uri("json");
            // Which transcripts the request asked the service to produce decides
            // which of the two locations the job reports ([`transcript_outputs`]).
            // `Media$RedactedMediaFileUri` cannot ask this family for a redacted
            // output: it "is only supported for Call Analytics
            // (StartCallAnalyticsJob) transcription requests" (model doc for
            // `Media$RedactedMediaFileUri`), so `ContentRedaction` decides alone.
            let outputs = transcript_outputs(self.content_redaction.as_ref(), false);
            self.transcript = Some(Transcript {
                transcript_file_uri: outputs.unredacted.then(|| transcript_uri.clone()),
                redacted_transcript_file_uri: outputs
                    .redacted
                    .then(|| redacted_transcript_uri(&transcript_uri)),
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

    /// Record what the automatic language identification this job asked for
    /// determined, as part of completing it.
    ///
    /// Identifying the language is part of processing the media, so a job reports the
    /// result only once it has finished, and only when the request asked the service
    /// to identify the language instead of naming one: a job started with an explicit
    /// `LanguageCode` had nothing identified for it and reports neither a score nor a
    /// per-language breakdown.
    ///
    /// WHICH members a finished job carries follows from which of the two
    /// identifications it asked for: `LanguageCode` "is used with single-language
    /// identification. For multi-language identification requests, refer to the plural
    /// version of this parameter, `LanguageCodes`", and `IdentifiedLanguageScore` is
    /// "the confidence score associated with the language identified in your media
    /// file" — one language, so it accompanies the singular member
    /// (`smithy_query(query="shape", shape="TranscriptionJob")`).
    ///
    /// Both determinations are made from the request rather than from audio (see
    /// [`identified_language`] and [`identified_languages`]), so they are a function
    /// of the stored record and every later read of the job reports the same ones.
    fn materialise_identified_languages(&mut self) {
        if self.identify_multiple_languages == Some(true) {
            let codes = identified_languages(self.language_options.as_deref());
            self.language_codes = Some(codes);
        } else if self.identify_language == Some(true) {
            let code = identified_language(self.language_options.as_deref());
            self.language_code = Some(code);
            self.identified_language_score = Some(IDENTIFIED_LANGUAGE_SCORE);
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
            identified_language_score: self.identified_language_score,
            language_codes: self.language_codes.clone(),
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
            identified_language_score: self.identified_language_score,
            language_codes: self.language_codes.clone(),
            toxicity_detection: self.toxicity_detection.clone(),
        }
    }
}

/// One stored MEDICAL transcription job (Amazon Transcribe Medical): exactly what the
/// request asked for, plus the lifecycle facts the service itself owns (status,
/// timestamps and the location its transcript is written to).
///
/// NOTHING IS TRANSCRIBED, exactly as for the non-medical family (see this module's
/// header): the `MedicalTranscript.TranscriptFileUri` a completed job reports names a
/// location holding a transcript with no words in it, and no PHI is identified. The job
/// RECORD is what this emulates.
///
/// `StartMedicalTranscriptionJob` has no deferred-execution member of any kind
/// (`smithy_query(query="operation", operation="StartMedicalTranscriptionJob")` lists
/// no `JobExecutionSettings`), so a job of this family is processing from the moment
/// it is started and has exactly one clock-owed transition, `IN_PROGRESS` ->
/// `COMPLETED`.
#[derive(Debug, Clone)]
pub struct MedicalTranscriptionJobRecord {
    /// `MedicalTranscriptionJobName`, with the Region, this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor a listing of this family
    /// would page with.
    pub sequence: u64,
    /// `TranscriptionJobStatus`: [`STATUS_IN_PROGRESS`] until the clock owes it
    /// [`STATUS_COMPLETED`] — or [`STATUS_FAILED`], for a job the service found
    /// something wrong with in the media it was pointed at ([`Self::failure_due`]).
    /// [`STATUS_QUEUED`] is modelled but unreached: nothing queues a job of this
    /// family.
    pub status: String,
    /// `LanguageCode`, a REQUIRED member the operation narrows to one value: "US
    /// English (en-US) is the only valid value for medical transcription jobs"
    /// ([`MEDICAL_LANGUAGE_CODE`]).
    pub language_code: String,
    pub media: Media,
    /// `MediaFormat`, the format of this job's input media, determined as
    /// [`TranscriptionJobRecord::media_format`] is: the two families' start requests
    /// carry the member alike, so one determination answers for both
    /// ([`determined_media_format`]).
    pub media_format: Option<String>,
    pub media_sample_rate_hertz: Option<i32>,
    /// `OutputBucketName`, a REQUIRED member of the start request: the bucket the
    /// reported transcript URI names, so this family has no service-managed branch as
    /// [`output_uri`] has.
    pub output_bucket_name: String,
    pub output_key: Option<String>,
    /// `OutputEncryptionKMSKeyId`: the key the transcript would be encrypted with.
    /// Recorded as asked for; nothing encrypts anything, because this emulator holds
    /// no key to encrypt the document it writes with.
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
    /// Whether the document the location this job reports serves has already been
    /// stored there ([`due_output_objects`]).
    pub outputs_stored: bool,
    /// `FailureReason`, set only on a [`STATUS_FAILED`] job — the reason the clock
    /// materialised from [`Self::failure_due`].
    pub failure_reason: Option<String>,
    /// The `FailureReason` the clock owes this job INSTEAD of a transcript, established
    /// from the media the start was pointed at ([`media_failure_reason`]); `None` for a
    /// job the service found nothing wrong with. Processing is what fails, so the job is
    /// accepted and reports this only once the clock has finished with it
    /// ([`Self::advance_clock`]).
    pub failure_due: Option<String>,
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
    /// Completing a job records WHERE its transcript is written; the operation that
    /// finds the job finished stores the document there, for the reason
    /// [`TranscriptionJobRecord::advance_clock`] gives. It records the format of the
    /// input audio the service determined too, exactly as that family's does
    /// ([`determined_media_format`]).
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.status != STATUS_IN_PROGRESS || now < self.completion_due {
            return;
        }
        self.completion_time = Some(self.completion_due);
        // A job whose media the service found wrong ends FAILED, reporting why and no
        // transcript: there is no output to name for a job that produced none.
        if let Some(reason) = self.failure_due.clone() {
            self.status = STATUS_FAILED.to_string();
            self.failure_reason = Some(reason);
            return;
        }
        self.status = STATUS_COMPLETED.to_string();
        self.media_format = determined_media_format(self.media_format.take(), &self.media);
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
/// location its output is written to).
///
/// `StartMedicalScribeJob` has no deferred-execution member of any kind
/// (`smithy_query(query="operation", operation="StartMedicalScribeJob")`), so a job
/// of this family is processing from the moment it is started and has exactly one
/// clock-owed transition, `IN_PROGRESS` -> `COMPLETED`.
#[derive(Debug, Clone)]
pub struct MedicalScribeJobRecord {
    /// `MedicalScribeJobName`, with the Region, this record's key in the store.
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
    /// Recorded as asked for; nothing encrypts anything, because this emulator holds
    /// no key to encrypt the document it writes with.
    pub output_encryption_kms_key_id: Option<String>,
    /// `KMSEncryptionContext`, recorded for the same reason as
    /// [`Self::output_encryption_kms_key_id`].
    pub kms_encryption_context: Option<std::collections::HashMap<String, String>>,
    pub data_access_role_arn: String,
    /// `Settings`, as [`recorded_medical_scribe_settings`] records them: what the start
    /// asked for, with the documented clinical-note template default materialised.
    pub settings: MedicalScribeSettings,
    pub channel_definitions: Option<Vec<MedicalScribeChannelDefinition>>,
    /// Whether the start request carried a `MedicalScribeContext`, which is the ONLY
    /// thing the model reports about it (`MedicalScribeJob$MedicalScribeContextProvided`
    /// "Indicates whether the MedicalScribeContext object was provided"). The context
    /// itself is not kept: no operation of this service reports it, and its one member
    /// (`MedicalScribePatientContext$Pronouns`) is modelled `@sensitive`.
    pub context_provided: bool,
    /// Where the transcript and the clinical note were written; `None` until the
    /// job completes. See [`medical_scribe_output_uri`].
    pub output: Option<MedicalScribeOutput>,
    /// Whether the documents the locations this job reports serve have already been
    /// stored there ([`due_output_objects`]).
    pub outputs_stored: bool,
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
    /// Completing a job records WHERE its output is written; the operation that finds
    /// the job finished stores the documents there, for the reason
    /// [`TranscriptionJobRecord::advance_clock`] gives.
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
/// Reported AND served, exactly as the unredacted location is (see this module's
/// header): nothing redacts a transcript nothing produced, so the document stored
/// there is the same contentless transcript — at a location distinct from the
/// unredacted one, as the model's two separate members require.
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

/// The `MediaFormat` a job that has just FINISHED reports for the media it processed:
/// the format the request declared, or — where it declared none — the one the service
/// determined from the media it was given ([`media_format_of`]).
///
/// `MediaFormat` is "The format of the input media file" on every job shape that reports
/// one (`smithy_query(query="doc", shape="TranscriptionJob", member="MediaFormat")`, and
/// the same sentence on `MedicalTranscriptionJob` and `CallAnalyticsJob`), so it is the
/// format of the FILE rather than an echo of what a request guessed: a finished job
/// reports what the service established about its input audio whether or not the request
/// said anything about it. The two can never disagree on a job that FINISHED — a declared
/// format the stored media contradicts ends the job `FAILED` instead
/// ([`media_failure_reason`]) — so a declared format is kept as the start reported it and
/// the determination fills the member in only where the request left it blank. Media that
/// evidences no modelled format leaves it absent rather than guessed, as
/// [`media_format_of`] does.
///
/// One determination for all three families that report the member: the two
/// transcription families, whose start request carries a `MediaFormat` of its own, and
/// the Call Analytics family, whose start carries none at all.
pub fn determined_media_format(declared: Option<String>, media: &Media) -> Option<String> {
    declared.or_else(|| media_format_of(media))
}

/// The Amazon S3 bucket an `s3://` URI addresses, together with everything after it —
/// the key, or the key PREFIX, which is empty for a URI that names the bucket alone.
///
/// The ONE `s3://` parse this service makes: the training and tuning locations of a
/// custom language model ([`read_training_location`]) are prefixes, so they may name a
/// bucket with nothing after it, while a URI naming one OBJECT layers the
/// non-empty-key requirement on top of this ([`s3_object_address`]) — so the two
/// cannot disagree about what a URI addresses.
///
/// `None` for a value that addresses no bucket, including the `http(s)://` form the
/// `Uri` pattern also admits, which addresses a HOST, and this emulator resolves no
/// hosts, so such a URI names nothing it can ask an object store about.
fn s3_bucket_and_path(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.trim().strip_prefix("s3://")?;
    let (bucket, path) = rest.split_once('/').unwrap_or((rest, ""));
    (!bucket.is_empty()).then_some((bucket, path))
}

/// The Amazon S3 object a URI addresses: its bucket and its key.
///
/// Every member of this service that names ONE object spells it the same way —
/// `Media$MediaFileUri` is "The Amazon S3 location of the media file you want to
/// transcribe", spelled `s3://DOC-EXAMPLE-BUCKET/media-files/my-media-file.flac` in the
/// member's own examples, and `CreateMedicalVocabularyRequest$VocabularyFileUri` is "The
/// Amazon S3 location (URI) of the text file that contains your custom medical
/// vocabulary", spelled `s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt` — so the first path
/// segment is the bucket and everything after it is the key, and one parse answers for
/// the media a job is pointed at and the file a vocabulary is built from alike.
/// [`s3_bucket_and_path`] with the requirement those members add: such a URI names an
/// OBJECT, so a URI naming only a bucket addresses none.
fn s3_object_address(uri: &str) -> Option<(&str, &str)> {
    let (bucket, key) = s3_bucket_and_path(uri)?;
    (!key.is_empty()).then_some((bucket, key))
}

/// Whether `uri` is of the Amazon S3 scheme and yet addresses NO object of it
/// ([`s3_object_address`]): `s3://` with no bucket, or a bucket with no key under it.
///
/// Such a URI needs no object store to judge: the media member is "The Amazon S3
/// location of the media file you want to transcribe", and every example of it names a
/// bucket AND a key (`s3://DOC-EXAMPLE-BUCKET/my-media-file.flac`), so an `s3://` URI
/// carrying no key addresses a bucket rather than a media file and names no object the
/// service could ever read — unlike the `http(s)://` form, which addresses a host this
/// emulator cannot resolve and so leaves the media simply unknown.
fn s3_uri_addresses_no_object(uri: &str) -> bool {
    uri.trim().starts_with("s3://") && s3_object_address(uri).is_none()
}

/// What asking this emulator's own Amazon S3 about the media a start request named
/// established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaVerdict {
    /// No object of the object store answers for the media the request named — either
    /// Amazon S3 reports that an object it named is not there, or the URI is of the S3
    /// scheme and addresses no object of it at all
    /// ([`s3_uri_addresses_no_object`]): the start is refused and nothing is stored.
    NoSuchObject,
    /// Every object the request addressed resolved, so the service has seen the stored
    /// media it is to process.
    Resolved,
    /// Nothing is known about the media: there was no object store to ask, the URI
    /// addresses no object this emulator can resolve ([`s3_object_address`]), or only
    /// SOME of the objects the request addressed resolved. Nothing is refused and no job
    /// is failed on an assumption.
    Unknown,
}

/// The refusal a request that names an Amazon S3 object the store holds none of gets:
/// "To make a StartMedicalTranscriptionJob request, you must first upload your media
/// file into an Amazon S3 bucket; you can then specify the Amazon S3 location of the
/// file using the Media parameter" — the same precondition every job family's start
/// documents, and the same one a custom vocabulary's table file carries ("you must
/// first upload a text file that contains your vocabulary table into an Amazon S3
/// bucket"), so one wording answers for every member of this service that names an
/// object: the file a request points at is not there.
pub const URI_NAMES_NO_OBJECT: &str =
    "The URI that you provided doesn't point to an S3 object. Make sure that the object \
     exists and try your request again.";

/// The Amazon S3 error codes in which S3 reports that an object a request names is NOT
/// THERE: a bucket it does not hold (`NoSuchBucket`), a key that bucket does not hold
/// (`NoSuchKey`), a version it holds none of (`NoSuchVersion`), and the code `HeadObject`
/// models for either (`NotFound`, which a HEAD carries no body to spell out).
///
/// The code is required TOGETHER WITH the 404 status ([`object_is_absent`]), and never
/// the status alone, because a status is not evidence that S3 answered at all: a request
/// that reached no S3 is a 404 too, and that is this emulator failing to ask rather than
/// a verdict about the object. The list is deliberately CLOSED, and holds only the codes
/// that say the object is absent — the refusal this wording states
/// ([`URI_NAMES_NO_OBJECT`]) is about an object that is not there, so a code
/// reporting something else about it (a read that is not permitted, a refusal S3 adds
/// later) answers [`MediaVerdict::Unknown`] and refuses nothing.
const ABSENT_OBJECT_CODES: &[&str] = &["NoSuchBucket", "NoSuchKey", "NoSuchVersion", "NotFound"];

/// Whether S3's answer to a read is S3 ITSELF saying it holds nothing at that
/// address: HTTP 404 AND one of the codes it sends for a missing one
/// ([`ABSENT_OBJECT_CODES`]).
///
/// One judgement for every read this service makes of a caller-named object, whichever
/// operation of the object store asked: the `HeadObject` a job's media is resolved with
/// ([`resolve_media_objects`]) and the `GetObject` a vocabulary's file is read with
/// ([`resolve_vocabulary_file`]) answer an absent object the same way, so what "the
/// object is not there" means cannot differ between them.
fn object_is_absent<E>(
    error: &::aws_sdk_s3::error::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool
where
    E: ::aws_sdk_s3::error::ProvideErrorMetadata,
{
    use ::aws_sdk_s3::error::ProvideErrorMetadata;

    error
        .raw_response()
        .is_some_and(|response| response.status().as_u16() == 404)
        && error
            .code()
            .is_some_and(|code| ABSENT_OBJECT_CODES.contains(&code))
}

/// Resolve every media object `uris` names against this emulator's own Amazon S3 and
/// report what that established.
///
/// A job's media is resolved when the job is STARTED: "you must first upload your media
/// file into an Amazon S3 bucket; you can then specify the Amazon S3 location of the
/// file using the Media parameter", so a start naming an object the store does not hold
/// is refused with the operation's modelled `BadRequestException`
/// ([`URI_NAMES_NO_OBJECT`]) and nothing is persisted. S3 is reached the way any
/// emulated service reaches another: through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`], see [`s3_client`] — no
/// socket, no outbound request, no host resolution).
///
/// The verdict is read from S3's own absent-object answer and never from an HTTP status
/// alone ([`object_is_absent`]): with no S3 to ask (a router with only this service
/// mounted, say) nothing is known about the media, so the start is accepted — a refusal
/// has to be grounded in what the object store answered, never assumed. The one refusal
/// that needs no answer is a URI of the S3 scheme that addresses no object of it
/// ([`s3_uri_addresses_no_object`]): it names no media file for any store to hold.
///
/// The read is a `HeadObject`, "the same options as a GET operation on an object"
/// without its body: what a start has to establish is that the media it was pointed at
/// IS there, and nothing in this service transcribes audio, so no media file's bytes are
/// ever fetched or held.
pub async fn resolve_media_objects(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uris: &[&str],
) -> MediaVerdict {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    // One client for the whole set: a family that names its input audio in two members
    // resolves both through it.
    let client = s3_client(ctx, region);
    let mut asked = 0usize;
    let mut resolved = 0usize;
    for uri in uris {
        let Some((bucket, key)) = s3_object_address(uri) else {
            // A URI of the S3 scheme that addresses no object of it names no media file
            // at all ([`s3_uri_addresses_no_object`]), so the start is refused without
            // asking the store: there is no object for it to answer about.
            if s3_uri_addresses_no_object(uri) {
                return MediaVerdict::NoSuchObject;
            }
            continue;
        };
        asked += 1;
        match client.head_object().bucket(bucket).key(key).send().await {
            // The object is there. Only its METADATA was asked for, never its bytes:
            // nothing in this service transcribes audio, so what the start needs to
            // know is that the media it was pointed at exists.
            Ok(_) => resolved += 1,
            Err(error) => {
                if object_is_absent(&error) {
                    // S3 says the object is not there: the refusal is grounded.
                    return MediaVerdict::NoSuchObject;
                }
                // Any other answer is not S3 reporting an absent object, so it refuses
                // nothing — and it is worth a warning, because a deployment with no S3
                // mounted to ask is one where this check does not run. The caller's own
                // values are rendered with their escaped `Debug` form, so a key carrying
                // a newline cannot forge a log line.
                ::tracing::warn!(
                    bucket = ?bucket,
                    key = ?key,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    code = ?error.code(),
                    "the media object could not be resolved against this emulator's Amazon S3"
                );
            }
        }
    }
    // `Resolved` says the service has seen EVERY object the request addressed, which is
    // what judging the media it was given rests on: one object that resolved beside one
    // the store could not be asked about leaves the set unknown, whatever order they
    // were asked in.
    if asked > 0 && resolved == asked {
        MediaVerdict::Resolved
    } else {
        MediaVerdict::Unknown
    }
}

/// The `FailureReason` of a job whose declared `MediaFormat` is not the format of the
/// media the service found at the location the request named: "The media format
/// specified in `MediaFormat` doesn't match the format of the input file" — one of the
/// values the model enumerates for that member (`smithy_query(query="doc",
/// shape="MedicalTranscriptionJob", member="FailureReason")`), reported verbatim as the
/// doc spells it.
pub const MEDIA_FORMAT_MISMATCH_FAILURE_REASON: &str =
    "The media format provided does not match the detected media format";

/// The `FailureReason` the clock owes a job started with `declared` as its
/// `MediaFormat` over the media `media` names, or `None` for a job the service found
/// nothing wrong with.
///
/// A format MISMATCH is the one per-media failure this emulator can establish: the
/// format of the stored media is what its name evidences ([`media_format_of`]), and the
/// request states what the caller believes it to be, so the two can disagree. Judged
/// only for a job whose media actually RESOLVED ([`MediaVerdict::Resolved`]) — the
/// service has to have seen the file to have detected its format — and never for the
/// members only the audio itself could answer (the sample rate, the file size), which
/// stay unjudged rather than guessed.
pub fn media_failure_reason(
    verdict: MediaVerdict,
    declared: Option<&str>,
    media: &Media,
) -> Option<String> {
    if verdict != MediaVerdict::Resolved {
        return None;
    }
    let declared = declared?;
    let detected = media_format_of(media)?;
    (detected != declared).then(|| MEDIA_FORMAT_MISMATCH_FAILURE_REASON.to_string())
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
/// [`identified_language`] and [`identified_languages`] have no option list to read.
pub const DEFAULT_IDENTIFIED_LANGUAGE: &str = "en-US";

/// The `IdentifiedLanguageScore` that accompanies an identified language:
/// "Confidence scores are values between 0 and 1; a larger value indicates a higher
/// probability" — the same sentence on both shapes that report one
/// (`smithy_query(query="doc", shape="CallAnalyticsJob", member="IdentifiedLanguageScore")`,
/// `smithy_query(query="doc", shape="TranscriptionJob", member="IdentifiedLanguageScore")`).
///
/// This emulator's identification is not probabilistic — it reads the language off the
/// request rather than weighing audio (see [`identified_language`]) — so what it
/// reports is the top of that documented range, the certainty of a determination that
/// had nothing to weigh, rather than an invented probability.
pub const IDENTIFIED_LANGUAGE_SCORE: f32 = 1.0;

/// The single language a job whose language the service IDENTIFIED was transcribed
/// in, narrowed to the options the request allowed.
///
/// Two families ask the service to identify one language: a Call Analytics job, which
/// has no language member to name one with, and a transcription job started with
/// `IdentifyLanguage` (`IdentifyMultipleLanguages` asks for the plural determination
/// [`identified_languages`] makes instead). Both narrow the choice with a
/// `LanguageOptions` list and both report the result as the finished job's
/// `LanguageCode`, so one determination answers for both rather than each family
/// making its own.
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

/// Every language a MULTI-language identification job identified, narrowed to the
/// options the request allowed.
///
/// `IdentifyMultipleLanguages` "enables automatic multi-language identification in
/// your transcription job request ... you can optionally include a list of language
/// codes, using LanguageOptions ... Including LanguageOptions restricts
/// [identification] to only the language options that you specify"
/// (`smithy_query(query="operation", operation="StartTranscriptionJob")`), and the
/// finished job reports one `LanguageCodeItem` per language identified in the media
/// (`smithy_query(query="shape", shape="LanguageCodeItem")`).
///
/// This emulator weighs no audio, so the determination is made from the request, as
/// the single-language one [`identified_language`] makes is: every code the options
/// allowed, in the order the request listed them, since identification may answer
/// with no other and the options are the caller's own statement of the "language
/// codes that represent the languages you think may be present in your media" —
/// reporting a SUBSET of them, which is what the real service's answer usually is,
/// would take the audio that says which of them is actually spoken. Where the request
/// narrowed the choice to nothing, the one [`DEFAULT_IDENTIFIED_LANGUAGE`] is the
/// whole answer. It is a function of the request alone, so every read of a job reports
/// the same languages in the same order.
///
/// `DurationInSeconds` is "the total time, in seconds, each identified language is
/// spoken in your media" — a measurement of the media itself, which nothing here
/// reads (see the module header: nothing is transcribed) — so each entry leaves that
/// optional member ABSENT rather than reporting a duration nothing timed.
pub fn identified_languages(language_options: Option<&[String]>) -> Vec<LanguageCodeItem> {
    let default = [DEFAULT_IDENTIFIED_LANGUAGE.to_string()];
    let codes = match language_options {
        Some(options) if !options.is_empty() => options,
        _ => default.as_slice(),
    };
    codes
        .iter()
        .map(|code| LanguageCodeItem {
            language_code: Some(code.clone()),
            duration_in_seconds: None,
        })
        .collect()
}

/// One stored Call Analytics job: exactly what the request asked for, plus the
/// lifecycle facts the service itself owns (status, timestamps and the location its
/// transcript is written to).
///
/// This family has job QUEUEING, and the operation names the request member that opts
/// into it: "Job queuing is available for Call Analytics jobs. If you pass a
/// DataAccessRoleArn in your request and you exceed your Concurrent Job Limit, your job
/// will automatically be added to a queue to be processed once your concurrent job
/// count is below the limit"
/// (`smithy_query(query="doc", shape="StartCallAnalyticsJob")`). So a start that passes
/// that member while the family's concurrency ceiling is already reached
/// ([`MAX_CONCURRENT_CALL_ANALYTICS_JOBS`]) is ACCEPTED in [`STATUS_QUEUED`] with no
/// start time and has two clock-owed transitions, `QUEUED` -> `IN_PROGRESS` ->
/// `COMPLETED`, exactly as a deferred batch transcription job does
/// ([`TranscriptionJobRecord`]); a start at that ceiling that passes no role has opted
/// into nothing and is refused, as is one that would make the queue deeper than this
/// emulator keeps it ([`MAX_QUEUED_CALL_ANALYTICS_JOBS`]), and every accepted job below
/// the ceiling is processing from the moment it is started.
///
/// WHAT IT EMULATES is the job RECORD (see the module header): no call is
/// transcribed, no sentiment or call characteristic is derived and no category is
/// matched, so the `Transcript.TranscriptFileUri` a completed job reports names a
/// location holding a transcript with none of that in it.
#[derive(Debug, Clone)]
pub struct CallAnalyticsJobRecord {
    /// `CallAnalyticsJobName`, with the Region, this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor `ListCallAnalyticsJobs`
    /// pages with.
    pub sequence: u64,
    /// `CallAnalyticsJobStatus`: [`STATUS_QUEUED`] while the job waits for a
    /// concurrency slot, then [`STATUS_IN_PROGRESS`] until the clock owes it
    /// [`STATUS_COMPLETED`]. [`STATUS_FAILED`] is modelled but unreached — nothing
    /// this emulator does can fail a job it accepted.
    pub status: String,
    pub media: Media,
    /// `OutputLocation`, the one output member this operation takes; `None` when the
    /// request named none, in which case the transcript is reported in a
    /// service-managed location (see [`call_analytics_output_uri`]).
    pub output_location: Option<String>,
    /// `OutputEncryptionKMSKeyId`: the key the output would be encrypted with.
    /// Recorded as asked for; nothing encrypts anything, because this emulator holds
    /// no key to encrypt the document it writes with.
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
    /// ([`determined_media_format`], which this family reaches with nothing declared:
    /// its start request carries no `MediaFormat` member at all).
    pub media_format: Option<String>,
    /// `MediaSampleRateHertz`, the rate the service determined the input audio is
    /// sampled at; `None` until the job completes
    /// ([`CALL_AUDIO_SAMPLE_RATE_HERTZ`]).
    pub media_sample_rate_hertz: Option<i32>,
    /// Where the transcript was written; `None` until the job completes.
    pub transcript: Option<Transcript>,
    /// Whether the documents the locations this job reports serve have already been
    /// stored there ([`due_output_objects`]).
    pub outputs_stored: bool,
    /// `FailureReason`, set only on a `FAILED` job — which none of this emulator's
    /// are: nothing it does can fail a job it accepted.
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

impl CallAnalyticsJobRecord {
    /// Materialise every status transition the CLOCK already owes this job — leaving
    /// the queue it was accepted into, and finishing — each of which was due at its
    /// stored instant whether or not anybody called. A reporting operation (`Get`,
    /// `List`) runs this before projecting the record, and that is the only write it
    /// performs.
    ///
    /// Completing a job records what the service determined while it processed the
    /// call — the language it identified, the format and sample rate of the audio it
    /// was given, and WHERE each transcript it produced is written (the operation that
    /// finds the job finished stores the document there, for the reason
    /// [`TranscriptionJobRecord::advance_clock`] gives). Every one of those
    /// values is a function of the stored record, so a job reports the same ones at
    /// every later read.
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
        self.media_format = determined_media_format(self.media_format.take(), &self.media);
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
/// Reported AND served: the document a finished job owes this location is stored at it
/// ([`store_job_outputs`]).
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
///
/// The service-managed branch names [`service_managed_bucket`], the one bucket this
/// service's own files live in — the same one a custom vocabulary's `DownloadUri`
/// names ([`vocabulary_download_uri`]).
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
            "https://s3.{region}.amazonaws.com/{bucket}/{account}/{key}",
            bucket = service_managed_bucket(region),
            account = ::engine::emulator_account_id(),
        ),
    }
}

/// One object a finished job's reported output location names: the address it is
/// served from and the document stored there.
///
/// The location a reader REPORTS and the object a client FETCHES are built from one
/// value — the URI the record projects ([`output_object_address`]) — so the two cannot
/// drift apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputObject {
    /// The bucket the reported URI addresses.
    pub bucket: String,
    /// The key it addresses under that bucket.
    pub key: String,
    /// The document stored there.
    pub body: String,
    /// The `Content-Type` it is stored with.
    pub content_type: String,
}

/// The bucket and key a reported output URI addresses, in either of the two forms this
/// service reports one in: the `s3://BUCKET/KEY` of an output location the request
/// named ([`output_uri`], [`call_analytics_output_uri`],
/// [`medical_scribe_output_uri`]) and the path-style
/// `https://s3.REGION.amazonaws.com/BUCKET/KEY` of a service-managed one — both of
/// which the modelled `Uri` pattern `^(s3://|http(s*)://).+$` admits.
///
/// `None` for a value that addresses no object of this emulator's store, which there is
/// nothing to write at: one naming a bucket alone, and one of the `http(s)` form whose
/// authority is not the path-style Amazon S3 endpoint ([`is_path_style_s3_host`]). That
/// second case is why the two forms are not parsed alike: an output location is
/// caller-supplied, and a URI of that form naming any other authority addresses a HOST,
/// which this emulator resolves none of — reading its first path segment as a bucket
/// would write an object into a bucket the caller never named and this service never
/// reports.
fn output_object_address(uri: &str) -> Option<(&str, &str)> {
    let uri = uri.trim();
    if let Some(address) = s3_object_address(uri) {
        return Some(address);
    }
    let (host, path) = uri.split_once("://")?.1.split_once('/')?;
    if !is_path_style_s3_host(host) {
        return None;
    }
    let (bucket, key) = path.split_once('/')?;
    (!bucket.is_empty() && !key.is_empty()).then_some((bucket, key))
}

/// Whether `host` is the path-style Amazon S3 endpoint this service reports a
/// service-managed location at: `s3.REGION.amazonaws.com` for a Region-shaped REGION
/// ([`is_region_like`]), which is the authority [`output_uri`] and
/// [`vocabulary_download_uri`] build such a URI with.
fn is_path_style_s3_host(host: &str) -> bool {
    host.strip_prefix("s3.")
        .and_then(|rest| rest.strip_suffix(".amazonaws.com"))
        .is_some_and(is_region_like)
}

/// The `Content-Type` a JSON output document is stored with.
const OUTPUT_JSON_CONTENT_TYPE: &str = "application/json";

/// `SubtitleFormat` values, from the model's enum
/// (`smithy_query(query="shape", shape="SubtitleFormat")`).
pub const SUBTITLE_FORMAT_SUBRIP: &str = SUBTITLE_FORMAT_VALUES[0];
/// See [`SUBTITLE_FORMAT_SUBRIP`].
pub const SUBTITLE_FORMAT_WEBVTT: &str = SUBTITLE_FORMAT_VALUES[1];

/// The first line of every WebVTT file, which is what identifies the format.
const WEBVTT_SIGNATURE: &str = "WEBVTT";

/// The `Content-Type` a WebVTT subtitle file is stored with: the media type registered
/// for that format.
const SUBTITLE_CONTENT_TYPE_WEBVTT: &str = "text/vtt";

/// The `Content-Type` a SubRip subtitle file is stored with. CHOSEN: SubRip has no
/// registered media type of its own and the file is plain text.
const SUBTITLE_CONTENT_TYPE_SUBRIP: &str = "text/plain";

/// The transcript document a finished job serves at the location it reports.
///
/// There IS a document to fetch there: `Transcript$TranscriptFileUri` is "The Amazon S3
/// location of your transcript. You can use this URI to access or download your
/// transcript" (`smithy_query(query="doc", shape="Transcript",
/// member="TranscriptFileUri")`), and `MedicalTranscript$TranscriptFileUri` says the
/// same of its own. It is JSON, as every output key this service names ends in `.json`.
///
/// Its members are the facts the service itself owns: the job this is the output of,
/// the account that ran it and the status it finished in. The transcribed text is EMPTY
/// and the item list holds nothing, because no engine read the audio (see the module
/// header) — the document is the true envelope of a transcription that produced no
/// words, not invented speech.
pub fn transcript_document(job_name: &str, status: &str) -> String {
    ::serde_json::json!({
        "jobName": job_name,
        "accountId": ::engine::emulator_account_id(),
        "status": status,
        "results": {"transcripts": [{"transcript": ""}], "items": []},
    })
    .to_string()
}

/// The clinical note a finished Medical Scribe job serves at the location it reports
/// ([`MEDICAL_SCRIBE_CLINICAL_DOCUMENT_FILE`]).
///
/// `MedicalScribeOutput$ClinicalDocumentUri` "Holds the Amazon S3 URI for the Clinical
/// Document" (`smithy_query(query="shape", shape="MedicalScribeOutput")`), so a
/// document is fetchable there. It carries NO section: a clinical note is written from
/// the conversation, and nothing here transcribes one, so what a client reads is a note
/// with no content rather than an invented summary of a patient.
pub fn clinical_document() -> String {
    ::serde_json::json!({"ClinicalDocumentation": {"Sections": []}}).to_string()
}

/// The subtitle file a finished job serves for one requested format, with the
/// `Content-Type` it is stored with.
///
/// `SubtitleFormat` is `vtt | srt` ([`SUBTITLE_FORMAT_VALUES`]). A WebVTT file is
/// identified by its own first line, so the one served carries that line and no cue; a
/// SubRip file is a sequence of cues and nothing else, so one with no cue is empty.
/// Either way it holds no caption, for the reason [`transcript_document`] gives.
pub fn subtitle_document(format: &str) -> (String, &'static str) {
    if format.eq_ignore_ascii_case(SUBTITLE_FORMAT_SUBRIP) {
        return (String::new(), SUBTITLE_CONTENT_TYPE_SUBRIP);
    }
    // The other of the two modelled formats, [`SUBTITLE_FORMAT_WEBVTT`].
    (
        format!("{WEBVTT_SIGNATURE}\n\n"),
        SUBTITLE_CONTENT_TYPE_WEBVTT,
    )
}

/// One output object at the address `uri` names, or `None` when that URI addresses no
/// object an object store could hold ([`output_object_address`]).
fn output_object(uri: &str, body: String, content_type: &str) -> Option<OutputObject> {
    let (bucket, key) = output_object_address(uri)?;
    Some(OutputObject {
        bucket: bucket.to_string(),
        key: key.to_string(),
        body,
        content_type: content_type.to_string(),
    })
}

/// The object a reported transcript location serves: the transcript document of the job
/// named `job_name` at the address `uri` names.
fn transcript_object(uri: &str, job_name: &str, status: &str) -> Option<OutputObject> {
    output_object(
        uri,
        transcript_document(job_name, status),
        OUTPUT_JSON_CONTENT_TYPE,
    )
}

/// What a job family's record owes the output locations it REPORTS: the documents a
/// client that follows one of those URIs fetches.
///
/// One trait rather than one spelling per family, so "a finished job's reported
/// location serves the document it names" is written once over any family's record
/// ([`due_output_objects`], [`store_job_outputs`]): the job families differ in WHICH
/// locations a finished record reports, not in how the documents reach the object
/// store. A family added later reaches those helpers only by implementing this.
pub trait JobOutputs {
    /// The objects this record's reported output locations serve — empty for a job the
    /// clock has not finished yet, and for one that finished with no output to report
    /// (a `FAILED` job produced none).
    fn output_objects(&self) -> Vec<OutputObject>;

    /// Whether those objects have already been stored.
    fn outputs_stored_mut(&mut self) -> &mut bool;
}

/// The objects `record` still owes the locations it reports, taken ONCE.
///
/// The write a finished job owes is materialised exactly like the status transition
/// that finished it: it fell due when the job completed, whether or not anybody called,
/// so the first operation that finds the job finished takes the documents and stores
/// them ([`store_job_outputs`]) — and no later call takes them again, so a reader
/// reports the same locations without touching what is stored there and what a client
/// fetched once stays what it fetched.
pub fn due_output_objects<R: JobOutputs>(record: &mut R) -> Vec<OutputObject> {
    if *record.outputs_stored_mut() {
        return Vec::new();
    }
    let mut objects = record.output_objects();
    // ONE write per location, however many times the record reports it. A request's
    // subtitle formats are a list the model gives no length (`SubtitleFormats`), so a
    // request may name one format over and over — and each naming is the same file:
    // "if you select both WebVTT (vtt) and SubRip (srt) formats, two output files are
    // generated" (model doc for `Subtitles$Formats`), so there are as many files as
    // there are distinct formats. Deduplicating here is what keeps the work one read
    // does bounded by how many distinct locations a job HAS rather than by how long a
    // list the request sent.
    let mut seen = ::std::collections::BTreeSet::new();
    objects.retain(|object| seen.insert((object.bucket.clone(), object.key.clone())));
    if !objects.is_empty() {
        *record.outputs_stored_mut() = true;
    }
    objects
}

/// Store the documents a finished job's reported output locations serve.
///
/// The addresses come from the record's own projection, so a client that follows the
/// URI a reader reported fetches the document stored here — which is what lets the
/// canonical workflow (start a job, poll it until it is finished, then fetch its
/// output) complete. The object store is this emulator's own Amazon S3, reached the way
/// any emulated service reaches another: over the in-process callback ([`s3_client`] —
/// no socket, no outbound request, no host resolution).
///
/// The service-managed bucket is this service's own, so a write into it stands it up
/// first ([`create_service_managed_bucket`]); a bucket the REQUEST named belongs to the
/// caller, who created it before starting the job, so nothing is created there.
///
/// Nothing is ENCRYPTED: `OutputEncryptionKMSKeyId` is recorded as asked for and the
/// document is stored as written, because this emulator holds no key to encrypt it
/// with.
///
/// Object storage answering nothing says nothing about this service's record — the
/// control plane is real whether or not the emulator was assembled with Amazon S3 — so
/// a failed write is logged and the operation still answers; it never turns a
/// successful read into a refusal. It is REPORTED, though: the answer is whether every
/// document was stored, and a caller that hears otherwise gives the documents back to
/// the record ([`return_due_output_objects`]), because a write the object store did not
/// take is one the job still owes.
pub async fn store_job_outputs(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    objects: &[OutputObject],
) -> bool {
    if objects.is_empty() {
        return true;
    }
    let client = s3_client(ctx, region);
    let service_managed = service_managed_bucket(region);
    let mut stood_up = false;
    let mut stored_every_one = true;
    for object in objects {
        if object.bucket == service_managed && !stood_up {
            create_service_managed_bucket(&client, &service_managed, region).await;
            stood_up = true;
        }
        match client
            .put_object()
            .bucket(&object.bucket)
            .key(&object.key)
            .body(::aws_sdk_s3::primitives::ByteStream::from(
                object.body.clone().into_bytes(),
            ))
            .content_type(object.content_type.as_str())
            .send()
            .await
        {
            Ok(_) => ::tracing::info!(
                bucket = ?object.bucket,
                key = ?object.key,
                "the document this job's reported output location names was stored"
            ),
            Err(error) => {
                stored_every_one = false;
                ::tracing::warn!(
                    bucket = ?object.bucket,
                    key = ?object.key,
                    detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the document this job's reported output location names was not stored"
                );
            }
        }
    }
    stored_every_one
}

/// The objects a job's reported output locations name in THIS SERVICE'S OWN bucket
/// ([`service_managed_bucket`]), out of every location `record` reports
/// ([`JobOutputs::output_objects`]).
///
/// The service-managed branch of a reported location is the only one this service owns:
/// a bucket the REQUEST named belongs to the caller, who created it before starting the
/// job ([`store_job_outputs`]), so nothing of the caller's is ever named here. The
/// addresses are the record's own projection — the very one the write addresses — so a
/// caller can never be told about one location and have another touched.
///
/// Taken while the record is still held, since it is the record that knows where its
/// files are; what is DONE with them is awaited outside the store lock
/// ([`withdraw_job_outputs`]).
pub fn service_managed_outputs<R: JobOutputs>(record: &R, region: &str) -> Vec<OutputObject> {
    let service_managed = service_managed_bucket(region);
    let mut objects = record.output_objects();
    objects.retain(|object| object.bucket == service_managed);
    // ONE withdrawal per address, however many times the record reports it, for the
    // reason [`due_output_objects`] deduplicates the writes.
    let mut seen = ::std::collections::BTreeSet::new();
    objects.retain(|object| seen.insert(object.key.clone()));
    objects
}

/// Withdraw the documents this service's own bucket holds for a job whose record has
/// just been removed.
///
/// The other direction of [`store_job_outputs`], and the same rule the vocabulary
/// family's delete closes with ([`store_vocabulary_content`] called with no terms): the
/// record is gone, so this service reports those locations to nobody, and the files it
/// wrote into its OWN bucket for that record are withdrawn with it rather than left
/// behind for a job nothing holds. Only the service-managed objects are passed in
/// ([`service_managed_outputs`]) — an object in a bucket the request named is the
/// caller's, who created that bucket and keeps whatever is in it.
///
/// Object storage answering nothing says nothing about this service's record — the
/// control plane is real whether or not the emulator was assembled with Amazon S3 — so a
/// withdrawal that is not taken is logged and the delete still answers. An answer saying
/// the bucket or the object is not there already means nothing is served there, which is
/// why that is not a warning.
pub async fn withdraw_job_outputs(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    objects: &[OutputObject],
) {
    if objects.is_empty() {
        return;
    }
    let client = s3_client(ctx, region);
    for object in objects {
        match client
            .delete_object()
            .bucket(&object.bucket)
            .key(&object.key)
            .send()
            .await
        {
            Ok(_) => ::tracing::info!(
                bucket = ?object.bucket,
                key = ?object.key,
                "the document this job's reported output location named was withdrawn"
            ),
            Err(error) => ::tracing::debug!(
                bucket = ?object.bucket,
                key = ?object.key,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "no document was withdrawn for this job; nothing is served there"
            ),
        }
    }
}

/// Give the documents a write did NOT store back to `record`, so the next operation
/// that reads the job takes them again ([`due_output_objects`]).
///
/// What the job owes its reported locations does not stop being owed because one
/// attempt to store it failed — the bucket the request named may not have existed yet,
/// or the emulator may have been assembled with no object store at that moment — so the
/// write stays due rather than being consumed by the attempt. `None` is the job having
/// been deleted while the write was in flight, which owes nothing.
pub fn return_due_output_objects<R: JobOutputs>(record: Option<&mut R>) {
    if let Some(record) = record {
        *record.outputs_stored_mut() = false;
    }
}

// The four job families' output sets, together so what each one owes its reported
// locations can be read beside the others. Each is a projection of the SAME members the
// family's reader reports, so no location is written that a client was not told about,
// and none is reported that holds nothing.

impl JobOutputs for TranscriptionJobRecord {
    /// The transcript this job produced, at each location it reports one at
    /// ([`transcript_outputs`]), plus one subtitle file per requested format.
    fn output_objects(&self) -> Vec<OutputObject> {
        let Some(transcript) = self.transcript.as_ref() else {
            return Vec::new();
        };
        let mut objects: Vec<OutputObject> = [
            transcript.transcript_file_uri.as_deref(),
            transcript.redacted_transcript_file_uri.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|uri| transcript_object(uri, &self.name, &self.status))
        .collect();
        // "Your subtitle file is stored in the same location as your transcript. If you
        // specified both WebVTT and SubRip subtitle formats, two URIs are provided"
        // (model doc for `SubtitlesOutput$SubtitleFileUris`): one file per requested
        // format, each at the location reported for that format — the two lists are
        // built in one order ([`Self::advance_clock`]), so they pair up.
        if let Some(subtitles) = self.subtitles.as_ref() {
            let formats = subtitles.formats.iter().flatten();
            let uris = subtitles.subtitle_file_uris.iter().flatten();
            objects.extend(formats.zip(uris).filter_map(|(format, uri)| {
                let (body, content_type) = subtitle_document(format);
                output_object(uri, body, content_type)
            }));
        }
        objects
    }

    fn outputs_stored_mut(&mut self) -> &mut bool {
        &mut self.outputs_stored
    }
}

impl JobOutputs for MedicalTranscriptionJobRecord {
    /// The one transcript this family reports, in the bucket the start named.
    fn output_objects(&self) -> Vec<OutputObject> {
        self.transcript
            .as_ref()
            .and_then(|transcript| transcript.transcript_file_uri.as_deref())
            .and_then(|uri| transcript_object(uri, &self.name, &self.status))
            .into_iter()
            .collect()
    }

    fn outputs_stored_mut(&mut self) -> &mut bool {
        &mut self.outputs_stored
    }
}

impl JobOutputs for CallAnalyticsJobRecord {
    /// The transcript of the call, at each location this job reports one at
    /// ([`transcript_outputs`]). No analysis of the call is in it, for the reason
    /// [`transcript_document`] gives: nothing listened to it.
    fn output_objects(&self) -> Vec<OutputObject> {
        let Some(transcript) = self.transcript.as_ref() else {
            return Vec::new();
        };
        [
            transcript.transcript_file_uri.as_deref(),
            transcript.redacted_transcript_file_uri.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|uri| transcript_object(uri, &self.name, &self.status))
        .collect()
    }

    fn outputs_stored_mut(&mut self) -> &mut bool {
        &mut self.outputs_stored
    }
}

impl JobOutputs for MedicalScribeJobRecord {
    /// Both files this family reports: the transcript and the clinical note.
    fn output_objects(&self) -> Vec<OutputObject> {
        let Some(output) = self.output.as_ref() else {
            return Vec::new();
        };
        [
            transcript_object(&output.transcript_file_uri, &self.name, &self.status),
            output_object(
                &output.clinical_document_uri,
                clinical_document(),
                OUTPUT_JSON_CONTENT_TYPE,
            ),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn outputs_stored_mut(&mut self) -> &mut bool {
        &mut self.outputs_stored
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

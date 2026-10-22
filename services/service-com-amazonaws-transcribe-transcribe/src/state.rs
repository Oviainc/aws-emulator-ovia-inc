// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-transcribe-transcribe service.
//!
//! Two resource families live here: the transcription-job lifecycle — the stored
//! record, the clock-owed status transitions, and the projections of a record onto
//! the modelled `TranscriptionJob` / `TranscriptionJobSummary` shapes — and the
//! custom-vocabulary family (vocabularies and vocabulary filters), with its own
//! records, its `PENDING` -> `READY` transition and its projections onto
//! `VocabularyInfo` / `VocabularyFilterInfo`. Several operations read each record, so
//! the projections are single-homed here rather than copied into each handler; the
//! three tagging operations reach every family's tag set through one lookup
//! ([`ComAmazonawsTranscribeTranscribeState::tags_mut`]).
//!
//! WHAT THIS EMULATES, AND WHAT IT DOES NOT: the job RECORD — its name, its
//! settings, its status, its timestamps and the location its output would be
//! written to. No audio is transcribed and no object is ever written at the
//! reported `Transcript.TranscriptFileUri` / `SubtitlesOutput.SubtitleFileUris`,
//! so a completed job means "the control plane finished with this record", never
//! "a transcript is fetchable from that URI". Nothing invents transcript content,
//! a confidence score (`IdentifiedLanguageScore`) or an identified-language
//! breakdown (`LanguageCodes`): those stay absent because no engine produced them.
//! A custom vocabulary is the same: its terms are held in this state, where a
//! `StartTranscriptionJob` that names the vocabulary would read them from, while the
//! `DownloadUri` it reports is a location nothing serves (see
//! [`vocabulary_download_uri`]) and no operation of this service reports the terms
//! themselves — none of their output shapes has a member for them. A vocabulary whose
//! content would have to be READ from a file URI is refused at the create rather than
//! recorded: this emulator reads no object store and makes no outbound request, so it
//! would have nothing to build the vocabulary from.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ContentRedaction, EncryptionConfiguration, JobExecutionSettings, LanguageIdSettings, Media,
    ModelSettings, Settings, SubtitlesOutput, Tag, ToxicityDetectionSettings, Transcript,
    TranscriptionJob, TranscriptionJobSummary, VocabularyFilterInfo, VocabularyInfo,
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
pub fn is_terminal_job_status(status: &str) -> bool {
    status == STATUS_COMPLETED || status == STATUS_FAILED
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

/// The number of transcription jobs `ListTranscriptionJobs` returns when the
/// request names no `MaxResults`: "If you do not specify a value, a default of 5 is
/// used" (model doc for `ListTranscriptionJobsRequest$MaxResults`).
pub const DEFAULT_LIST_MAX_RESULTS: i32 = 5;

/// The largest page `ListTranscriptionJobs` serves: its `MaxResults` is modelled
/// `@range(min: 1, max: 100)`, so the boundary validator refuses more than this
/// before a handler runs.
pub const MAX_LIST_MAX_RESULTS: usize = 100;

/// The most transcription jobs this emulator holds for an account.
///
/// Amazon Transcribe publishes a default quota of 100 batch transcription jobs per
/// account and answers `LimitExceededException` ("You've either sent too many
/// requests or your input file is too long") when a request would exceed a quota.
/// Applying that published figure to the stored jobs keeps the store bounded: a
/// caller that never deletes cannot grow it without end, and
/// `DeleteTranscriptionJob` is what makes room again.
pub const MAX_TRANSCRIPTION_JOBS: usize = 100;

/// `VocabularyState` values, from the model's enum
/// (`smithy_query(query="shape", shape="VocabularyState")`).
pub const VOCABULARY_STATE_PENDING: &str = "PENDING";
/// See [`VOCABULARY_STATE_PENDING`].
pub const VOCABULARY_STATE_READY: &str = "READY";
/// See [`VOCABULARY_STATE_PENDING`]. No vocabulary this emulator records reaches it:
/// a create whose content it cannot build is refused with the operation's modelled
/// `BadRequestException` instead of being recorded as a failed vocabulary.
pub const VOCABULARY_STATE_FAILED: &str = "FAILED";

/// How long after the call that starts it a custom vocabulary's
/// `PENDING` -> `READY` transition is owed.
///
/// A chosen span, not a documented one: Amazon Transcribe gives no processing-time
/// guarantee for building a custom vocabulary. It is deliberately non-zero, so a
/// caller that reads the vocabulary straight back still observes `PENDING` (and the
/// state errors that go with it), and short enough that a poll-until-ready script
/// terminates quickly. The due instant is stored on the record; nothing sleeps for it.
pub const VOCABULARY_TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The most custom vocabularies this emulator holds for an account.
///
/// Amazon Transcribe publishes a default quota of 100 custom vocabularies per account
/// per Region, and the same figure for custom vocabulary filters. Applying the
/// published figure keeps the store bounded: a caller that never deletes cannot grow
/// it without end, and `DeleteVocabulary` / `DeleteVocabularyFilter` is what makes
/// room again.
pub const MAX_VOCABULARIES: usize = 100;

/// The number of custom vocabularies `ListVocabularies` / `ListVocabularyFilters`
/// return when the request names no `MaxResults`: "If you do not specify a value, a
/// default of 5 is used" (model doc for `ListVocabulariesRequest$MaxResults` and
/// `ListVocabularyFiltersRequest$MaxResults`).
pub const DEFAULT_VOCABULARY_LIST_MAX_RESULTS: i32 = 5;

/// The ARN resource type of a transcription job, as the `TranscribeArn` pattern's
/// `<type>/<name>` resource segment spells it
/// (`^arn:aws(-[^:]+)?:transcribe:[a-zA-Z0-9-]*:[0-9]{12}:[a-zA-Z-]*/[0-9a-zA-Z._-]+$`),
/// and as the Amazon Transcribe "Tagging resources" guide lists it.
pub const RESOURCE_TYPE_TRANSCRIPTION_JOB: &str = "transcription-job";
/// The ARN resource type of a custom vocabulary. See
/// [`RESOURCE_TYPE_TRANSCRIPTION_JOB`].
pub const RESOURCE_TYPE_VOCABULARY: &str = "vocabulary";
/// The ARN resource type of a custom vocabulary filter. See
/// [`RESOURCE_TYPE_TRANSCRIPTION_JOB`].
pub const RESOURCE_TYPE_VOCABULARY_FILTER: &str = "vocabulary-filter";

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
pub const KIND_VOCABULARY_FILTER: &str = "vocabulary filter";
/// See [`KIND_JOB`].
pub const KIND_RESOURCE: &str = "resource";

/// The resource kind the ARN resource type `resource_type` denotes — the word a
/// refusal about that resource names it by.
///
/// The three mapped types are the ones [`ComAmazonawsTranscribeTranscribeState::tags`]
/// and [`ComAmazonawsTranscribeTranscribeState::tags_mut`] can find a record for, so
/// the two agree about which ARN names which family.
pub fn resource_kind(resource_type: &str) -> &'static str {
    match resource_type {
        RESOURCE_TYPE_TRANSCRIPTION_JOB => KIND_JOB,
        RESOURCE_TYPE_VOCABULARY => KIND_VOCABULARY,
        RESOURCE_TYPE_VOCABULARY_FILTER => KIND_VOCABULARY_FILTER,
        _ => KIND_RESOURCE,
    }
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
    /// Every custom vocabulary this account holds, keyed by its name (which
    /// `CreateVocabulary` requires to be unique within an account).
    pub vocabularies: BTreeMap<String, VocabularyRecord>,
    /// Every custom vocabulary filter this account holds, keyed by its name (which
    /// `CreateVocabularyFilter` requires to be unique within an account).
    pub vocabulary_filters: BTreeMap<String, VocabularyFilterRecord>,
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

    /// The stored custom vocabulary filters, newest first. A filter has no
    /// `VocabularyState` in the model, so none of them has a clock-owed transition.
    pub fn vocabulary_filters_newest_first(&self) -> Vec<&VocabularyFilterRecord> {
        let mut records: Vec<&VocabularyFilterRecord> =
            self.vocabulary_filters.values().collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.sequence));
        records
    }

    /// The tag set of the resource `resource` names, or `None` when this account holds
    /// no such resource.
    ///
    /// The read-only half of [`Self::tags_mut`], so a reporting operation
    /// (`ListTagsForResource`) takes no mutable borrow for a read. The two must agree
    /// about which ARN names which stored record, so both match on the same resource
    /// types.
    pub fn tags(&self, resource: &ResourceRef) -> Option<&Vec<Tag>> {
        match resource.resource_type.as_str() {
            RESOURCE_TYPE_TRANSCRIPTION_JOB => self
                .transcription_jobs
                .get(&resource.name)
                .map(|record| &record.tags),
            RESOURCE_TYPE_VOCABULARY => self
                .vocabularies
                .get(&resource.name)
                .map(|record| &record.tags),
            RESOURCE_TYPE_VOCABULARY_FILTER => self
                .vocabulary_filters
                .get(&resource.name)
                .map(|record| &record.tags),
            _ => None,
        }
    }

    /// The tag set of the resource `resource` names, for reading and writing, or
    /// `None` when this account holds no such resource.
    ///
    /// One lookup for the two tagging operations that WRITE, so `TagResource` and
    /// `UntagResource` cannot disagree about which ARN names which stored record. A
    /// resource type this emulator records nothing for (a language model, a Call
    /// Analytics category) answers `None`, which the caller renders as the modelled
    /// not-found error — never as an empty tag set.
    pub fn tags_mut(&mut self, resource: &ResourceRef) -> Option<&mut Vec<Tag>> {
        match resource.resource_type.as_str() {
            RESOURCE_TYPE_TRANSCRIPTION_JOB => self
                .transcription_jobs
                .get_mut(&resource.name)
                .map(|record| &mut record.tags),
            RESOURCE_TYPE_VOCABULARY => self
                .vocabularies
                .get_mut(&resource.name)
                .map(|record| &mut record.tags),
            RESOURCE_TYPE_VOCABULARY_FILTER => self
                .vocabulary_filters
                .get_mut(&resource.name)
                .map(|record| &mut record.tags),
            _ => None,
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

/// One stored custom vocabulary: the content the request asked for, plus the
/// lifecycle facts the service itself owns (its processing state and when it was
/// last modified).
#[derive(Debug, Clone)]
pub struct VocabularyRecord {
    /// `VocabularyName`, also this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor `ListVocabularies` pages
    /// with.
    pub sequence: u64,
    /// `LanguageCode`: the one language this vocabulary's terms are in.
    pub language_code: String,
    /// The terms this vocabulary holds, as the request listed them.
    pub phrases: Vec<String>,
    /// `VocabularyState`: [`VOCABULARY_STATE_PENDING`], [`VOCABULARY_STATE_READY`]
    /// or [`VOCABULARY_STATE_FAILED`].
    pub state: String,
    /// `FailureReason`, set only on a `FAILED` vocabulary.
    pub failure_reason: Option<String>,
    /// When `PENDING` -> `READY` falls due; `None` once it has been materialised.
    pub ready_due: Option<SystemTime>,
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
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.state != VOCABULARY_STATE_PENDING {
            return;
        }
        if let Some(due) = self.ready_due.filter(|due| now >= *due) {
            self.state = VOCABULARY_STATE_READY.to_string();
            // The resource changed when the build finished, so the modification
            // instant is that of the transition, not of the request that started it.
            self.last_modified_time = due;
            self.ready_due = None;
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

    /// Where this vocabulary's own file is reported to live. See
    /// [`vocabulary_download_uri`].
    pub fn download_uri(&self) -> String {
        vocabulary_download_uri(RESOURCE_TYPE_VOCABULARY, &self.name, &self.region)
    }
}

/// One stored custom vocabulary filter: the words it holds and when it was last
/// modified.
///
/// A filter has no `VocabularyState` anywhere in the model — neither
/// `CreateVocabularyFilter`'s output nor `VocabularyFilterInfo` carries one
/// (`smithy_query(query="operation", operation="CreateVocabularyFilter")`) — so it
/// is usable from the moment it is created and has no clock-owed transition.
#[derive(Debug, Clone)]
pub struct VocabularyFilterRecord {
    /// `VocabularyFilterName`, also this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor `ListVocabularyFilters`
    /// pages with.
    pub sequence: u64,
    pub language_code: String,
    /// The words this filter masks, deletes or flags, as the request listed them.
    pub words: Vec<String>,
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

    /// Where this filter's own file is reported to live. See
    /// [`vocabulary_download_uri`].
    pub fn download_uri(&self) -> String {
        vocabulary_download_uri(RESOURCE_TYPE_VOCABULARY_FILTER, &self.name, &self.region)
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

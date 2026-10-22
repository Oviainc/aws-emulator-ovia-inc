// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-transcribe-transcribe service.
//!
//! The transcription-job lifecycle lives here: the stored record, the clock-owed
//! status transitions, and the projections of a record onto the modelled
//! `TranscriptionJob` / `TranscriptionJobSummary` shapes. Three operations
//! (`StartTranscriptionJob`, `GetTranscriptionJob`, `ListTranscriptionJobs`) read
//! the same record, so the projections are single-homed here rather than copied
//! into each handler.
//!
//! WHAT THIS EMULATES, AND WHAT IT DOES NOT: the job RECORD — its name, its
//! settings, its status, its timestamps and the location its output would be
//! written to. No audio is transcribed and no object is ever written at the
//! reported `Transcript.TranscriptFileUri` / `SubtitlesOutput.SubtitleFileUris`,
//! so a completed job means "the control plane finished with this record", never
//! "a transcript is fetchable from that URI". Nothing invents transcript content,
//! a confidence score (`IdentifiedLanguageScore`) or an identified-language
//! breakdown (`LanguageCodes`): those stay absent because no engine produced them.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ContentRedaction, JobExecutionSettings, LanguageIdSettings, Media, ModelSettings, Settings,
    SubtitlesOutput, Tag, ToxicityDetectionSettings, Transcript, TranscriptionJob,
    TranscriptionJobSummary,
};

/// `TranscriptionJobStatus` values, from the model's enum
/// (`smithy_query(query="shape", shape="TranscriptionJobStatus")`).
pub const STATUS_QUEUED: &str = "QUEUED";
/// See [`STATUS_QUEUED`].
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`STATUS_QUEUED`].
pub const STATUS_COMPLETED: &str = "COMPLETED";

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

/// Per-service state for com-amazonaws-transcribe-transcribe. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTranscribeTranscribeState {
    /// Every transcription job this account has started, keyed by its name (which
    /// `StartTranscriptionJob` requires to be unique).
    pub transcription_jobs: BTreeMap<String, TranscriptionJobRecord>,
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

// SPDX-License-Identifier: CC0-1.0

//! The asynchronous batch translation jobs this service runs: the record
//! `StartTextTranslationJob` persists, the status the clock owes that record,
//! and the documents the job reads out of — and writes back into — the
//! emulator's own object storage.
//!
//! A batch job is a control-plane resource with a REAL data plane here: the
//! job reads every document under its `InputDataConfig` location out of Amazon
//! S3 in process (the framework's callback transport, the same one a client's
//! own S3 request takes), translates each one with the transform the
//! synchronous translate operations share, and writes each translation under
//! its `OutputDataConfig` location. So everything the job reports — the
//! document counts, the message, the end time — is what it really did, and a
//! job whose input or output location cannot be used ends in the modelled
//! `FAILED` status instead of reporting a success nothing backs.
//!
//! The record lives in [`super::ComAmazonawsTranslateAwsshinefrontendservice20170701State::jobs`],
//! so `StartTextTranslationJob`, `DescribeTextTranslationJob` and
//! `StopTextTranslationJob` report one stored job by one rule.

use std::time::{Duration, Instant, SystemTime};

use ::sha2::{Digest, Sha256};

use super::{
    counted, language_for_detector_code, resolved_pairs, substitute, timestamp,
    translation_marker, Terminology, AUTO_LANGUAGE_CODE, MAX_DOCUMENT_BYTES,
};
use crate::ops::types::{
    InputDataConfig, JobDetails, OutputDataConfig, TextTranslationJobProperties,
    TranslationSettings,
};

/// `JobStatus`'s seven values (`smithy_query shape=JobStatus`).
pub const STATUS_SUBMITTED: &str = "SUBMITTED";
/// Amazon Translate is processing the job (same shape).
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// The job was successfully completed and the output is available (same shape).
pub const STATUS_COMPLETED: &str = "COMPLETED";
/// The job was completed with errors (same shape).
pub const STATUS_COMPLETED_WITH_ERROR: &str = "COMPLETED_WITH_ERROR";
/// The job did not complete (same shape).
pub const STATUS_FAILED: &str = "FAILED";
/// The user who started the job has requested that it be stopped (same shape).
pub const STATUS_STOP_REQUESTED: &str = "STOP_REQUESTED";
/// The job has been stopped (same shape).
pub const STATUS_STOPPED: &str = "STOPPED";

/// How long one phase of a submitted job's lifecycle takes before the next one
/// is due: `SUBMITTED` -> `IN_PROGRESS` -> its terminal status, and
/// `STOP_REQUESTED` -> `STOPPED`.
///
/// The span is this emulator's own — a real batch job takes minutes — and it is
/// never zero, so a caller that polls straight after submitting still observes
/// the transient statuses the model documents. Nothing sleeps for it: the
/// instant each phase falls due is stored on the record and every read derives
/// the status from the clock.
pub const JOB_PHASE: Duration = Duration::from_secs(3);

/// How many target languages one job may name: "Enter up to 10 language codes"
/// (`smithy_query shape=StartTextTranslationJobRequest
/// member=TargetLanguageCodes`).
pub const MAX_TARGET_LANGUAGES: usize = 10;

/// How many parallel data resources one job applies: "This parameter accepts
/// only one parallel data resource" (`smithy_query
/// shape=StartTextTranslationJobRequest member=ParallelDataNames`).
pub const MAX_PARALLEL_DATA_PER_JOB: usize = 1;

/// The content type whose documents this emulator's batch data plane reads:
/// "text/plain: The input data consists of one or more unformatted text files.
/// Amazon Translate translates every character in this type of input"
/// (`smithy_query shape=InputDataConfig member=ContentType`).
pub const CONTENT_TYPE_PLAIN: &str = "text/plain";

/// The most input documents one batch job reads.
///
/// The input location is caller-sized, and both the reading and the writing
/// grow with it, so the work one submission can ask for is bounded before any
/// of it is done. The bound is this emulator's own; a job pointed at more than
/// this is not silently truncated — it ends `FAILED` naming the bound.
pub const MAX_JOB_DOCUMENTS: usize = 50;

/// The most bytes one input document carries, which is the documented size of
/// one document Amazon Translate translates ([`MAX_DOCUMENT_BYTES`], 100 KB).
pub const MAX_JOB_DOCUMENT_BYTES: usize = MAX_DOCUMENT_BYTES;

/// The most bytes one batch job reads across every document (see
/// [`MAX_JOB_DOCUMENTS`]).
pub const MAX_JOB_TOTAL_BYTES: usize = 8 * 1024 * 1024;

/// How long a translated document this plane writes may grow, in characters.
///
/// A translation EMITS the stored terminology's text rather than copying the
/// document's, so without a ceiling a document at the size limit could make the
/// written answer arbitrarily larger than the input that asked for it. The
/// ceiling is this emulator's own, at the same multiple of the document limit
/// the synchronous document translation holds itself to.
const MAX_TRANSLATED_DOCUMENT_CHARS: usize = 10 * MAX_DOCUMENT_BYTES;

/// The most bytes one batch job WRITES, across every document and every target
/// language.
///
/// What the job reads is bounded by [`MAX_JOB_TOTAL_BYTES`], but what it writes
/// is the PRODUCT of the documents, the target languages and what a
/// translation emits, and bounding each factor alone leaves that product free:
/// a submission at the input ceiling could otherwise make this plane write
/// orders of magnitude more than it read. The budget is spent across the whole
/// job, and a job that would pass it stops there and ends `FAILED` naming the
/// bound rather than writing past it. The bound is this emulator's own.
pub const MAX_JOB_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// How long one batch job's data plane may run.
///
/// The ceilings above bound the work in documents and bytes; this bounds it in
/// TIME, whatever those cost on the host, so one submission can never occupy
/// the process indefinitely. It is checked between documents and between
/// writes, each of which is awaited, so the check is always reached. A job that
/// runs out of time ends `FAILED` naming the bound. The bound is this
/// emulator's own.
pub const MAX_JOB_RUN_TIME: Duration = Duration::from_secs(10);

/// How many batch translation jobs this store holds.
///
/// The store only ever grows — a job record is never deleted — so without this
/// bound a caller looping on submission grows it until the host runs out of
/// memory. The bound is this emulator's own.
pub const MAX_STORED_JOBS: usize = 100;

/// The URI scheme both of a job's data locations name: `S3Uri`'s modelled
/// pattern is `^s3://...`, and that is the storage this emulator serves.
const S3_SCHEME: &str = "s3://";

/// How many characters a job id carries: `JobId` is `length 1..32`
/// (`smithy_query shape=JobId`).
const JOB_ID_CHARS: usize = 32;

/// The outcome a job's processing reached, which is the terminal status the
/// clock reports once the job's last phase falls due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    /// Every input document was translated into every applicable target
    /// language.
    Completed,
    /// The job ran, and at least one document could not be translated.
    CompletedWithError,
    /// The job could not be carried out at all.
    Failed,
}

impl JobOutcome {
    /// The `JobStatus` this outcome is reported as.
    pub fn status(self) -> &'static str {
        match self {
            Self::Completed => STATUS_COMPLETED,
            Self::CompletedWithError => STATUS_COMPLETED_WITH_ERROR,
            Self::Failed => STATUS_FAILED,
        }
    }
}

/// One stored batch translation job.
#[derive(Debug, Clone)]
pub struct TranslationJob {
    pub job_id: String,
    pub job_name: Option<String>,
    /// The request's `ClientToken`, which identifies the submission: a repeated
    /// submission of one token is the same job rather than a second one.
    pub client_token: String,
    pub source_language_code: String,
    pub target_language_codes: Vec<String>,
    pub terminology_names: Vec<String>,
    pub parallel_data_names: Vec<String>,
    pub input_data_config: InputDataConfig,
    pub output_data_config: OutputDataConfig,
    pub data_access_role_arn: String,
    pub settings: Option<TranslationSettings>,
    pub submitted_at: SystemTime,
    /// When `IN_PROGRESS` falls due (see [`JOB_PHASE`]).
    pub in_progress_at: SystemTime,
    /// When the job's terminal status falls due.
    pub finished_at: SystemTime,
    pub outcome: JobOutcome,
    /// "An explanation of any errors that may have occurred during the
    /// translation job" (`smithy_query shape=TextTranslationJobProperties
    /// member=Message`), so it is filled only for a job that had something to
    /// report.
    pub message: Option<String>,
    pub input_documents: usize,
    pub translated_documents: usize,
    pub documents_with_errors: usize,
    /// When a caller asked for the job to be stopped, which is what makes
    /// `STOPPED` — rather than the job's own outcome — its terminal status.
    ///
    /// A stop marks the job; it does not unwrite what the job already wrote.
    /// This plane carries the documents out where the submission is taken, so
    /// by the time any caller can name the job its translations are already in
    /// the output location — and the counts beside this field go on reporting
    /// exactly those, because they are what is really there. What a stop
    /// changes is the job's status and when it ended.
    pub stop_requested_at: Option<SystemTime>,
}

impl TranslationJob {
    /// The job's status at `now`, derived from the instants the submission
    /// stored: a read never has to write one on.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if let Some(requested) = self.stop_requested_at {
            return if now >= requested + JOB_PHASE {
                STATUS_STOPPED
            } else {
                STATUS_STOP_REQUESTED
            };
        }
        if now < self.in_progress_at {
            STATUS_SUBMITTED
        } else if now < self.finished_at {
            STATUS_IN_PROGRESS
        } else {
            self.outcome.status()
        }
    }

    /// Whether the job has reached a status it never leaves, which is what a
    /// stop request is judged against.
    pub fn is_terminal(&self, now: SystemTime) -> bool {
        matches!(
            self.status(now),
            STATUS_COMPLETED | STATUS_COMPLETED_WITH_ERROR | STATUS_FAILED | STATUS_STOPPED
        )
    }

    /// "The time at which the translation job ended" — reported only once it
    /// has (`smithy_query shape=TextTranslationJobProperties member=EndTime`).
    pub fn end_time(&self, now: SystemTime) -> Option<SystemTime> {
        if let Some(requested) = self.stop_requested_at {
            let stopped = requested + JOB_PHASE;
            return (now >= stopped).then_some(stopped);
        }
        (now >= self.finished_at).then_some(self.finished_at)
    }

    /// The stored job as the `TextTranslationJobProperties` every operation
    /// that reports one reports it by.
    pub fn properties(&self, now: SystemTime) -> TextTranslationJobProperties {
        TextTranslationJobProperties {
            job_id: Some(self.job_id.clone()),
            job_name: self.job_name.clone(),
            job_status: Some(self.status(now).to_string()),
            job_details: Some(JobDetails {
                translated_documents_count: Some(counted(self.translated_documents)),
                documents_with_errors_count: Some(counted(self.documents_with_errors)),
                input_documents_count: Some(counted(self.input_documents)),
            }),
            source_language_code: Some(self.source_language_code.clone()),
            target_language_codes: Some(self.target_language_codes.clone()),
            // Reported only for a job that named one: the member lists "the
            // names of the terminologies applied to a translation job".
            terminology_names: (!self.terminology_names.is_empty())
                .then(|| self.terminology_names.clone()),
            parallel_data_names: (!self.parallel_data_names.is_empty())
                .then(|| self.parallel_data_names.clone()),
            message: self.message.clone(),
            submitted_time: Some(timestamp(self.submitted_at)),
            end_time: self.end_time(now).map(timestamp),
            input_data_config: Some(self.input_data_config.clone()),
            output_data_config: Some(self.output_data_config.clone()),
            data_access_role_arn: Some(self.data_access_role_arn.clone()),
            settings: self.settings.clone(),
        }
    }
}

/// The id a submission's job is identified by: 32 hex characters, which is
/// what `JobId`'s length admits.
///
/// Derived from the submission's `ClientToken` and the Region it was signed
/// for, which is what makes the id IDEMPOTENT — "a unique identifier for the
/// request" names one job however many times it is retried — and what keeps two
/// different submissions from colliding on one id.
pub fn job_id(region: &str, client_token: &str) -> String {
    ::hex::encode(Sha256::digest(format!("{region}:{client_token}").as_bytes()))
        .chars()
        .take(JOB_ID_CHARS)
        .collect()
}

/// An `s3://bucket/key` location split into its parts.
#[derive(Debug, Clone)]
pub struct S3Location {
    pub bucket: String,
    /// The key prefix the location names, empty for the bucket's root.
    pub prefix: String,
}

impl S3Location {
    /// The location's prefix as a FOLDER prefix: a location that names one
    /// ends at a delimiter, so the keys under it are the ones that start with
    /// it.
    pub fn folder(&self) -> String {
        if self.prefix.is_empty() || self.prefix.ends_with('/') {
            self.prefix.clone()
        } else {
            format!("{}/", self.prefix)
        }
    }
}

/// Split an `S3Uri` into bucket and key prefix; `None` for a value that names
/// no bucket.
pub fn parse_s3_uri(uri: &str) -> Option<S3Location> {
    let rest = uri.strip_prefix(S3_SCHEME)?;
    let (bucket, prefix) = match rest.split_once('/') {
        Some((bucket, prefix)) => (bucket, prefix),
        None => (rest, ""),
    };
    (!bucket.is_empty()).then(|| S3Location {
        bucket: bucket.to_string(),
        prefix: prefix.to_string(),
    })
}

/// Everything one job's data plane needs, settled from the submission before
/// any document is read.
pub struct BatchPlan {
    pub job_id: String,
    pub input: S3Location,
    pub output: S3Location,
    /// The submission's `SourceLanguageCode`, which may be
    /// [`AUTO_LANGUAGE_CODE`] — then each document's source language is
    /// determined from the document itself.
    pub source_language_code: String,
    pub target_language_codes: Vec<String>,
    /// The terminology records the submission named, copied out of the store so
    /// the documents are translated with no lock held.
    pub terminologies: Vec<Terminology>,
    pub settings: Option<TranslationSettings>,
}

/// What a job's data plane did: the documents it read, how many of them it
/// translated, how many it could not, and what to report about the ones it
/// could not.
#[derive(Debug, Clone, Default)]
pub struct BatchOutcome {
    pub input_documents: usize,
    pub translated_documents: usize,
    pub documents_with_errors: usize,
    pub message: Option<String>,
}

/// One input document: the key it was read from, and its text — `None` for an
/// object whose bytes are not the unformatted text its content type declares.
struct Document {
    key: String,
    text: Option<String>,
}

/// The source language one document is translated FROM: the submission's own
/// code, or — for a submission that asked for `auto` — the language the
/// document's own text is in.
///
/// The detection is the `whatlang` crate's, the same statistical identifier the
/// synchronous translate operations use, so the code is computed from the
/// document rather than echoed from the request.
fn source_language_of(document: &str, declared: &str) -> Result<String, String> {
    if declared != AUTO_LANGUAGE_CODE {
        return Ok(declared.to_string());
    }
    let detected = ::whatlang::detect_lang(document)
        .ok_or_else(|| "its source language could not be determined".to_string())?;
    language_for_detector_code(detected.code())
        .map(ToString::to_string)
        .ok_or_else(|| {
            "it is in a language Amazon Translate does not translate from".to_string()
        })
}

/// Translate one document's text into `target`, with the terms of the named
/// terminologies that apply to the pair.
///
/// The transform is the crate's shared one, so a document translated in batch
/// and the same text translated synchronously are translated by one rule.
fn translate(
    text: &str,
    source: &str,
    target: &str,
    plan: &BatchPlan,
) -> Result<String, String> {
    let marker = translation_marker(source, target, plan.settings.as_ref());
    let mut budget = MAX_TRANSLATED_DOCUMENT_CHARS.saturating_sub(marker.chars().count());
    let mut translated = text.to_string();
    for record in &plan.terminologies {
        let pairs = resolved_pairs(record, source, target);
        let (substituted, _, overflowed) = substitute(&translated, &pairs, budget);
        if overflowed {
            return Err(format!(
                "its translation into {target} is larger than the \
                 {MAX_TRANSLATED_DOCUMENT_CHARS} characters this emulator writes for one \
                 document"
            ));
        }
        budget = budget.saturating_sub(substituted.chars().count());
        translated = substituted;
    }
    Ok(format!("{marker}{translated}"))
}

/// The key one translated document is written under.
///
/// Amazon Translate writes a job's output into a folder of the output location
/// named for the account and the job, with each file named for the target
/// language it was translated into:
/// `<output>/<account>-TranslateText-<job id>/<target>.<input file>`.
fn output_key(plan: &BatchPlan, target: &str, document_key: &str) -> String {
    let relative = document_key
        .strip_prefix(&plan.input.folder())
        .unwrap_or(document_key);
    format!(
        "{}{}-TranslateText-{}/{target}.{relative}",
        plan.output.folder(),
        ::engine::emulator_account_id(),
        plan.job_id
    )
}

/// Every document under the job's input location, as text.
///
/// `Err` is the reason the job could not be carried out — the input location
/// could not be listed or read, or it holds more than this plane reads — which
/// is what the job's `FAILED` status reports.
async fn read_documents(
    client: &::aws_sdk_s3::Client,
    plan: &BatchPlan,
) -> Result<Vec<Document>, String> {
    let folder = plan.input.folder();
    let mut keys: Vec<(String, usize)> = Vec::new();
    let mut continuation: Option<String> = None;
    loop {
        let page = client
            .list_objects_v2()
            .bucket(&plan.input.bucket)
            .prefix(&folder)
            .set_continuation_token(continuation.clone())
            .send()
            .await
            .map_err(|error| {
                format!(
                    "The input location s3://{}/{folder} could not be read: {}",
                    plan.input.bucket,
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error),
                )
            })?;
        for object in page.contents() {
            let Some(key) = object.key() else { continue };
            // A key that ends at the delimiter is a folder marker, not a
            // document.
            if key.ends_with('/') {
                continue;
            }
            // Checked BEFORE the entry is buffered, so not even one document
            // past the bound is held.
            if keys.len() >= MAX_JOB_DOCUMENTS {
                return Err(format!(
                    "The input location holds more than the {MAX_JOB_DOCUMENTS} documents this \
                     emulator translates in one job."
                ));
            }
            keys.push((
                key.to_string(),
                usize::try_from(object.size().unwrap_or_default().max(0)).unwrap_or(usize::MAX),
            ));
        }
        match page.next_continuation_token() {
            Some(token) => continuation = Some(token.to_string()),
            None => break,
        }
    }
    if keys.is_empty() {
        return Err(format!(
            "No documents were found at the input location s3://{}/{folder}.",
            plan.input.bucket
        ));
    }
    // The bounds are decided from the LISTED sizes, before a single document is
    // fetched, so a job past them costs no buffering at all.
    if let Some((key, size)) = keys
        .iter()
        .find(|(_, size)| *size > MAX_JOB_DOCUMENT_BYTES)
    {
        return Err(format!(
            "The document s3://{}/{key} is {size} bytes, more than the \
             {MAX_JOB_DOCUMENT_BYTES} bytes this emulator translates in one document.",
            plan.input.bucket
        ));
    }
    let listed: usize = keys.iter().map(|(_, size)| *size).sum();
    if listed > MAX_JOB_TOTAL_BYTES {
        return Err(format!(
            "The input location holds {listed} bytes, more than the {MAX_JOB_TOTAL_BYTES} bytes \
             this emulator translates in one job."
        ));
    }

    let mut documents = Vec::with_capacity(keys.len());
    for (key, _) in keys {
        let object = client
            .get_object()
            .bucket(&plan.input.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|error| {
                format!(
                    "The document s3://{}/{key} could not be read: {}",
                    plan.input.bucket,
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error),
                )
            })?;
        let body = object
            .body
            .collect()
            .await
            .map_err(|error| {
                format!(
                    "The document s3://{}/{key} could not be read: {error}",
                    plan.input.bucket
                )
            })?
            .into_bytes();
        // The declared content type is unformatted text, so bytes that are not
        // text are not a document this job can translate: that document is
        // counted as one the job could not process rather than failing the
        // whole job.
        documents.push(Document {
            key,
            text: String::from_utf8(body.to_vec()).ok(),
        });
    }
    Ok(documents)
}

/// Run the job's data plane: translate every document under the input location
/// into each target language and write each translation to the output location.
///
/// `Err` is the reason the job could not be carried out at all, which the job's
/// `FAILED` status reports; `Ok` carries what it did, document by document.
pub async fn run_batch(
    client: &::aws_sdk_s3::Client,
    plan: &BatchPlan,
) -> Result<BatchOutcome, String> {
    let documents = read_documents(client, plan).await?;
    let mut outcome = BatchOutcome {
        input_documents: documents.len(),
        ..BatchOutcome::default()
    };
    // The two bounds on the PRODUCT of this job's factors: how much it may
    // write in total, and how long it may take doing it.
    let started = Instant::now();
    let mut output_budget = MAX_JOB_OUTPUT_BYTES;

    for document in &documents {
        if started.elapsed() > MAX_JOB_RUN_TIME {
            return Err(format!(
                "The job did not finish within the {} seconds this emulator runs one batch \
                 translation for.",
                MAX_JOB_RUN_TIME.as_secs()
            ));
        }
        let mut failure = None;
        let mut written = 0usize;
        match document.text.as_deref() {
            None => {
                failure = Some(format!(
                    "s3://{}/{} could not be translated: it is not the {CONTENT_TYPE_PLAIN} \
                     text the job declared.",
                    plan.input.bucket, document.key
                ));
            }
            Some(text) => match source_language_of(text, &plan.source_language_code) {
                Ok(source) => {
                    for target in &plan.target_language_codes {
                        // Amazon Translate never translates a document into the
                        // language it is already in, so such a pair writes
                        // nothing.
                        if *target == source {
                            continue;
                        }
                        match translate(text, &source, target, plan) {
                            Ok(translated) => {
                                let key = output_key(plan, target, &document.key);
                                if translated.len() > output_budget
                                    || started.elapsed() > MAX_JOB_RUN_TIME
                                {
                                    return Err(format!(
                                        "The job passed what this emulator carries out for one \
                                         batch translation: {MAX_JOB_OUTPUT_BYTES} bytes \
                                         written in {} seconds.",
                                        MAX_JOB_RUN_TIME.as_secs()
                                    ));
                                }
                                output_budget =
                                    output_budget.saturating_sub(translated.len());
                                client
                                    .put_object()
                                    .bucket(&plan.output.bucket)
                                    .key(&key)
                                    .content_type(CONTENT_TYPE_PLAIN)
                                    .body(::aws_sdk_s3::primitives::ByteStream::from(
                                        translated.into_bytes(),
                                    ))
                                    .send()
                                    .await
                                    .map_err(|error| {
                                        format!(
                                            "The output location s3://{}/{key} could not be \
                                             written: {}",
                                            plan.output.bucket,
                                            ::aws_smithy_types::error::display::DisplayErrorContext(
                                                &error
                                            ),
                                        )
                                    })?;
                                written = written.saturating_add(1);
                            }
                            Err(reason) => {
                                failure = Some(format!(
                                    "s3://{}/{} could not be translated: {reason}.",
                                    plan.input.bucket, document.key
                                ));
                            }
                        }
                    }
                }
                Err(reason) => {
                    failure = Some(format!(
                        "s3://{}/{} could not be translated: {reason}.",
                        plan.input.bucket, document.key
                    ));
                }
            },
        }
        if failure.is_none() && written == 0 {
            failure = Some(format!(
                "s3://{}/{} is already in each of the job's target languages.",
                plan.input.bucket, document.key
            ));
        }
        match failure {
            Some(reason) => {
                outcome.documents_with_errors = outcome.documents_with_errors.saturating_add(1);
                // The first document that could not be processed is what the
                // job's message explains; the counts report how many there
                // were.
                if outcome.message.is_none() {
                    outcome.message = Some(reason);
                }
            }
            None => {
                outcome.translated_documents = outcome.translated_documents.saturating_add(1);
            }
        }
    }
    Ok(outcome)
}

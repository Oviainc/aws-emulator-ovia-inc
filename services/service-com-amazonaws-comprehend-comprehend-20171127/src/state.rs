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
//! A custom model is the other kind of record: [`DocumentClassifier`] is what
//! `CreateDocumentClassifier` persists, keyed by the ARN it mints, and
//! [`DocumentClassifier::settle`] is the ONE place its clock-owed `ModelStatus`
//! change is materialised, exactly as [`AnalysisJob::settle`] is for a job.
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
//! [`run_analysis_job`] is the ONE batch run every family's `Start*DetectionJob`
//! performs — list the input objects, read each one, split it into documents, analyse
//! each document and write the `output.tar.gz` — parametrised by the per-document
//! analysis the family models, so no two families can disagree about how an input is
//! read, bounded or reported. [`AnalysisJob::record_run_outcome`] is the ONE place a
//! finished run is recorded, and it is what keeps the `SUBMITTED` -> `IN_PROGRESS`
//! -> terminal walk observable however long the run itself took.
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
//! * `classifier_idempotency_tokens` — an entry is dropped together with the
//!   classifier it points at, so this index cannot outgrow `document_classifiers`.
//! * `endpoints` — [`MAX_ENDPOINT_RECORDS`], enforced by the ONE insertion path,
//!   [`ComAmazonawsComprehendComprehend20171127State::insert_endpoint`], with
//!   oldest-first eviction. The bound also caps the lock-held `settle` sweep
//!   `ListEndpoints` performs.
//! * `endpoint_idempotency_tokens` — an entry is dropped together with the endpoint
//!   it points at, so this index cannot outgrow `endpoints`.
//!
//! Every analysis-job `List*DetectionJobs` bounds the RESPONSE the same way, with
//! [`DEFAULT_JOB_PAGE_SIZE`] and the modelled [`MAX_JOB_PAGE_SIZE`] ceiling, and
//! `ops::list_document_classifiers` does the same with its own `DEFAULT_MAX_RESULTS`.
//!
//! ## Enumerating a family's jobs
//!
//! [`JobFilterParts`] is the ONE reading of the filter members every family's
//! `List*DetectionJobs` shares, and
//! [`ComAmazonawsComprehendComprehend20171127State::analysis_job_page`] is the ONE
//! place selection, ordering and paging over [`AnalysisJob`] records happen, so no
//! two families can disagree about any of them. Its cursor is a submission sequence
//! number, rendered opaque by [`encode_job_page_token`].
//!
//! ## Custom models
//!
//! [`DocumentClassifier`] is one trained (or training) custom classification model,
//! keyed by the `DocumentClassifierArn` that names it everywhere else in the API
//! (Describe/Delete/StopTraining and the resource-policy operations), and
//! [`DocumentClassifier::document_classifier_properties`] projects it into the
//! modelled `DocumentClassifierProperties` shape that `ListDocumentClassifiers` and
//! `DescribeDocumentClassifier` report.
//!
//! ## Endpoints
//!
//! [`Endpoint`] is one model-specific inference endpoint — what `CreateEndpoint`
//! persists, keyed by the `EndpointArn` it mints — and
//! [`Endpoint::endpoint_properties`] projects it into the modelled
//! `EndpointProperties` shape `DescribeEndpoint` and `ListEndpoints` report.
//! [`Endpoint::settle`] is the ONE place its clock-owed `EndpointStatus` change is
//! materialised, exactly as [`AnalysisJob::settle`] is for a job, and
//! [`EndpointProvisioning`] holds the outcome of the provisioning the create
//! actually performed, exactly as [`JobOutcome`] does for a job's run.
//!
//! ## Tags
//!
//! A resource's tags live ON the record they belong to ([`AnalysisJob::tags`],
//! [`DocumentClassifier::tags`]), so the `Tags` a creating operation was called with
//! and the tags `TagResource` associates later are ONE set.
//! [`ComAmazonawsComprehendComprehend20171127State::resource_tags_mut`] is the ONE
//! place an ARN is resolved to that set, so no tagging operation can disagree with
//! another about which ARNs name a resource, and [`apply_tags`] / [`remove_tags`] are
//! the ONE pair that changes it.
//!
//! ## Service-wide rules that are not state
//!
//! [`REALTIME_TEXT_QUOTA_BYTES`], [`DOMINANT_LANGUAGE_TEXT_QUOTA_BYTES`],
//! [`PII_TEXT_QUOTA_BYTES`], [`TOXIC_CONTENT_SEGMENT_QUOTA_BYTES`] and
//! [`text_size_refusal`] are the ONE per-document size quota table and the ONE
//! size-report refusal the real-time (synchronous) inference operations share, so no
//! two of them can disagree about the quota or about what a refusal tells the
//! caller. [`MAX_BATCH_DOCUMENTS`], [`batch_size_limit_refusal`] and
//! [`batch_text_size_refusal`] are the same ONE place for the two quotas a batch
//! request is refused WHOLESALE for — its document count and any one document's
//! size — so the `BatchDetect*` family cannot drift apart on either.
//! [`request_region`], [`epoch_parts`] and [`datetime_parts`] are the other
//! service-wide helpers that live here rather than in one operation's file.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

use crate::ops::types::{
    ClassifierMetadata, DocumentClassifierInputDataConfig, DocumentClassifierOutputDataConfig,
    DocumentClassifierProperties, DominantLanguageDetectionJobProperties, EndpointProperties,
    InputDataConfig, KeyPhrasesDetectionJobProperties, OutputDataConfig,
    PiiEntitiesDetectionJobProperties, PiiOutputDataConfig, RedactionConfig,
    SentimentDetectionJobProperties, Tag, VpcConfig,
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
/// `Start*DetectionJob` and every resource-creating operation declares: "The request
/// contains more tags than can be associated with a resource (50 tags per
/// resource)."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The refusal message for a request carrying more tags than a resource can hold,
/// or `None` when `tags` is within [`MAX_TAGS_PER_RESOURCE`].
///
/// The bound and its wording live here rather than in one operation, so every
/// operation that accepts `Tags` refuses the same count the same way; each handler
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

/// The refusal message for a request that would leave a resource already holding
/// `held` carrying more tags than it can, or `None` when the merged set is within
/// [`MAX_TAGS_PER_RESOURCE`].
///
/// `TooManyTagsException`'s own documentation states that the bound counts BOTH
/// sides — "The maximum number of tags includes both existing tags and those
/// included in your current request" — so a request is measured against the set it
/// would leave behind, not against its own length. A tag is identified by its key,
/// so a request that only gives keys the resource already holds a new value adds
/// nothing to the count.
pub fn too_many_merged_tags_message(held: &[Tag], incoming: &[Tag]) -> Option<String> {
    let merged = merged_tag_count(held, incoming);
    (merged > MAX_TAGS_PER_RESOURCE).then(|| {
        format!(
            "The request would leave the resource with {merged} tags, more than the \
             {MAX_TAGS_PER_RESOURCE} tags that can be associated with a resource, counting \
             both its existing tags and those in this request."
        )
    })
}

/// How many tags a resource holding `held` would hold once `incoming` is applied:
/// the number of DISTINCT keys across both, because a resource's tags are keyed by
/// their key.
fn merged_tag_count(held: &[Tag], incoming: &[Tag]) -> usize {
    held.iter()
        .chain(incoming)
        .map(|tag| tag.key.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

/// The refusal message for a request naming more tag keys than a resource can hold,
/// or `None` when `tag_keys` is within [`MAX_TAGS_PER_RESOURCE`].
///
/// `TooManyTagKeysException` states the same bound from the removal side: "The
/// request contains more tag keys than can be associated with a resource (50 tag
/// keys per resource)."
pub fn too_many_tag_keys_message(tag_keys: &[String]) -> Option<String> {
    (tag_keys.len() > MAX_TAGS_PER_RESOURCE).then(|| {
        format!(
            "The request contains {} tag keys, more than the {MAX_TAGS_PER_RESOURCE} tag \
             keys that can be associated with a resource.",
            tag_keys.len()
        )
    })
}

/// Associate `incoming` with a resource already holding `held`.
///
/// A resource's tags are a key-value MAP: a tag whose key the resource already
/// holds has its value replaced, and only a key it does not hold is added — so
/// associating the same tag twice leaves one tag, and the same key with a new value
/// leaves the new value. Repeated keys within one request are applied in order, so
/// the last one given wins; the model declares neither `uniqueItems` on `TagList`
/// nor an error for a repeated key.
pub fn apply_tags(held: &mut Vec<Tag>, incoming: Vec<Tag>) {
    for tag in incoming {
        match held.iter_mut().find(|existing| existing.key == tag.key) {
            Some(existing) => existing.value = tag.value,
            None => held.push(tag),
        }
    }
}

/// Remove from `held` every tag whose key `tag_keys` names.
///
/// A key the resource does not hold is removed by having nothing to remove: the
/// request asked for that key to be absent and it is, and the model declares no
/// error for it.
pub fn remove_tags(held: &mut Vec<Tag>, tag_keys: &[String]) {
    held.retain(|tag| !tag_keys.contains(&tag.key));
}

/// The message a `ResourceNotFoundException` carries, whichever operation raised it
/// and whatever `ResourceArn` was asked for.
///
/// Amazon Comprehend answers a well-formed ARN that names no resource with ONE fixed
/// sentence, carrying the service's own reason code and NOTHING derived from the
/// request: the refusal states that the service holds no such resource and never
/// echoes the ARN back. Fixed for the same reason as [`JOB_NOT_FOUND_MESSAGE`], and
/// shared by every operation that resolves a resource by ARN — the tagging
/// operations (`TagResource`, `UntagResource`, `ListTagsForResource`), the
/// custom-model Describe/Delete/StopTraining operations and the resource-policy
/// operations — so no two of them can disclose a different amount about the request.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str =
    "RESOURCE_NOT_FOUND: Could not find specified resource.";

/// The message a `ResourceNotFoundException` carries when the resource the request
/// named was a custom MODEL and the operation words its refusal for that class of
/// resource rather than for a resource in general.
///
/// Fixed for the same reason as [`RESOURCE_NOT_FOUND_MESSAGE`] — nothing of the
/// request reaches the sentence — but model-flavoured, because the operations that
/// use it resolve a trained model by its ARN and say so, which is what lets a client
/// tell "that model is gone" from a generic resource miss.
pub const MODEL_NOT_FOUND_MESSAGE: &str = "Could not find specified model.";

/// The actions a custom-model resource policy can grant.
///
/// A model's resource-based policy exists to "authorize an entity in another Amazon
/// Web Services account to import the custom model, which replicates it in Amazon
/// Comprehend in their account" (`PutResourcePolicy`'s own documentation), and
/// importing a model is `ImportModel` — so that is the whole of what such a policy
/// can allow, and a statement naming any other action names one the service does not
/// support in this policy.
pub const SUPPORTED_MODEL_POLICY_ACTIONS: &[&str] = &["comprehend:ImportModel"];

/// The message the `InvalidRequestException` carries when a policy document names an
/// action a custom-model resource policy cannot grant.
///
/// The service's own fixed text behind its `POLICY_INVALID_INPUT` reason code: it
/// states the class of fault and echoes nothing back from the document, so it is the
/// same sentence whichever action the policy named.
pub const UNSUPPORTED_POLICY_ACTION_MESSAGE: &str =
    "POLICY_INVALID_INPUT: The provided policy uses unsupported action.";

/// The message the `InvalidRequestException` carries when the policy document is not
/// a JSON document at all.
///
/// `ResourcePolicy` is "the JSON resource-based policy to attach to your custom
/// model", which the member's documentation spells out ("To provide valid JSON for
/// your policy, enclose the attribute names and values in double quotes"), so a
/// string that does not parse as JSON is not a policy. Fixed text behind the same
/// reason code, echoing nothing of the document back.
pub const MALFORMED_POLICY_MESSAGE: &str =
    "POLICY_INVALID_INPUT: The provided policy is not valid JSON.";

/// Why the service refuses `policy` as a custom-model resource policy, or `None`
/// when it accepts the document.
///
/// The document is judged on its OWN content, independently of any resource: both
/// `PutResourcePolicy`'s `ResourcePolicy` and a creation's `ModelPolicy` attach the
/// same policy, and both are validated before the request's resources are resolved,
/// so a document the service will not accept earns the same invalid-request refusal
/// whatever ARN the request named.
pub fn policy_document_refusal(policy: &str) -> Option<&'static str> {
    let Ok(document) = serde_json::from_str::<JsonValue>(policy) else {
        return Some(MALFORMED_POLICY_MESSAGE);
    };
    policy_actions(&document)
        .into_iter()
        .any(|action| !grants_supported_action(action))
        .then_some(UNSUPPORTED_POLICY_ACTION_MESSAGE)
}

/// Every action the statements of `document` name.
///
/// A policy document's `Statement` is one statement or a list of them, and a
/// statement's `Action` is one action or a list of them, so both forms are walked.
/// A document that names no action at all (no statement, or statements carrying no
/// `Action`) grants nothing and so names nothing unsupported.
fn policy_actions(document: &JsonValue) -> Vec<&str> {
    let statements: Vec<&JsonValue> = match document.get("Statement") {
        Some(JsonValue::Array(items)) => items.iter().collect(),
        Some(single) => vec![single],
        None => Vec::new(),
    };
    let mut actions = Vec::new();
    for statement in statements {
        match statement.get("Action") {
            Some(JsonValue::Array(items)) => {
                actions.extend(items.iter().filter_map(JsonValue::as_str));
            }
            Some(JsonValue::String(single)) => actions.push(single.as_str()),
            _ => {}
        }
    }
    actions
}

/// Whether `action`, as a policy statement writes it, names at least one of
/// [`SUPPORTED_MODEL_POLICY_ACTIONS`].
///
/// An action in a policy statement is matched case-insensitively and may carry the
/// `*` (any sequence) and `?` (any one character) wildcards, so a statement allowing
/// `comprehend:*` or `comprehend:Import*` names the import action a model policy
/// grants, while one allowing another operation of this service, or another
/// service's, names none of them.
///
/// An action carrying no wildcard — every ordinary statement — is compared directly
/// and reaches no matching engine at all. One that does carry a wildcard becomes a
/// linear-time expression over a FIXED, short subject (the supported action names,
/// which are this file's own constants): the request's part is only the pattern, its
/// length is already bounded by `Policy`'s modelled 1..20000 constraint, which the
/// boundary validator applies before any handler runs, and a pattern too large for
/// the engine's own size limit to compile names no supported action.
fn grants_supported_action(action: &str) -> bool {
    if !action.contains(['*', '?']) {
        return SUPPORTED_MODEL_POLICY_ACTIONS
            .iter()
            .any(|supported| supported.eq_ignore_ascii_case(action));
    }
    let expression: String = action
        .chars()
        .map(|character| match character {
            '*' => ".*".to_string(),
            '?' => ".".to_string(),
            other => regex::escape(&other.to_string()),
        })
        .collect();
    let Ok(pattern) = Regex::new(&format!("(?i)^{expression}$")) else {
        return false;
    };
    SUPPORTED_MODEL_POLICY_ACTIONS
        .iter()
        .any(|supported| pattern.is_match(supported))
}

/// Why a job whose batch run this emulator does not perform ends in the modelled
/// `FAILED` state, naming the analysis that was not carried out and both S3
/// locations the submission pointed at.
///
/// Running one of these jobs means reading the documents from the Amazon S3 location
/// `InputDataConfig` names, analysing each one and writing the analysed output back
/// to `OutputDataConfig`. The families whose run this emulator DOES perform —
/// dominant-language detection, key-phrase extraction and sentiment analysis — record
/// what their run actually did ([`run_analysis_job`]); a family whose per-document
/// analysis this emulator cannot carry out ends with this reason instead of reporting
/// a `COMPLETED` run that nothing performed. `analysis` names the family's work
/// ("PII entity detection", …).
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
    /// `StartPiiEntitiesDetectionJob` and its Describe/List/Stop operations.
    PiiEntitiesDetection,
}

impl JobFamily {
    /// The resource type segment this family's `JobArn` carries, e.g.
    /// `arn:aws:comprehend:<region>:<account>:sentiment-detection-job/<JobId>`.
    ///
    /// Each value is the one the family's own `JobArn` documentation states — the
    /// PII family's says
    /// `arn:<partition>:comprehend:<region>:<account-id>:pii-entities-detection-job/<job-id>`.
    pub fn arn_resource_type(self) -> &'static str {
        match self {
            Self::SentimentDetection => "sentiment-detection-job",
            Self::DominantLanguageDetection => "dominant-language-detection-job",
            Self::KeyPhrasesDetection => "key-phrases-detection-job",
            Self::PiiEntitiesDetection => "pii-entities-detection-job",
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
            // AWS names a PII job's output directory
            // `<account-id>-PII-<JobId>`.
            Self::PiiEntitiesDetection => "PII",
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
    /// The family's own analysis mode, for a family whose input models one (the PII
    /// family's `Mode`); `None` for a family whose input models none.
    pub mode: Option<&'a str>,
    /// The `RedactionConfig` the submission carried, for the family whose input
    /// models one; `None` otherwise.
    pub redaction_config: Option<&'a RedactionConfig>,
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
        let redaction_config = self.redaction_config.map_or_else(
            || "absent".to_string(),
            |redaction| {
                format!(
                    "{}|{}|{}",
                    redaction
                        .pii_entity_types
                        .as_ref()
                        .map_or_else(|| "absent".to_string(), |types| types.join("+")),
                    rendered(redaction.mask_mode.as_deref()),
                    rendered(redaction.mask_character.as_deref()),
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
            // The members only one family's input models, rendered for every family
            // so no part can shift position between families.
            rendered(self.mode),
            redaction_config,
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
    /// The family's own analysis mode, for a family that models one: the PII
    /// family's `Mode` (`ONLY_OFFSETS` | `ONLY_REDACTION`). `None` for a family
    /// whose input models no mode.
    pub mode: Option<String>,
    /// The `RedactionConfig` a PII job was submitted with, if any; `None` for every
    /// other family, whose input models none.
    pub redaction_config: Option<RedactionConfig>,
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

    /// Record what this job's run reported, as of the instant `now` it reported it.
    ///
    /// The clock cannot have owed a status change for a phase that had not elapsed
    /// yet: a run that took longer than [`QUEUE_SPAN`] (reading object storage,
    /// analysing a large input) was still queued-or-running the whole time it ran, so
    /// the instants the remaining transitions are owed at are carried forward from
    /// the report rather than left in the past. That is what keeps every phase
    /// OBSERVABLE — a caller polling after the submission returns sees `IN_PROGRESS`
    /// before the terminal status however long the run took — while a transition
    /// whose span really has not elapsed yet keeps the instant it was submitted with.
    ///
    /// Only the outcome is recorded here; [`Self::settle`] is still the one place a
    /// status is moved.
    pub fn record_run_outcome(&mut self, outcome: JobOutcome, now: SystemTime) {
        self.terminal_outcome = Some(outcome);
        if now >= self.in_progress_due_at {
            self.in_progress_due_at = now + QUEUE_SPAN;
        }
        let earliest_terminal = self.in_progress_due_at + PROCESSING_SPAN;
        if self.terminal_due_at < earliest_terminal {
            self.terminal_due_at = earliest_terminal;
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
    /// Project this record into the PII family's modelled properties shape.
    ///
    /// Hand-written rather than an instantiation of `job_properties_projections!`
    /// above: this family's shape declares members the others do not — `Mode` and
    /// `RedactionConfig` — reports its output location as the family's own
    /// `PiiOutputDataConfig`, and models neither a volume key nor a VPC
    /// configuration.
    ///
    /// `OutputDataConfig` carries the location the submission supplied — the member
    /// documents exactly that: "The output data configuration that you supplied when
    /// you created the PII entities detection job."
    pub fn pii_entities_detection_job_properties(&self) -> PiiEntitiesDetectionJobProperties {
        PiiEntitiesDetectionJobProperties {
            job_id: Some(self.job_id.clone()),
            job_arn: Some(self.job_arn.clone()),
            job_name: self.job_name.clone(),
            job_status: Some(self.job_status.clone()),
            message: self.message.clone(),
            submit_time: Some(::aws_smithy_types::DateTime::from(self.submit_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
            input_data_config: Some(self.input_data_config.clone()),
            output_data_config: Some(PiiOutputDataConfig {
                s3_uri: self.output_data_config.s3_uri.clone(),
                kms_key_id: self.output_data_config.kms_key_id.clone(),
            }),
            redaction_config: self.redaction_config.clone(),
            language_code: self.language_code.clone(),
            data_access_role_arn: Some(self.data_access_role_arn.clone()),
            mode: self.mode.clone(),
        }
    }

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

// --- Reading and writing an analysis job's object storage -----------------------
//
// Every `Start*DetectionJob` run, and the training run behind
// `CreateDocumentClassifier`, reaches the SAME object storage the same way, so the
// client, the URI split, the listing, the bounds and the archive writer live here
// once rather than once per operation.

/// The most input objects one run reads, and the most bytes it reads from one object
/// and in total.
///
/// AWS publishes its own per-job document and size quotas; these are the EMULATOR's
/// bounds on work done inside one request, so no submission can make this process
/// read an unbounded amount of object storage or hold the service for an unbounded
/// time. A submission that points past them is not silently truncated: the run ends
/// in the modelled failure state naming the bound it hit.
pub const MAX_INPUT_OBJECTS: usize = 200;
/// The most bytes read from one input object (see [`MAX_INPUT_OBJECTS`]).
pub const MAX_INPUT_OBJECT_BYTES: usize = 1024 * 1024;
/// The most bytes read across a run's whole input (see [`MAX_INPUT_OBJECTS`]).
pub const MAX_TOTAL_INPUT_BYTES: usize = 8 * 1024 * 1024;

/// The most documents one job analyses, and the most output bytes it accumulates.
///
/// The input bounds above do NOT bound these: `ONE_DOC_PER_LINE` makes every line of
/// an input object its own document, and every document costs an output entry that
/// repeats the object's key, so a bounded input can ask for an unbounded multiple of
/// itself in output. These bounds are on what the run PRODUCES, checked as each
/// entry is appended, so the archive one request builds can never outgrow them; a
/// run that reaches one ends in the modelled `FAILED` state naming it.
pub const MAX_OUTPUT_DOCUMENTS: usize = 100_000;
/// The most output bytes one job accumulates (see [`MAX_OUTPUT_DOCUMENTS`]).
pub const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
/// How many documents an analysis pass handles between yields.
///
/// Analysing a document is CPU work with no await of its own, so the pass hands the
/// runtime a yield point this often: a cancelled request or a shutdown has somewhere
/// to land even while one object of many documents is being analysed.
pub const DOCUMENTS_PER_YIELD: usize = 256;

/// How each input file is split into documents when a submission sets no
/// `InputFormat`.
///
/// The model declares no default for `InputDataConfig$InputFormat`, so this is the
/// emulator's choice, and it is the coarser of the two: each file is one document
/// (`ONE_DOC_PER_FILE`), so an output entry names the file it came from and carries
/// no line number.
pub const DEFAULT_INPUT_FORMAT: &str = "ONE_DOC_PER_FILE";

/// The single file an analysis job's output archive holds, one JSON document per
/// line — the form Comprehend's asynchronous analysis output takes inside the
/// `output.tar.gz` that `OutputDataConfig$S3Uri` documents.
pub const OUTPUT_ARCHIVE_ENTRY: &str = "output";

/// An `s3://bucket/key` URI split into its parts.
///
/// The modelled `S3Uri` pattern (`^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$`)
/// is enforced before a handler runs, so a URI reaching here always has a bucket and
/// may have a key.
#[derive(Debug, Clone)]
pub struct S3Location {
    pub bucket: String,
    pub key: String,
}

/// Split a modelled `S3Uri` into bucket and key; `None` for a string the pattern
/// would not have admitted.
pub fn parse_s3_uri(uri: &str) -> Option<S3Location> {
    let rest = uri.strip_prefix("s3://")?;
    let (bucket, key) = match rest.split_once('/') {
        Some((bucket, key)) => (bucket, key),
        None => (rest, ""),
    };
    (!bucket.is_empty()).then(|| S3Location {
        bucket: bucket.to_string(),
        key: key.to_string(),
    })
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated
/// service: it routes in process, so no network client is configured here and this
/// crate names no other service's crate. `region` is the request's own signing
/// region, so a run reads and writes where the caller's own S3 requests go.
pub fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a
        // virtual-host bucket name is not a host this emulator resolves, so every
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

/// The input objects under `input` with their listed sizes, in listing order, or the
/// reason the listing could not be used.
///
/// The SIZE is carried so a run can refuse an object, or a whole input, that is past
/// its byte bounds BEFORE fetching and buffering any of it.
pub async fn input_objects(
    client: &::aws_sdk_s3::Client,
    input: &S3Location,
) -> Result<Vec<(String, usize)>, String> {
    let mut objects: Vec<(String, usize)> = Vec::new();
    let mut continuation: Option<String> = None;
    loop {
        let page = client
            .list_objects_v2()
            .bucket(&input.bucket)
            .prefix(&input.key)
            .set_continuation_token(continuation.clone())
            .send()
            .await
            .map_err(|error| {
                ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string()
            })?;
        for object in page.contents() {
            let Some(key) = object.key() else { continue };
            // A key ending in the delimiter is a folder marker rather than a
            // document. An EMPTY object is a document: an analysis engine's answer
            // for text it can place nowhere is an empty list, which is the honest
            // result for an empty file.
            if key.ends_with('/') {
                continue;
            }
            objects.push((key.to_string(), object.size().unwrap_or_default().max(0) as usize));
            if objects.len() > MAX_INPUT_OBJECTS {
                return Err(format!(
                    "The input at s3://{}/{} holds more than the {MAX_INPUT_OBJECTS} input \
                     objects this emulator reads in one job.",
                    input.bucket, input.key,
                ));
            }
        }
        match page.next_continuation_token() {
            Some(token) => continuation = Some(token.to_string()),
            None => break,
        }
    }
    Ok(objects)
}

/// Read every object under `input`, bounded by [`MAX_INPUT_OBJECTS`] and the byte
/// bounds beside it, and hand each one's key and UTF-8 text to `visit`.
///
/// The ONE input-reading path: a `Start*DetectionJob`'s analysis run and
/// `CreateDocumentClassifier`'s training run both read their object storage through
/// it, so neither can read an input the other would have refused. An `Err` is the
/// reason the run could not be carried out, ready for the caller's own failure
/// state.
pub async fn input_objects_within_bounds(
    client: &::aws_sdk_s3::Client,
    input: &S3Location,
    uri: &str,
) -> Result<Vec<(String, usize)>, String> {
    let objects = input_objects(client, input)
        .await
        .map_err(|reason| format!("Could not list the input documents at {uri}: {reason}"))?;
    if objects.is_empty() {
        return Err(format!("No input documents were found at {uri}."));
    }
    // The byte bounds are decided from the LISTED sizes, before a single object is
    // fetched, so an input past them costs no buffering at all.
    if let Some((key, size)) = objects
        .iter()
        .find(|(_, size)| *size > MAX_INPUT_OBJECT_BYTES)
    {
        return Err(format!(
            "The input document s3://{}/{key} is {size} bytes, more than the \
             {MAX_INPUT_OBJECT_BYTES} bytes this emulator reads from one document.",
            input.bucket,
        ));
    }
    let listed_bytes: usize = objects.iter().map(|(_, size)| *size).sum();
    if listed_bytes > MAX_TOTAL_INPUT_BYTES {
        return Err(format!(
            "The input at {uri} is {listed_bytes} bytes, more than the \
             {MAX_TOTAL_INPUT_BYTES} bytes this emulator reads in one job.",
        ));
    }
    Ok(objects)
}

/// Fetch one input object as UTF-8 text, charged against `remaining_bytes` — the
/// bytes this run may still read across its whole input.
///
/// The second half of the ONE input-reading path (see
/// [`input_objects_within_bounds`]), so every run fetches, bounds and decodes an
/// object the same way. An `Err` is the reason the run could not be carried out.
pub async fn read_input_object(
    client: &::aws_sdk_s3::Client,
    input: &S3Location,
    key: &str,
    remaining_bytes: &mut usize,
) -> Result<String, String> {
    let object = client
        .get_object()
        .bucket(&input.bucket)
        .key(key)
        .send()
        .await
        .map_err(|error| {
            format!(
                "Could not read the input document s3://{}/{key}: {}",
                input.bucket,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error),
            )
        })?;
    let body = object.body.collect().await.map_err(|error| {
        format!(
            "Could not read the input document s3://{}/{key}: {error}",
            input.bucket,
        )
    })?;
    let body = body.into_bytes();
    // The listed size is what the bounds in `input_objects_within_bounds` were
    // decided from; these are the backstop for an object whose fetched bytes
    // disagree with its listing.
    if body.len() > MAX_INPUT_OBJECT_BYTES {
        return Err(format!(
            "The input document s3://{}/{key} is {} bytes, more than the \
             {MAX_INPUT_OBJECT_BYTES} bytes this emulator reads from one document.",
            input.bucket,
            body.len(),
        ));
    }
    if body.len() > *remaining_bytes {
        return Err(format!(
            "The input at s3://{}/{} is larger than the {MAX_TOTAL_INPUT_BYTES} bytes this \
             emulator reads in one job.",
            input.bucket, input.key,
        ));
    }
    *remaining_bytes -= body.len();
    String::from_utf8(body.to_vec()).map_err(|_| {
        format!(
            "The input document s3://{}/{key} is not UTF-8 text; this emulator extracts \
             text from plain-text documents only.",
            input.bucket,
        )
    })
}

/// One document a run analysed: where it came from, and the line it was on for a
/// `ONE_DOC_PER_LINE` input.
///
/// Borrows the object's key and body rather than copying them, so splitting an
/// object into documents allocates nothing per document.
#[derive(Debug)]
pub struct Document<'a> {
    /// The input object's key with the submitted prefix removed — the file the
    /// output entry names.
    pub file: &'a str,
    /// The document's line within that file, for `ONE_DOC_PER_LINE` only.
    pub line: Option<usize>,
    pub text: &'a str,
}

/// Split one input object into the documents its `InputFormat` says it holds.
///
/// `ONE_DOC_PER_FILE` — "Each file is considered a separate document";
/// `ONE_DOC_PER_LINE` — "Each line in a file is considered a separate document",
/// where a blank line holds no document.
///
/// Lazy: the documents are yielded one at a time, so a file of a million lines never
/// exists as a million values at once.
pub fn documents_in<'a>(
    file: &'a str,
    body: &'a str,
    input_format: &str,
) -> Box<dyn Iterator<Item = Document<'a>> + Send + 'a> {
    if input_format == "ONE_DOC_PER_LINE" {
        return Box::new(
            body.lines()
                .enumerate()
                .filter(|(_, line)| !line.trim().is_empty())
                .map(move |(index, line)| Document {
                    file,
                    line: Some(index),
                    text: line,
                }),
        );
    }
    Box::new(::std::iter::once(Document {
        file,
        line: None,
        text: body,
    }))
}

/// The file an output entry names for the object `key` read under the prefix
/// `input`: the key with the submitted prefix removed, so an entry points back at the
/// document it was produced from.
///
/// An input URI that names ONE file — "if the prefix is a single file, Amazon
/// Comprehend uses that file as input" (`InputDataConfig$S3Uri`) — leaves nothing
/// under the prefix, so the entry names that object.
fn output_entry_file<'a>(input: &S3Location, key: &'a str) -> &'a str {
    let relative = key
        .strip_prefix(input.key.as_str())
        .unwrap_or(key)
        .trim_start_matches('/');
    if relative.is_empty() {
        key.split('/').next_back().unwrap_or(key)
    } else {
        relative
    }
}

/// The `output.tar.gz` bytes for `body`: a gzip-compressed tar holding the single
/// [`OUTPUT_ARCHIVE_ENTRY`] file, which is what `OutputDataConfig$S3Uri` documents
/// ("a compressed archive that contains the output of the operation").
pub fn output_archive(body: &str) -> ::std::io::Result<Vec<u8>> {
    let encoder = ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    let mut archive = ::tar::Builder::new(encoder);
    let mut header = ::tar::Header::new_gnu();
    header.set_path(OUTPUT_ARCHIVE_ENTRY)?;
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    // The archive is a function of the analysed documents alone: nothing about the
    // machine or the moment it was built takes part, so the same input produces the
    // same bytes.
    header.set_mtime(0);
    header.set_cksum();
    archive.append(&header, body.as_bytes())?;
    let mut encoder = archive.into_inner()?;
    encoder.flush()?;
    encoder.finish()
}

/// Run one asynchronous analysis job: read every input document, analyse it with
/// `analyse`, and write the output archive under the submitted output prefix.
///
/// The ONE batch run every family shares. `analyse` is the only per-family part —
/// the JSON entry the family's analysis produces for one document — so the way an
/// input is listed, bounded, split and reported is identical for all of them.
///
/// Answers a [`JobOutcome`] rather than an error channel: the run is the job's own
/// work, so what it could not do is the job's modelled `FAILED` state and its
/// `Message`, never a failure of the `Start*DetectionJob` request that submitted it.
pub async fn run_analysis_job<Analyse>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    family: JobFamily,
    job_id: &str,
    input_data_config: &InputDataConfig,
    output_data_config: &OutputDataConfig,
    mut analyse: Analyse,
) -> JobOutcome
where
    Analyse: FnMut(&Document<'_>) -> ::serde_json::Value,
{
    let Some(input) = parse_s3_uri(&input_data_config.s3_uri) else {
        return JobOutcome::failed(format!(
            "The input location {} is not an Amazon S3 URI.",
            input_data_config.s3_uri
        ));
    };
    let Some(output) = parse_s3_uri(&output_data_config.s3_uri) else {
        return JobOutcome::failed(format!(
            "The output location {} is not an Amazon S3 URI.",
            output_data_config.s3_uri
        ));
    };
    // Extracting text from PDF and image files is Amazon Textract's work, which this
    // emulator does not perform, so a job configured for it is not reported as
    // having analysed anything.
    if input_data_config.document_reader_config.is_some() {
        return JobOutcome::failed(
            "DocumentReaderConfig asks for Amazon Textract text extraction, which this \
             emulator does not perform, so no document was analyzed."
                .to_string(),
        );
    }

    let client = s3_client(ctx, region);
    let objects =
        match input_objects_within_bounds(&client, &input, &input_data_config.s3_uri).await {
            Ok(objects) => objects,
            Err(reason) => return JobOutcome::failed(reason),
        };
    let input_format = input_data_config
        .input_format
        .as_deref()
        .unwrap_or(DEFAULT_INPUT_FORMAT);
    // The output is accumulated as the single archive member it becomes, entry by
    // entry, so it is never held twice and never grows past [`MAX_OUTPUT_BYTES`].
    let mut output_body = String::new();
    let mut documents = 0usize;
    let mut remaining_bytes = MAX_TOTAL_INPUT_BYTES;
    for (key, _) in &objects {
        let text = match read_input_object(&client, &input, key, &mut remaining_bytes).await {
            Ok(text) => text,
            Err(reason) => return JobOutcome::failed(reason),
        };
        let file = output_entry_file(&input, key);
        for document in documents_in(file, &text, input_format) {
            documents += 1;
            if documents > MAX_OUTPUT_DOCUMENTS {
                return JobOutcome::failed(format!(
                    "The input at {} holds more than the {MAX_OUTPUT_DOCUMENTS} documents this \
                     emulator analyzes in one job.",
                    input_data_config.s3_uri,
                ));
            }
            // Analysis is CPU work with no await of its own, so the pass hands the
            // runtime a yield point every [`DOCUMENTS_PER_YIELD`] documents: a
            // cancelled request or a shutdown has somewhere to land even while one
            // object of many documents is being analysed.
            if documents.is_multiple_of(DOCUMENTS_PER_YIELD) {
                ::tokio::task::yield_now().await;
            }
            let entry = analyse(&document).to_string();
            if output_body.len() + entry.len() + 1 > MAX_OUTPUT_BYTES {
                return JobOutcome::failed(format!(
                    "The output for {} is larger than the {MAX_OUTPUT_BYTES} bytes this \
                     emulator writes in one job.",
                    input_data_config.s3_uri,
                ));
            }
            output_body.push_str(&entry);
            output_body.push('\n');
        }
    }

    let archive = match output_archive(&output_body) {
        Ok(archive) => archive,
        Err(error) => {
            return JobOutcome::failed(format!("Could not build the output archive: {error}"))
        }
    };
    let archive_key = output_archive_key(&output.key, family, job_id);
    if let Err(error) = client
        .put_object()
        .bucket(&output.bucket)
        .key(&archive_key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(archive))
        .content_type("application/x-gzip")
        .send()
        .await
    {
        return JobOutcome::failed(format!(
            "Analyzed {documents} document(s) but could not write the output to \
             s3://{}/{archive_key}: {}",
            output.bucket,
            ::aws_smithy_types::error::display::DisplayErrorContext(&error),
        ));
    }

    JobOutcome::completed(format!("s3://{}/{archive_key}", output.bucket))
}

/// Records against the stored job what its run reported — and, if it is dropped
/// without a report, the modelled failure for a run that will never report.
///
/// A job's terminal status is only revealed once its outcome is recorded, so a run
/// whose handler never gets to report (the future dropped because the client went
/// away, a panic the server contains) would otherwise leave the job in `IN_PROGRESS`
/// for the life of the process. `Drop` runs on EVERY exit from the handler, which is
/// what makes that unreachable.
///
/// `Drop` takes the state lock, so a recorder must not be dropped while a
/// [`::engine::ServiceState::lock`] guard is alive in the same scope.
pub struct RunOutcomeRecorder {
    store: ::engine::ServiceState<ComAmazonawsComprehendComprehend20171127State>,
    family: JobFamily,
    job_id: String,
    reported: bool,
}

impl RunOutcomeRecorder {
    /// Watch the run of the job `job_id` of `family`.
    pub fn new(
        store: ::engine::ServiceState<ComAmazonawsComprehendComprehend20171127State>,
        family: JobFamily,
        job_id: String,
    ) -> Self {
        Self {
            store,
            family,
            job_id,
            reported: false,
        }
    }

    /// Record `outcome` as what this job's run reported. The clock reveals it once
    /// the job's processing span is over, never before.
    pub fn report(&mut self, outcome: JobOutcome) {
        self.reported = true;
        let now = SystemTime::now();
        let mut store = self.store.lock();
        if let Some(job) = store
            .analysis_jobs
            .get_mut(&(self.family, self.job_id.clone()))
        {
            job.record_run_outcome(outcome, now);
        }
    }
}

impl Drop for RunOutcomeRecorder {
    fn drop(&mut self) {
        if self.reported {
            return;
        }
        let now = SystemTime::now();
        let mut store = self.store.lock();
        if let Some(job) = store
            .analysis_jobs
            .get_mut(&(self.family, self.job_id.clone()))
        {
            if job.terminal_outcome.is_none() {
                job.record_run_outcome(
                    JobOutcome::failed(
                        "The analysis run did not complete: the request that started it ended \
                         before the run reported an outcome.",
                    ),
                    now,
                );
            }
        }
    }
}

/// How long a freshly created document classifier stays `SUBMITTED` before the
/// clock owes it `TRAINING`.
///
/// The emulator's own span, not a documented AWS duration: training a real custom
/// model takes tens of minutes, which no test can wait for. It is deliberately
/// non-zero, so a caller that describes a classifier straight back still observes
/// `SUBMITTED`.
pub const CLASSIFIER_SUBMITTED_SPAN: Duration = Duration::from_millis(300);

/// How long a classifier stays `TRAINING` before the clock owes it its terminal
/// `ModelStatus`.
///
/// The emulator's own span, chosen for the same reason as
/// [`CLASSIFIER_SUBMITTED_SPAN`] and deliberately at the top of the band the project
/// allows, so an out-of-process client that provisions a classifier and then stops
/// its training can still catch it `TRAINING`.
pub const CLASSIFIER_TRAINING_SPAN: Duration = Duration::from_millis(450);

/// How long a classifier stays `STOP_REQUESTED` before the clock owes it `STOPPED`.
/// The emulator's own span, chosen for the same reason as
/// [`CLASSIFIER_SUBMITTED_SPAN`].
pub const CLASSIFIER_STOPPING_SPAN: Duration = Duration::from_millis(300);

/// How long a classifier stays `DELETING` before the clock owes its removal.
///
/// `DeleteDocumentClassifier` documents the asynchrony this span stands for: "This
/// is an asynchronous action that puts the classifier into a DELETING state, and it
/// is then removed by a background job." The span is the emulator's own, chosen for
/// the same reason as [`CLASSIFIER_SUBMITTED_SPAN`], and non-zero so a caller that
/// describes the classifier straight after the delete still observes `DELETING`.
pub const CLASSIFIER_DELETING_SPAN: Duration = Duration::from_millis(300);

/// The `DocumentClassifierMode` a creation that names none is trained in.
///
/// `CreateDocumentClassifier`'s `Mode` member is optional and the service documents
/// `MULTI_CLASS` as what it defaults to.
pub const DEFAULT_CLASSIFIER_MODE: &str = "MULTI_CLASS";

/// The `DocumentClassifierDataFormat` an `InputDataConfig` that names none uses.
///
/// From the modelled member documentation of
/// `DocumentClassifierInputDataConfig$DataFormat`: "If you don't specify a value,
/// Amazon Comprehend uses `COMPREHEND_CSV` as the default."
pub const DEFAULT_CLASSIFIER_DATA_FORMAT: &str = "COMPREHEND_CSV";

/// One resource-based policy attached to a custom model.
///
/// `PutResourcePolicy` documents the revision contract this record stands for: "Each
/// time you modify a policy, Amazon Comprehend assigns a new revision ID, and it
/// deletes the prior version of the policy." So a model holds at most one policy, the
/// revision id changes on every modification, the creation instant is the one the
/// FIRST attachment happened at and the modification instant is the latest one's.
#[derive(Debug, Clone)]
pub struct ResourcePolicy {
    /// The JSON policy document the caller attached, byte for byte.
    pub policy: String,
    /// The revision id this version of the policy carries, as
    /// [`ComAmazonawsComprehendComprehend20171127State::mint_policy_revision_id`]
    /// mints it.
    pub revision_id: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// The number of tokens a trained classifier's vocabulary holds at most.
///
/// The EMULATOR's bound, so one training request cannot make this process hold an
/// unbounded model: once the vocabulary is full the fit keeps counting the tokens it
/// already knows and ignores new ones, which is the standard bounded-vocabulary
/// treatment and leaves every document's known tokens counted.
pub const MAX_CLASSIFIER_VOCABULARY: usize = 50_000;

/// The number of labelled documents one training run fits a model on at most.
///
/// The EMULATOR's bound, for the same reason as [`MAX_CLASSIFIER_VOCABULARY`]. A
/// training input holding more rows than this is not silently truncated: the run
/// ends `IN_ERROR` naming the bound.
pub const MAX_TRAINING_DOCUMENTS: usize = 20_000;

/// The number of distinct labels one model, or one training row, may carry.
///
/// Amazon Comprehend publishes 1,000 as the per-classifier quota for both modes (the
/// "number of classes" of a multi-class classifier and the "number of labels" of a
/// multi-label one), so a corpus naming more labels than this is past what a custom
/// classifier can hold and the run ends `IN_ERROR` naming the bound rather than
/// fitting a model nobody could train.
pub const MAX_CLASSIFIER_LABELS: usize = 1_000;

/// The number of (label, token) counts one fitted model holds at most.
///
/// The EMULATOR's bound on the PRODUCT the per-label counts form: a model's size is
/// the sum over labels of the distinct tokens their documents hold, so bounding the
/// vocabulary ([`MAX_CLASSIFIER_VOCABULARY`]) and the label count
/// ([`MAX_CLASSIFIER_LABELS`]) separately still leaves their product unbounded. The
/// pairs are counted as they are inserted, so one training request can never make
/// this process hold more than this many of them; a fit that reaches the bound ends
/// the classifier `IN_ERROR` naming it.
pub const MAX_CLASSIFIER_MODEL_ENTRIES: usize = 500_000;

/// The number of (label, token) counts this store holds across EVERY classifier.
///
/// The EMULATOR's bound on the other product a caller could grow: the record set is
/// bounded ([`MAX_DOCUMENT_CLASSIFIER_RECORDS`]) and each model is bounded
/// ([`MAX_CLASSIFIER_MODEL_ENTRIES`]), but their product is what this process would
/// hold, so the TOTAL is bounded too. Enforced where a model would be taken into the
/// store, [`ComAmazonawsComprehendComprehend20171127State::admit_training_outcome`]:
/// a training whose model does not fit beside the models already held ends `IN_ERROR`
/// naming the bound, so a stored model is never silently dropped and no classifier
/// reports a model it does not have.
pub const MAX_STORED_MODEL_ENTRIES: usize = 500_000;

/// The number of documents one training run holds back to test the model with at
/// most.
///
/// The EMULATOR's bound on the measuring pass, whose cost is the product of the test
/// documents, the model's labels and the documents' tokens: at most this many
/// documents are measured, and the rest of the training data trains the model, while
/// [`MAX_CLASSIFIER_FIT_OPERATIONS`] bounds that product itself. Reported as
/// `ClassifierMetadata$NumberOfTestDocuments`, which is therefore the number of
/// documents HELD BACK; a corpus too small to hold any back reports zero and says in
/// its `TRAINED_WITH_WARNING` message that the metrics were measured on the training
/// documents instead, so held-out and resubstitution metrics are never confused.
pub const MAX_TEST_DOCUMENTS: usize = 200;

/// The share of the training documents held back to test the fitted model, as a
/// divisor: every `TRAINING_TEST_SPLIT`-th document is a test document.
///
/// Amazon Comprehend documents the split for a creation that names no `TestS3Uri`:
/// "Amazon Comprehend uses 10 percent of your training documents to test the custom
/// classifier."
pub const TRAINING_TEST_SPLIT: usize = 10;

/// The number of (label, document) token passes one training run performs at most.
///
/// The EMULATOR's bound on the WORK of a training, not on any one factor of it:
/// fitting charges one operation per token occurrence per label of a row, and
/// measuring charges the same per label of the model per test document, so the budget
/// bounds the product of rows, labels and tokens however a caller's CSV distributes
/// them. A run that exhausts it ends the classifier `IN_ERROR` naming the bound
/// rather than occupying a core for as long as its input asks.
pub const MAX_CLASSIFIER_FIT_OPERATIONS: u64 = 20_000_000;

/// How many charges the budget makes between two looks at the clock.
///
/// Reading the clock is itself work, so the deadline is checked every so many
/// charges rather than on each one; a charge covers one document's tokens for one
/// label, so the interval is far finer than the budget it guards.
const BUDGET_DEADLINE_CHECK_INTERVAL: u64 = 256;

/// What one training run may spend: a number of token operations, and the instant it
/// must stop at.
///
/// Carried THROUGH the fit and the measuring pass ([`TrainedClassifier::fit`] and
/// [`TrainedClassifier::predict`]), so the bound binds the work the process does
/// rather than only the caller's wait: the pass charges as it goes and stops itself
/// when either the operations or the deadline run out. `Err` is the reason it
/// stopped, ready for the classifier's `IN_ERROR` message — nothing is reported for
/// a fit that did not finish.
#[derive(Debug)]
pub struct TrainingBudget {
    limit: u64,
    remaining: u64,
    deadline: Instant,
    wall_clock: Duration,
    charges: u64,
}

impl TrainingBudget {
    /// A budget of `limit` token operations, expiring `wall_clock` from now.
    pub fn new(limit: u64, wall_clock: Duration) -> Self {
        Self {
            limit,
            remaining: limit,
            deadline: Instant::now() + wall_clock,
            wall_clock,
            charges: 0,
        }
    }

    /// Charge `operations` token operations against this budget.
    pub fn charge(&mut self, operations: u64) -> Result<(), String> {
        self.remaining = self.remaining.checked_sub(operations).ok_or_else(|| {
            format!(
                "the training data asks for more than the {} token operations this emulator \
                 spends on fitting and measuring one classifier",
                self.limit
            )
        })?;
        self.charges += 1;
        if self.charges.is_multiple_of(BUDGET_DEADLINE_CHECK_INTERVAL)
            && Instant::now() >= self.deadline
        {
            return Err(format!(
                "the training did not finish within the {} seconds this emulator spends on \
                 fitting and measuring one classifier",
                self.wall_clock.as_secs()
            ));
        }
        Ok(())
    }
}

/// One label's share of a fitted model: how many documents carry it, and how often
/// each token occurs in them.
#[derive(Debug, Clone, Default)]
pub struct LabelCounts {
    /// How many documents of the training set carry this label.
    pub documents: usize,
    /// How many times each token occurs across those documents.
    pub token_counts: BTreeMap<String, u64>,
    /// The total token occurrences across those documents, i.e. the sum of
    /// [`Self::token_counts`].
    pub total_tokens: u64,
}

impl LabelCounts {
    /// Count one document's `tokens` towards this label, and answer how many tokens
    /// this label had never seen before — the new (label, token) counts the model
    /// just grew by, which is what [`MAX_CLASSIFIER_MODEL_ENTRIES`] bounds.
    fn observe(&mut self, tokens: &[String]) -> usize {
        self.documents += 1;
        let mut added = 0usize;
        for token in tokens {
            match self.token_counts.get_mut(token) {
                Some(count) => *count += 1,
                None => {
                    self.token_counts.insert(token.clone(), 1);
                    added += 1;
                }
            }
            self.total_tokens += 1;
        }
        added
    }

    /// `ln P(token | this label)` under add-one (Laplace) smoothing over a
    /// vocabulary of `vocabulary` tokens, so a token this label never saw still has
    /// a finite probability.
    fn token_log_probability(&self, token: &str, vocabulary: usize) -> f64 {
        let count = self.token_counts.get(token).copied().unwrap_or(0);
        ((count + 1) as f64 / (self.total_tokens as f64 + vocabulary.max(1) as f64)).ln()
    }

    /// `ln P(token | every label but this one)`, from the corpus totals: the
    /// one-versus-rest counterpart of [`Self::token_log_probability`], which is what
    /// a `MULTI_LABEL` model decides each label on.
    fn complement_log_probability(
        &self,
        corpus: &LabelCounts,
        token: &str,
        vocabulary: usize,
    ) -> f64 {
        let count = corpus
            .token_counts
            .get(token)
            .copied()
            .unwrap_or(0)
            .saturating_sub(self.token_counts.get(token).copied().unwrap_or(0));
        let total = corpus.total_tokens.saturating_sub(self.total_tokens);
        ((count + 1) as f64 / (total as f64 + vocabulary.max(1) as f64)).ln()
    }
}

/// One custom classification model, as `CreateDocumentClassifier`'s training run
/// fitted it: a multinomial naive-Bayes bag-of-words model over the labelled
/// documents the training data held.
///
/// Persisted on the [`DocumentClassifier`] record so what a training run produced is
/// the model a later inference call uses, rather than a result invented per request.
#[derive(Debug, Clone)]
pub struct TrainedClassifier {
    /// The `DocumentClassifierMode` the model was fitted for: `MULTI_CLASS` picks the
    /// single best label, `MULTI_LABEL` decides each label on its own.
    pub mode: String,
    /// Each label's counts, keyed by the label as the training data spelled it.
    pub labels: BTreeMap<String, LabelCounts>,
    /// The counts over the whole training set, which is what the one-versus-rest
    /// complement of a label is computed from.
    pub corpus: LabelCounts,
    /// How many (label, token) counts this model holds, counted as the fit inserted
    /// them. Kept on the model so the store can bound the TOTAL it holds across
    /// records ([`MAX_STORED_MODEL_ENTRIES`]) without walking every label of every
    /// model.
    entries: usize,
}

impl TrainedClassifier {
    /// Fit a model on `documents`, each a pair of the labels the row carried and the
    /// document's text.
    ///
    /// Bounded in every direction a caller's training data could grow: the vocabulary
    /// by [`MAX_CLASSIFIER_VOCABULARY`] (once it is full, a token the corpus has not
    /// seen before takes no part in the fit), the label set by
    /// [`MAX_CLASSIFIER_LABELS`], the (label, token) counts they form — their PRODUCT
    /// — by [`MAX_CLASSIFIER_MODEL_ENTRIES`], checked as each count is inserted, and
    /// the WORK by `budget`, charged per label per document token whether the count it
    /// lands on is new or not. `Err` is the reason the model could not be fitted,
    /// ready for the classifier's `IN_ERROR` message: no model is reported for a fit
    /// that was refused or cut short.
    pub fn fit(
        mode: &str,
        documents: &[(Vec<String>, String)],
        budget: &mut TrainingBudget,
    ) -> Result<Self, String> {
        let mut model = Self {
            mode: mode.to_string(),
            labels: BTreeMap::new(),
            corpus: LabelCounts::default(),
            entries: 0,
        };
        let mut entries = 0usize;
        for (labels, text) in documents {
            let tokens: Vec<String> = tokenize(text)
                .into_iter()
                .filter(|token| {
                    model.corpus.token_counts.contains_key(token)
                        || model.corpus.token_counts.len() < MAX_CLASSIFIER_VOCABULARY
                })
                .collect();
            budget.charge(tokens.len() as u64)?;
            model.corpus.observe(&tokens);
            for label in labels {
                if !model.labels.contains_key(label) && model.labels.len() >= MAX_CLASSIFIER_LABELS
                {
                    return Err(format!(
                        "The training data names more than the {MAX_CLASSIFIER_LABELS} labels a \
                         custom classifier can carry, so no classifier was trained."
                    ));
                }
                budget.charge(tokens.len() as u64)?;
                entries += model
                    .labels
                    .entry(label.clone())
                    .or_default()
                    .observe(&tokens);
                if entries > MAX_CLASSIFIER_MODEL_ENTRIES {
                    return Err(format!(
                        "The training data holds more than the {MAX_CLASSIFIER_MODEL_ENTRIES} \
                         per-label token counts this emulator fits a model from, so no \
                         classifier was trained."
                    ));
                }
            }
        }
        model.entries = entries;
        Ok(model)
    }

    /// How many (label, token) counts this model holds — its size, as
    /// [`MAX_CLASSIFIER_MODEL_ENTRIES`] and [`MAX_STORED_MODEL_ENTRIES`] count it.
    pub fn model_entries(&self) -> usize {
        self.entries
    }

    /// How many distinct tokens the fitted model knows.
    pub fn vocabulary(&self) -> usize {
        self.corpus.token_counts.len()
    }

    /// The labels this model gives `text`, in label order.
    ///
    /// A `MULTI_CLASS` model answers the single likeliest label, as its mode's
    /// documentation says ("the document is assigned exactly one class"); a
    /// `MULTI_LABEL` model answers every label whose one-versus-rest comparison comes
    /// out positive ("an individual class represents a different category").
    ///
    /// Charged against `budget` per label per token, like the fit, so a classification
    /// pass over many documents stops itself rather than running for as long as the
    /// model's labels and the document's tokens multiply out to. `Err` is the reason
    /// it stopped: no label is answered for a comparison that was not finished.
    pub fn predict(
        &self,
        text: &str,
        budget: &mut TrainingBudget,
    ) -> Result<Vec<String>, String> {
        let tokens = tokenize(text);
        let vocabulary = self.vocabulary();
        let documents = self.corpus.documents.max(1) as f64;
        let mut given: Vec<String> = Vec::new();
        if self.mode == "MULTI_LABEL" {
            for (label, counts) in &self.labels {
                budget.charge(tokens.len() as u64)?;
                let positive_prior = (counts.documents as f64 / documents).max(f64::MIN_POSITIVE);
                let negative_prior =
                    ((documents - counts.documents as f64) / documents).max(f64::MIN_POSITIVE);
                let mut positive = positive_prior.ln();
                let mut negative = negative_prior.ln();
                for token in &tokens {
                    positive += counts.token_log_probability(token, vocabulary);
                    negative += counts.complement_log_probability(&self.corpus, token, vocabulary);
                }
                if positive > negative {
                    given.push(label.clone());
                }
            }
            return Ok(given);
        }
        // The highest score wins; ties go to the label that sorts first, so one model
        // always answers one document the same way.
        let mut best: Option<(&String, f64)> = None;
        for (label, counts) in &self.labels {
            budget.charge(tokens.len() as u64)?;
            let mut score = (counts.documents as f64 / documents)
                .max(f64::MIN_POSITIVE)
                .ln();
            for token in &tokens {
                score += counts.token_log_probability(token, vocabulary);
            }
            if best.is_none_or(|(_, highest)| score > highest) {
                best = Some((label, score));
            }
        }
        if let Some((label, _)) = best {
            given.push(label.clone());
        }
        Ok(given)
    }
}

/// Split `text` into the bag-of-words tokens a [`TrainedClassifier`] counts:
/// lowercased runs of alphanumeric characters, with everything else a separator.
///
/// Script-agnostic on purpose — a Comprehend classifier is trained per language, and
/// `char::is_alphanumeric` is Unicode-aware — so a document in any language the
/// `LanguageCode` enum names tokenises the same way.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// What a classifier's training run reported: the terminal `ModelStatus` the clock
/// reveals once the training span is over, the `Message` that says why, the metrics
/// the run measured and the model it fitted.
///
/// The classifier's counterpart of [`JobOutcome`], and held for the same reason: a
/// status is only reported for work that ran.
#[derive(Debug, Clone)]
pub struct ClassifierTrainingOutcome {
    /// One of `super::ops::types::MODEL_STATUS_VALUES`, and terminal: `TRAINED`,
    /// `TRAINED_WITH_WARNING` or `IN_ERROR`.
    pub status: String,
    pub message: Option<String>,
    /// The metrics of the fitted model, measured on the held-out test documents;
    /// `None` for a run that fitted nothing.
    pub metadata: Option<ClassifierMetadata>,
    /// The model the run fitted; `None` for a run that fitted nothing.
    pub model: Option<TrainedClassifier>,
}

impl ClassifierTrainingOutcome {
    /// The outcome of a run that read the labelled documents and fitted `model` on
    /// them, measuring `metadata`. `warning` carries what the training data made the
    /// run report alongside the model, which is what `TRAINED_WITH_WARNING` means.
    pub fn trained(
        model: TrainedClassifier,
        metadata: ClassifierMetadata,
        warning: Option<String>,
    ) -> Self {
        Self {
            status: match warning {
                Some(_) => "TRAINED_WITH_WARNING".to_string(),
                None => "TRAINED".to_string(),
            },
            message: warning,
            metadata: Some(metadata),
            model: Some(model),
        }
    }

    /// The outcome of a training that could not be carried out, naming `reason`.
    /// Nothing was fitted, so the classifier reports no metrics and holds no model.
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            status: "IN_ERROR".to_string(),
            message: Some(reason.into()),
            metadata: None,
            model: None,
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
    /// When the clock owes `SUBMITTED` -> `TRAINING`.
    pub training_due_at: SystemTime,
    /// When the clock owes `TRAINING` -> its terminal status.
    pub terminal_due_at: SystemTime,
    /// When the clock owes `STOP_REQUESTED` -> `STOPPED`; `None` until a stop is
    /// requested.
    pub stopped_due_at: Option<SystemTime>,
    /// When the clock owes this record's removal; `None` until a delete is
    /// requested.
    pub removal_due_at: Option<SystemTime>,
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
    /// The resource-based policy attached to this model, if any: what
    /// `CreateDocumentClassifier`'s `ModelPolicy` attached at creation and what
    /// `PutResourcePolicy` replaces, `DescribeResourcePolicy` reports and
    /// `DeleteResourcePolicy` removes. ONE policy per model, as the operations'
    /// `PolicyRevisionId` says: each modification replaces the prior version.
    pub resource_policy: Option<ResourcePolicy>,
    /// What this classifier's training run reported, until the clock reveals it — the
    /// classifier's counterpart of [`AnalysisJob::terminal_outcome`]. `None` while the
    /// training is still running, and `None` again once [`Self::settle`] has taken
    /// the outcome onto the record, so a fitted model is never held twice.
    pub training_outcome: Option<ClassifierTrainingOutcome>,
    /// The model this classifier's training actually fitted, revealed with its
    /// terminal status. `None` for a classifier that has not finished training, and
    /// for one whose training failed: nothing was fitted, so there is no model.
    pub model: Option<TrainedClassifier>,
    pub tags: Vec<Tag>,
    /// The `ClientRequestToken` the classifier was created under, if any. Held so
    /// the idempotency-index entry can be dropped with the record it points at.
    pub client_request_token: Option<String>,
}

impl DocumentClassifier {
    /// Materialise every status change the clock already owed at `now`.
    ///
    /// Called by the reporting operations as well as the mutating ones: the
    /// transition was due at `now` whether or not anybody called, so applying it on
    /// read reports the status the classifier already had rather than advancing it.
    ///
    /// The terminal status is never assumed: it is whatever the training run
    /// [`Self::training_outcome`] holds reported — `TRAINED` only when this emulator
    /// really read the labelled documents and fitted a model on them, `IN_ERROR`
    /// with a reason when it could not — and a classifier whose run has not reported
    /// yet stays `TRAINING`, because it is still training. No status is ever reported
    /// for work nothing performed.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == "SUBMITTED" && now >= self.training_due_at {
            self.status = "TRAINING".to_string();
            self.training_start_time = Some(self.training_due_at);
        }
        if self.status == "TRAINING" && now >= self.terminal_due_at {
            // TAKEN, not cloned: a fitted model can hold half a million per-label
            // token counts, so the record moves it over rather than keeping a second
            // copy of it in the outcome for the life of the classifier.
            if let Some(outcome) = self.training_outcome.take() {
                self.status = outcome.status;
                self.message = outcome.message;
                self.classifier_metadata = outcome.metadata;
                self.model = outcome.model;
                self.training_end_time = Some(self.terminal_due_at);
                self.end_time = Some(self.terminal_due_at);
            }
        }
        if self.status == "STOP_REQUESTED" && self.stopped_due_at.is_some_and(|due| now >= due) {
            self.status = "STOPPED".to_string();
            self.training_end_time = self.stopped_due_at;
            self.end_time = self.stopped_due_at;
        }
    }

    /// Record that a caller asked for this classifier's training to stop.
    ///
    /// `StopTrainingDocumentClassifier` documents the two outcomes: a classifier
    /// whose state is `TRAINING` "is marked for termination and put into the
    /// `STOP_REQUESTED` state", and one whose training already finished keeps the
    /// status it reached. A classifier still `SUBMITTED` has no training to wind
    /// down, so it stops outright.
    pub fn request_stop(&mut self, now: SystemTime) {
        match self.status.as_str() {
            "SUBMITTED" => {
                self.status = "STOPPED".to_string();
                self.end_time = Some(now);
            }
            "TRAINING" => {
                self.status = "STOP_REQUESTED".to_string();
                self.stopped_due_at = Some(now + CLASSIFIER_STOPPING_SPAN);
            }
            // Already stopping, stopped, deleting or terminal: nothing to wind down.
            _ => return,
        }
        self.message = Some("Training was stopped at the request of the caller.".to_string());
    }

    /// Record that a caller asked for this classifier to be deleted: it goes
    /// `DELETING` now and the record is removed once the clock owes it, which
    /// [`ComAmazonawsComprehendComprehend20171127State::settle_document_classifiers`]
    /// materialises.
    pub fn request_deletion(&mut self, now: SystemTime) {
        self.status = "DELETING".to_string();
        self.removal_due_at = Some(now + CLASSIFIER_DELETING_SPAN);
    }

    /// Whether this classifier's training has finished, for good or ill \u2014 the
    /// states `DeleteDocumentClassifier` calls terminated.
    ///
    /// The operation documents "Only those classifiers that are in terminated states
    /// (IN_ERROR, TRAINED) will be deleted"; `STOPPED` and `TRAINED_WITH_WARNING`
    /// belong to the same set, because training has ended there too and no status
    /// change can ever leave them.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "TRAINED" | "TRAINED_WITH_WARNING" | "IN_ERROR" | "STOPPED"
        )
    }

    /// Record what this classifier's training run reported, as of the instant `now`
    /// it reported it.
    ///
    /// The classifier's counterpart of [`AnalysisJob::record_run_outcome`], and owed
    /// for the same reason: a training run that took longer than
    /// [`CLASSIFIER_SUBMITTED_SPAN`] was submitted-or-training the whole time it ran,
    /// so the instants the remaining transitions are owed at are carried forward from
    /// the report rather than left in the past. That is what keeps `TRAINING`
    /// OBSERVABLE to a caller polling after the creation returns.
    ///
    /// A classifier a caller has already stopped keeps the stop: the run's outcome is
    /// recorded, but no status change is owed to a classifier that has left the
    /// training path.
    pub fn record_training_outcome(&mut self, outcome: ClassifierTrainingOutcome, now: SystemTime) {
        self.training_outcome = Some(outcome);
        if now >= self.training_due_at {
            self.training_due_at = now + CLASSIFIER_SUBMITTED_SPAN;
        }
        let earliest_terminal = self.training_due_at + CLASSIFIER_TRAINING_SPAN;
        if self.terminal_due_at < earliest_terminal {
            self.terminal_due_at = earliest_terminal;
        }
    }

    /// Whether a model trained from this classifier can serve inference: the two
    /// `ModelStatus` values that mean a model was fitted.
    ///
    /// `DescribeDocumentClassifier` documents `TRAINED` as "the classifier is trained
    /// and ready to use" and `TRAINED_WITH_WARNING` as trained on data the service
    /// had reservations about; every other status names a model that has not been
    /// fitted, so there is nothing to deploy behind an endpoint.
    pub fn is_trained(&self) -> bool {
        matches!(self.status.as_str(), "TRAINED" | "TRAINED_WITH_WARNING")
    }

    /// A classifier record as it stands the instant it was submitted: the identity
    /// and the configuration it was created with, nothing trained yet, and the two
    /// instants the clock owes its next status changes at.
    ///
    /// [`DEFAULT_CLASSIFIER_MODE`] is the mode `CreateDocumentClassifier` documents
    /// as its default; a caller that named a mode overwrites the field.
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
            training_due_at: submit_time + CLASSIFIER_SUBMITTED_SPAN,
            terminal_due_at: submit_time + CLASSIFIER_SUBMITTED_SPAN + CLASSIFIER_TRAINING_SPAN,
            stopped_due_at: None,
            removal_due_at: None,
            input_data_config: None,
            output_data_config: None,
            classifier_metadata: None,
            data_access_role_arn: None,
            volume_kms_key_id: None,
            model_kms_key_id: None,
            vpc_config: None,
            mode: DEFAULT_CLASSIFIER_MODE.to_string(),
            source_model_arn: None,
            flywheel_arn: None,
            resource_policy: None,
            training_outcome: None,
            model: None,
            tags: Vec::new(),
            client_request_token: None,
        }
    }

    /// Project this record into the modelled `DocumentClassifierProperties` shape.
    ///
    /// `ClassifierMetadata` is whatever the record carries: the metrics of a fitted
    /// model are reported only for a classifier that has them, never invented for one
    /// that trained nothing.
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

/// How long a freshly created endpoint stays `CREATING` before the clock owes it the
/// status [`Endpoint::provisioning`] holds.
///
/// The emulator's own span, not a documented AWS duration: provisioning real
/// inference capacity takes minutes, which no test can wait for. It is deliberately
/// non-zero, so a caller that describes an endpoint straight back still observes
/// `CREATING` — the state `EndpointProperties$Status` documents callers have to wait
/// out before making inference requests.
pub const ENDPOINT_CREATING_SPAN: Duration = Duration::from_millis(300);

/// The most endpoint records this emulator retains.
///
/// The EMULATOR's bound, for the same reason as [`MAX_ANALYSIS_JOB_RECORDS`]: the
/// oldest record is evicted once the store passes it, so no sequence of requests can
/// grow this store without limit. AWS's own per-account endpoint quota is adjustable
/// and is not emulated as a refusal here, so creating an endpoint is never refused
/// for this bound; only the record set is trimmed. The bound also caps the page
/// `ListEndpoints` can ever walk.
pub const MAX_ENDPOINT_RECORDS: usize = 1000;

/// The outcome of the provisioning a `CreateEndpoint` call performed: the status the
/// clock reveals once the endpoint's creating span is over, and the `Message` that
/// says why.
///
/// Held separately from [`Endpoint::status`] for the same reason [`JobOutcome`] is
/// held separately from a job's status: a status is only reported for work that ran,
/// so until the provisioning reports, the endpoint is still `CREATING`.
#[derive(Debug, Clone)]
pub struct EndpointProvisioning {
    /// One of `super::ops::types::ENDPOINT_STATUS_VALUES`, and the status the
    /// creation settles into: `IN_SERVICE` or `FAILED`.
    pub status: String,
    /// What the provisioning has to say — the failure's reason, or nothing for an
    /// endpoint that came into service.
    pub message: Option<String>,
}

impl EndpointProvisioning {
    /// The outcome of provisioning that put a trained model behind the endpoint, so
    /// the endpoint can serve inference from it.
    pub fn in_service() -> Self {
        Self {
            status: "IN_SERVICE".to_string(),
            message: None,
        }
    }

    /// The outcome of provisioning that could not be carried out, naming `reason`.
    /// `FAILED` is the modelled status whose `Message` member documents exactly this:
    /// "Specifies a reason for failure in cases of `Failed` status."
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            status: "FAILED".to_string(),
            message: Some(reason.into()),
        }
    }
}

/// One persisted model-specific inference endpoint.
///
/// Every member is a value `CreateEndpoint` was called with, or a value this service
/// minted for it — never anything a later `Describe`/`List` request supplied. The
/// record is keyed by its `EndpointArn`, which is what every other endpoint operation
/// names it by, and [`Self::endpoint_properties`] is the ONE place it is projected
/// into the modelled shape the reporting operations return.
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// Creation order within the service, the stable key `ListEndpoints` pages on.
    pub seq: u64,
    /// The endpoint's ARN, as [`endpoint_arn`] renders it.
    pub endpoint_arn: String,
    /// `EndpointName`, the descriptive suffix of the ARN. Not a member of
    /// `EndpointProperties`: the ARN carries it.
    pub endpoint_name: String,
    /// One of `super::ops::types::ENDPOINT_STATUS_VALUES`.
    pub status: String,
    pub message: Option<String>,
    /// The ARN of the model this endpoint was created for.
    pub model_arn: Option<String>,
    pub desired_inference_units: i32,
    /// The inference units actually serving requests: `None` until the endpoint is
    /// in service, because nothing is serving before that — the modelled
    /// `InferenceUnitsInteger` has `@range min 1`, so an idle endpoint has no
    /// count to report rather than a zero.
    pub current_inference_units: Option<i32>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// When the clock owes `CREATING` -> the status [`Self::provisioning`] holds.
    pub ready_due_at: SystemTime,
    /// The outcome of the provisioning the create performed, revealed by
    /// [`Self::settle`] once the clock owes the transition.
    pub provisioning: EndpointProvisioning,
    pub data_access_role_arn: Option<String>,
    pub flywheel_arn: Option<String>,
    pub tags: Vec<Tag>,
    /// The `ClientRequestToken` the endpoint was created under, if any. Held so the
    /// idempotency-index entry can be dropped with the record it points at.
    pub client_request_token: Option<String>,
}

impl Endpoint {
    /// An endpoint record as it stands the instant it was created: `CREATING`, with
    /// nothing serving inference yet, the instant the clock owes it its settled
    /// status, and the outcome of the provisioning the create performed.
    pub fn creating(
        seq: u64,
        endpoint_arn: String,
        endpoint_name: String,
        desired_inference_units: i32,
        created_at: SystemTime,
        provisioning: EndpointProvisioning,
    ) -> Self {
        Self {
            seq,
            endpoint_arn,
            endpoint_name,
            status: "CREATING".to_string(),
            message: None,
            model_arn: None,
            desired_inference_units,
            current_inference_units: None,
            creation_time: created_at,
            last_modified_time: created_at,
            ready_due_at: created_at + ENDPOINT_CREATING_SPAN,
            provisioning,
            data_access_role_arn: None,
            flywheel_arn: None,
            tags: Vec::new(),
            client_request_token: None,
        }
    }

    /// Materialise the status change the clock already owed at `now`.
    ///
    /// Called by the reporting operations as well as the mutating ones: the
    /// transition was due at `now` whether or not anybody called, so applying it on
    /// read reports the status the endpoint already had rather than advancing it.
    ///
    /// What the transition reveals is [`Self::provisioning`] — the outcome of the
    /// provisioning the create actually performed — never an assumed `IN_SERVICE`:
    /// an endpoint is in service only when there is a trained model behind it to
    /// serve inference from.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == "CREATING" && now >= self.ready_due_at {
            self.status = self.provisioning.status.clone();
            self.message = self.provisioning.message.clone();
            self.last_modified_time = self.ready_due_at;
            if self.status == "IN_SERVICE" {
                self.current_inference_units = Some(self.desired_inference_units);
            }
        }
    }

    /// Project this record into the modelled `EndpointProperties` shape.
    ///
    /// `DesiredModelArn` and `DesiredDataAccessRoleArn` are the members an
    /// `UpdateEndpoint` in flight reports — "ARN of the new model to use for updating
    /// an existing endpoint" — so an endpoint with no update in flight reports
    /// neither.
    pub fn endpoint_properties(&self) -> EndpointProperties {
        EndpointProperties {
            endpoint_arn: Some(self.endpoint_arn.clone()),
            status: Some(self.status.clone()),
            message: self.message.clone(),
            model_arn: self.model_arn.clone(),
            desired_model_arn: None,
            desired_inference_units: Some(self.desired_inference_units),
            current_inference_units: self.current_inference_units,
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.last_modified_time)),
            data_access_role_arn: self.data_access_role_arn.clone(),
            desired_data_access_role_arn: None,
            flywheel_arn: self.flywheel_arn.clone(),
        }
    }
}

/// The ARN an endpoint carries, stated once here.
///
/// The form the modelled `ComprehendEndpointArn` pattern requires:
/// `arn:aws:comprehend:<region>:<account>:<resource-type>/<name>`, where
/// `resource_type` is what [`endpoint_resource_type`] derives from the attached
/// model's own ARN.
pub fn endpoint_arn(region: &str, resource_type: &str, name: &str) -> String {
    ::emulator_server::emulator_arn("comprehend", region, &format!("{resource_type}/{name}"))
}

/// The endpoint resource type an endpoint attached to the model `model_arn` carries,
/// or `None` when that ARN names no Comprehend custom model.
///
/// The modelled `ComprehendEndpointArn` pattern admits exactly two resource types,
/// `document-classifier-endpoint` and `entity-recognizer-endpoint`, and
/// `ComprehendModelArn` admits the two matching model types: an endpoint's own
/// resource type is the attached model's, suffixed `-endpoint`.
pub fn endpoint_resource_type(model_arn: &str) -> Option<&'static str> {
    let resource = model_arn.split(':').nth(5)?;
    match resource.split('/').next()? {
        "document-classifier" => Some("document-classifier-endpoint"),
        "entity-recognizer" => Some("entity-recognizer-endpoint"),
        _ => None,
    }
}

/// How many job records a `List*DetectionJobs` page carries when the request sets
/// no `MaxResults`.
///
/// From the modelled `MaxResults` documentation every analysis-job `List*` request
/// shares: "The maximum number of results to return in each page. The default is
/// 100."
pub const DEFAULT_JOB_PAGE_SIZE: usize = 100;

/// The ceiling a `List*DetectionJobs` page is clamped to: the modelled `MaxResults`
/// maximum (`@range` 1..500), so one request can never render an unbounded number of
/// records into one response even if a value reaches a handler outside that range.
pub const MAX_JOB_PAGE_SIZE: usize = 500;

/// One `List*DetectionJobs` request's filter, lifted out of the family's own
/// modelled filter shape.
///
/// Comprehend models a separate filter structure per job family, and every one of
/// them declares the same four members with the same documented meaning — `JobName`,
/// `JobStatus`, `SubmitTimeBefore`, `SubmitTimeAfter` — so the selection and the
/// ordering rules live here ONCE rather than once per family's handler, exactly as
/// `job_properties_projections!` holds the record -> properties projection once. A
/// family's handler borrows its own filter shape's members into this struct and
/// nothing else.
#[derive(Debug, Default, Clone, Copy)]
pub struct JobFilterParts<'a> {
    pub job_name: Option<&'a str>,
    pub job_status: Option<&'a str>,
    pub submit_time_before: Option<&'a ::aws_smithy_types::DateTime>,
    pub submit_time_after: Option<&'a ::aws_smithy_types::DateTime>,
}

impl JobFilterParts<'_> {
    /// How many of this filter's members are set.
    ///
    /// Every `List*DetectionJobs` operation documents "You can only set one filter
    /// at a time", so a handler refuses more than one with the
    /// `InvalidFilterException` its model declares.
    pub fn count_set(&self) -> usize {
        usize::from(self.job_name.is_some())
            + usize::from(self.job_status.is_some())
            + usize::from(self.submit_time_before.is_some())
            + usize::from(self.submit_time_after.is_some())
    }

    /// Does this filter ask for newest-to-oldest?
    ///
    /// The model states the order per filter member, and the two submit-time members
    /// state OPPOSITE orders: `SubmitTimeBefore` is "Jobs are returned in ascending
    /// order, oldest to newest" and `SubmitTimeAfter` is "Jobs are returned in
    /// descending order, newest to oldest". Only one filter may be set at a time, so
    /// the order each page is served in is unambiguous: `SubmitTimeAfter` reverses
    /// it, every other request keeps the ascending default.
    pub fn newest_first(&self) -> bool {
        self.submit_time_after.is_some()
    }

    /// Does `job` satisfy this filter?
    ///
    /// Each set member narrows the list the way its own documentation states:
    /// `JobName` and `JobStatus` match exactly, and the submit-time bounds are read
    /// strictly — "submitted before"/"submitted after" exclude a job submitted at
    /// the bound itself, which the model leaves unstated.
    pub fn matches(&self, job: &AnalysisJob) -> bool {
        if let Some(name) = self.job_name {
            if job.job_name.as_deref() != Some(name) {
                return false;
            }
        }
        if let Some(status) = self.job_status {
            if job.job_status != status {
                return false;
            }
        }
        let submitted = epoch_parts(job.submit_time);
        if let Some(before) = self.submit_time_before {
            if submitted >= datetime_parts(before) {
                return false;
            }
        }
        if let Some(after) = self.submit_time_after {
            if submitted <= datetime_parts(after) {
                return false;
            }
        }
        true
    }
}

/// Render the opaque page cursor that resumes a `List*` page after the record with
/// creation (or submission) sequence number `seq`.
///
/// The ONE cursor codec this service's paging operations share, so no two of them
/// can disagree about the form of a `NextToken`. What the number MEANS stays the
/// issuing operation's own — a token is opaque to everything but the operation that
/// minted it, and each caller checks the sequence against the records it pages — but
/// the rendering is stated once: the sequence number is not a member of any modelled
/// shape, so it is carried as hex rather than exposed.
pub fn encode_page_cursor(seq: u64) -> String {
    hex::encode(seq.to_be_bytes())
}

/// Read back a cursor produced by [`encode_page_cursor`]; `None` when the string is
/// not of that form. Each caller additionally checks that the sequence number it
/// carries is one this service has minted, so a well-formed string naming no page it
/// ever issued is refused too.
pub fn decode_page_cursor(token: &str) -> Option<u64> {
    let bytes = hex::decode(token).ok()?;
    let bytes: [u8; 8] = bytes.try_into().ok()?;
    Some(u64::from_be_bytes(bytes))
}

/// Render the page cursor that resumes after the job with submission sequence `seq`,
/// in whichever direction the page was served. Opaque to the caller: the sequence
/// number is not a member of any modelled shape.
pub fn encode_job_page_token(seq: u64) -> String {
    encode_page_cursor(seq)
}

/// Read back a cursor produced by [`encode_job_page_token`]; `None` when the string
/// is not of that form.
///
/// [`ComAmazonawsComprehendComprehend20171127State::job_page_cursor`] also checks
/// that the sequence number it carries is one this service has minted, so a
/// well-formed string naming no page it ever issued is refused too.
pub fn decode_job_page_token(token: &str) -> Option<u64> {
    decode_page_cursor(token)
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
    /// The first classifier ARN minted for a `ClientRequestToken`, so a retried
    /// `CreateDocumentClassifier` resolves to the classifier it already created. An
    /// entry is dropped with the classifier it points at.
    pub classifier_idempotency_tokens: HashMap<String, String>,
    /// Monotonic creation counter, the source of a classifier's `seq` — the stable
    /// order `ListDocumentClassifiers` pages on.
    pub document_classifier_seq: u64,
    /// Monotonic policy-modification counter: the source of the uniqueness of a
    /// minted `PolicyRevisionId`.
    pub policy_revision_seq: u64,
    /// Every persisted inference endpoint, keyed by its `EndpointArn`. Bounded by
    /// [`MAX_ENDPOINT_RECORDS`]: insert through [`Self::insert_endpoint`], never
    /// directly.
    pub endpoints: BTreeMap<String, Endpoint>,
    /// The first endpoint ARN minted for a `ClientRequestToken`, so a retried
    /// `CreateEndpoint` resolves to the endpoint it already created rather than
    /// colliding with it — which is what `ClientRequestToken`'s own documentation
    /// states: "If this token matches a previous endpoint creation request, Amazon
    /// Comprehend will not return a `ResourceInUseException`." An entry is dropped
    /// with the endpoint it points at.
    pub endpoint_idempotency_tokens: HashMap<String, String>,
    /// Monotonic creation counter, the source of an endpoint's `seq` — the stable
    /// order `ListEndpoints` pages on.
    pub endpoint_seq: u64,
}

impl ComAmazonawsComprehendComprehend20171127State {
    /// The tag set of the resource `resource_arn` names, for every kind of resource
    /// this service persists: a custom document classifier, keyed by the ARN its
    /// creation minted, and an asynchronous analysis job, found by the `JobArn` its
    /// submission minted.
    ///
    /// The ONE ARN lookup every tagging operation uses, so `TagResource`,
    /// `UntagResource` and `ListTagsForResource` cannot disagree about which ARNs
    /// name a resource, and so a tag associated through one of them is the tag the
    /// others see. The job scan is bounded by [`MAX_ANALYSIS_JOB_RECORDS`].
    ///
    /// A resource kind whose creating operation is still a scaffold stub (an
    /// endpoint, a flywheel, a dataset, an entity recognizer) has no records here,
    /// so its ARN resolves to nothing — which is what a store holding no such
    /// resource means, and what the modelled `ResourceNotFoundException` reports.
    /// Callers settle the clock-owed changes first, so an ARN whose record a due
    /// deletion has already removed is not found either.
    pub fn resource_tags_mut(&mut self, resource_arn: &str) -> Option<&mut Vec<Tag>> {
        // `contains_key` and then `get_mut` deliberately: returning the borrow from
        // inside an `if let Some(..) = self.document_classifiers.get_mut(..)` keeps
        // that mutable borrow of `self` alive across the job lookup below, which the
        // borrow checker refuses. Two map lookups are the price of one early return.
        if self.document_classifiers.contains_key(resource_arn) {
            return self
                .document_classifiers
                .get_mut(resource_arn)
                .map(|classifier| &mut classifier.tags);
        }
        self.analysis_jobs
            .values_mut()
            .find(|job| job.job_arn == resource_arn)
            .map(|job| &mut job.tags)
    }

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

    /// How many (label, token) counts this store holds across every classifier: the
    /// models already on their records, and the one a training has reported but the
    /// clock has not revealed yet.
    ///
    /// Bounded by [`MAX_STORED_MODEL_ENTRIES`] through [`Self::admit_training_outcome`],
    /// and cheap to compute: each model carries its own count, and the record set is
    /// bounded by [`MAX_DOCUMENT_CLASSIFIER_RECORDS`].
    pub fn stored_model_entries(&self) -> usize {
        self.document_classifiers
            .values()
            .map(|classifier| {
                let held = classifier
                    .model
                    .as_ref()
                    .map_or(0, TrainedClassifier::model_entries);
                let pending = classifier
                    .training_outcome
                    .as_ref()
                    .and_then(|outcome| outcome.model.as_ref())
                    .map_or(0, TrainedClassifier::model_entries);
                held + pending
            })
            .sum()
    }

    /// `outcome` as this store can take it: itself when its model fits beside the
    /// models already held, and the modelled failure naming
    /// [`MAX_STORED_MODEL_ENTRIES`] when it does not.
    ///
    /// The ONE place a fitted model is admitted into the store, so the total this
    /// process holds is bounded however many classifiers a caller trains. A refused
    /// model is NOT silently dropped: the classifier ends `IN_ERROR` with the reason,
    /// because a classifier reporting `TRAINED` has to have the model it was asked
    /// for.
    pub fn admit_training_outcome(
        &self,
        outcome: ClassifierTrainingOutcome,
    ) -> ClassifierTrainingOutcome {
        let Some(entries) = outcome
            .model
            .as_ref()
            .map(TrainedClassifier::model_entries)
        else {
            return outcome;
        };
        if self.stored_model_entries() + entries > MAX_STORED_MODEL_ENTRIES {
            return ClassifierTrainingOutcome::failed(format!(
                "The fitted model does not fit beside the models this emulator already \
                 holds: it keeps at most {MAX_STORED_MODEL_ENTRIES} per-label token counts \
                 across every classifier. Delete a classifier and try your request again."
            ));
        }
        outcome
    }

    /// The custom model `resource_arn` names, for the resource-policy operations to
    /// read and write the policy on.
    /// A `ComprehendModelArn` names a document classifier or an entity recognizer
    /// (the modelled pattern admits exactly those two resource types). This emulator
    /// persists classifiers; `CreateEntityRecognizer` is still a scaffold stub, so no
    /// entity-recognizer ARN names a resource that exists here, and the resource-policy
    /// operations answer it with their `ResourceNotFoundException` exactly as they
    /// answer a classifier ARN nobody created.
    pub fn custom_model_mut(&mut self, resource_arn: &str) -> Option<&mut DocumentClassifier> {
        self.document_classifiers.get_mut(resource_arn)
    }

    /// [`Self::custom_model_mut`] for a REPORTING operation: the same resolution, with
    /// no way to write through it, so a read path cannot modify the model it reports.
    pub fn custom_model(&self, resource_arn: &str) -> Option<&DocumentClassifier> {
        self.document_classifiers.get(resource_arn)
    }

    /// Mint the `PolicyRevisionId` a resource-policy modification assigns.
    ///
    /// The revision id is the first 16 bytes of a digest over the modification
    /// counter, the resource the policy is attached to, the policy document and the
    /// instant, rendered as 32 uppercase hex characters — the form and the modelled
    /// `^[0-9A-Fa-f]+$` pattern an AWS revision id has. It is unique per
    /// modification because the counter is, which is what `PutResourcePolicy`
    /// documents ("Each time you modify a policy, Amazon Comprehend assigns a new
    /// revision ID"): re-attaching the SAME document still mints a new revision.
    pub fn mint_policy_revision_id(
        &mut self,
        resource_arn: &str,
        policy: &str,
        at: SystemTime,
    ) -> String {
        self.policy_revision_seq += 1;
        let nanos = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let mut hasher = Sha256::new();
        hasher.update(self.policy_revision_seq.to_be_bytes());
        hasher.update(resource_arn.as_bytes());
        hasher.update(policy.as_bytes());
        hasher.update(nanos.to_be_bytes());
        let digest = hasher.finalize();
        hex::encode_upper(&digest[..16])
    }

    /// Materialise every document-classifier change the clock already owed at `now`:
    /// each record's own status transitions, and the removal of a record whose
    /// `DELETING` span has elapsed.
    ///
    /// The ONE place a classifier's clock-owed change is applied to the STORE, so
    /// every reporting and mutating operation calls it first and none of them has to
    /// know about the other's transitions. The sweep is bounded by
    /// [`MAX_DOCUMENT_CLASSIFIER_RECORDS`].
    pub fn settle_document_classifiers(&mut self, now: SystemTime) {
        for classifier in self.document_classifiers.values_mut() {
            classifier.settle(now);
        }
        let removed: Vec<String> = self
            .document_classifiers
            .iter()
            .filter(|(_, classifier)| classifier.removal_due_at.is_some_and(|due| now >= due))
            .map(|(arn, _)| arn.clone())
            .collect();
        for arn in removed {
            let dropped = self.document_classifiers.remove(&arn);
            self.forget_classifier_token(&arn, dropped);
        }
    }

    /// Drop the idempotency-index entry of a classifier record that has just left
    /// the store — but only while it still points at that record, because a later
    /// creation may have taken the same token over.
    ///
    /// The ONE place that invariant is written, for every way a record leaves:
    /// a completed deletion, an eviction at the bound, and a creation that
    /// displaces an earlier record under the same ARN.
    fn forget_classifier_token(&mut self, arn: &str, record: Option<DocumentClassifier>) {
        if let Some(token) = record.and_then(|classifier| classifier.client_request_token) {
            if self
                .classifier_idempotency_tokens
                .get(&token)
                .is_some_and(|indexed| indexed == arn)
            {
                self.classifier_idempotency_tokens.remove(&token);
            }
        }
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

    /// Materialise the clock-owed status change of every job of `family`.
    ///
    /// The sweep a reporting operation over a whole family performs before it
    /// reports: the transitions were due at `now` whether or not anybody called, so
    /// applying them on read reports the status each job already had rather than
    /// advancing it. Bounded by [`MAX_ANALYSIS_JOB_RECORDS`], like the store itself.
    pub fn settle_analysis_jobs(&mut self, family: JobFamily, now: SystemTime) {
        for ((job_family, _), job) in self.analysis_jobs.iter_mut() {
            if *job_family == family {
                job.settle(now);
            }
        }
    }

    /// Where a `List*DetectionJobs` page starts: the submission sequence number the
    /// page is served AFTER.
    ///
    /// `None` means `token` is not a cursor this service issued, which the caller
    /// refuses with the `InvalidRequestException` its model declares: a cursor is a
    /// submission sequence number this service has actually minted, so a well-formed
    /// string that names no page it ever issued — zero, or a sequence past the newest
    /// submission — is refused rather than silently read as "from the beginning".
    /// A request carrying NO token starts outside the whole range, at whichever end
    /// the page's order (`newest_first`) begins from.
    pub fn job_page_cursor(&self, token: Option<&str>, newest_first: bool) -> Option<u64> {
        match token {
            None => Some(if newest_first { u64::MAX } else { 0 }),
            Some(token) => {
                decode_job_page_token(token).filter(|seq| (1..=self.job_id_seq).contains(seq))
            }
        }
    }

    /// One page of `family`'s jobs: the records `filter` selects that lie past
    /// `cursor`, in the order the filter's own documentation states, truncated to
    /// `page_size`, together with the cursor that resumes after the last of them
    /// (`None` when this page is the last).
    ///
    /// The ONE place a family's `List*DetectionJobs` selection, ordering and paging
    /// happen, so no two families can disagree about them. Call
    /// [`Self::settle_analysis_jobs`] first: the status this reports — and the one
    /// `Filter.JobStatus` selects on — is then the status each job already had.
    pub fn analysis_job_page(
        &self,
        family: JobFamily,
        filter: &JobFilterParts<'_>,
        cursor: u64,
        page_size: usize,
    ) -> (Vec<&AnalysisJob>, Option<String>) {
        let descending = filter.newest_first();
        // `sort_by_key` is load-bearing: the store is keyed by `JobId`, whose order
        // is not submission order, and a job's `seq` is its submission order.
        let mut selected: Vec<&AnalysisJob> = self
            .analysis_jobs
            .iter()
            .filter(|((job_family, _), _)| *job_family == family)
            .map(|(_, job)| job)
            .filter(|job| {
                let past_cursor = if descending {
                    job.seq < cursor
                } else {
                    job.seq > cursor
                };
                past_cursor && filter.matches(job)
            })
            .collect();
        selected.sort_by_key(|job| job.seq);
        if descending {
            selected.reverse();
        }

        let page: Vec<&AnalysisJob> = selected.iter().copied().take(page_size).collect();
        let next_token = match page.last() {
            Some(last) if selected.len() > page.len() => Some(encode_job_page_token(last.seq)),
            _ => None,
        };
        (page, next_token)
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

    /// Persist a classifier record, index it under its `ClientRequestToken`, and
    /// evict the oldest once the store passes [`MAX_DOCUMENT_CLASSIFIER_RECORDS`].
    ///
    /// The ONE insertion path for [`Self::document_classifiers`], so the bound
    /// cannot be bypassed by a handler.
    ///
    /// A record is keyed by its ARN, so a creation can DISPLACE an earlier record
    /// with the same name and version. That is the third way a record leaves the
    /// store, beside a completed deletion and an eviction, and it reclaims the
    /// displaced record's index entry the same way: without it, a caller repeating
    /// one creation under fresh tokens would leave one unreachable entry per request
    /// behind and the index could outgrow the records it points at.
    pub fn insert_document_classifier(&mut self, classifier: DocumentClassifier) {
        let arn = classifier.classifier_arn.clone();
        let token = classifier.client_request_token.clone();
        let displaced = self.document_classifiers.insert(arn.clone(), classifier);
        self.forget_classifier_token(&arn, displaced);
        if let Some(token) = token {
            self.classifier_idempotency_tokens.insert(token, arn);
        }
        while self.document_classifiers.len() > MAX_DOCUMENT_CLASSIFIER_RECORDS {
            let oldest = self
                .document_classifiers
                .iter()
                .min_by(|(left_arn, left), (right_arn, right)| {
                    (left.seq, left_arn.as_str()).cmp(&(right.seq, right_arn.as_str()))
                })
                .map(|(arn, _)| arn.clone());
            let Some(oldest) = oldest else { break };
            let evicted = self.document_classifiers.remove(&oldest);
            self.forget_classifier_token(&oldest, evicted);
        }
    }

    /// Materialise every endpoint status change the clock already owed at `now`.
    ///
    /// The ONE place an endpoint's clock-owed change is applied to the STORE, so
    /// every reporting and mutating operation calls it first. The sweep is bounded by
    /// [`MAX_ENDPOINT_RECORDS`].
    pub fn settle_endpoints(&mut self, now: SystemTime) {
        for endpoint in self.endpoints.values_mut() {
            endpoint.settle(now);
        }
    }

    /// Mint the next endpoint creation sequence number.
    ///
    /// An endpoint's identity is its ARN (the name the caller chose), so nothing is
    /// hashed here: the counter exists only to give the records a stable creation
    /// order for paging.
    pub fn mint_endpoint_seq(&mut self) -> u64 {
        self.endpoint_seq += 1;
        self.endpoint_seq
    }

    /// Drop the idempotency-index entry of an endpoint record that has just left the
    /// store — but only while it still points at that record, because a later
    /// creation may have taken the same token over.
    ///
    /// The ONE place that invariant is written, for every way an endpoint record
    /// leaves: an eviction at the bound, and a creation that displaces an earlier
    /// record under the same ARN.
    fn forget_endpoint_token(&mut self, arn: &str, record: Option<Endpoint>) {
        if let Some(token) = record.and_then(|endpoint| endpoint.client_request_token) {
            if self
                .endpoint_idempotency_tokens
                .get(&token)
                .is_some_and(|indexed| indexed == arn)
            {
                self.endpoint_idempotency_tokens.remove(&token);
            }
        }
    }

    /// Persist an endpoint record, index it under its `ClientRequestToken`, and evict
    /// the oldest once the store passes [`MAX_ENDPOINT_RECORDS`].
    ///
    /// The ONE insertion path for [`Self::endpoints`], so the bound cannot be
    /// bypassed by a handler. It reclaims a displaced or evicted record's index entry
    /// exactly as [`Self::insert_document_classifier`] does, so the index cannot
    /// outgrow the records it points at.
    pub fn insert_endpoint(&mut self, endpoint: Endpoint) {
        let arn = endpoint.endpoint_arn.clone();
        let token = endpoint.client_request_token.clone();
        let displaced = self.endpoints.insert(arn.clone(), endpoint);
        self.forget_endpoint_token(&arn, displaced);
        if let Some(token) = token {
            self.endpoint_idempotency_tokens.insert(token, arn);
        }
        while self.endpoints.len() > MAX_ENDPOINT_RECORDS {
            let oldest = self
                .endpoints
                .iter()
                .min_by(|(left_arn, left), (right_arn, right)| {
                    (left.seq, left_arn.as_str()).cmp(&(right.seq, right_arn.as_str()))
                })
                .map(|(arn, _)| arn.clone());
            let Some(oldest) = oldest else { break };
            let evicted = self.endpoints.remove(&oldest);
            self.forget_endpoint_token(&oldest, evicted);
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

/// The per-document quota a real-time (synchronous) inference operation accepts, in
/// UTF-8 bytes, for every one of them EXCEPT those whose own member documentation
/// states a larger maximum: `DetectDominantLanguage`
/// ([`DOMINANT_LANGUAGE_TEXT_QUOTA_BYTES`]) and PII analysis
/// ([`PII_TEXT_QUOTA_BYTES`]).
///
/// Comprehend's model states no byte maximum — `CustomerInputString` carries only
/// `@length min 1`, and the member prose rounds the quota to "5 KB" — so the
/// enforced value is the published real-time quota, which is a count of BYTES and
/// not a power-of-two multiple of 1024: a 5001-byte document is refused with
/// `TextSizeLimitExceededException`, a 5000-byte one is accepted.
pub const REALTIME_TEXT_QUOTA_BYTES: usize = 5_000;

/// The per-document quota of `DetectDominantLanguage`, in UTF-8 bytes.
///
/// Language identification reads a whole document rather than a sentence, so this
/// operation alone carries the larger published quota (the member prose rounds it
/// to "100 KB"): a 100001-byte document is refused, a 100000-byte one is accepted.
pub const DOMINANT_LANGUAGE_TEXT_QUOTA_BYTES: usize = 100_000;

/// The per-document quota of Comprehend's real-time PII analysis, in UTF-8 bytes.
///
/// PII analysis is the other real-time family whose own modelled member
/// documentation states a maximum — `ContainsPiiEntitiesRequest$Text`: "A UTF-8 text
/// string. The maximum string size is 100 KB." — so it carries the larger quota
/// rather than [`REALTIME_TEXT_QUOTA_BYTES`]. That prose is read here exactly as
/// [`DOMINANT_LANGUAGE_TEXT_QUOTA_BYTES`] reads the same rounding, as a count of
/// BYTES: a 100001-byte document is refused with `TextSizeLimitExceededException`,
/// a 100000-byte one is accepted.
pub const PII_TEXT_QUOTA_BYTES: usize = 100_000;

/// The per-string quota `DetectToxicContent` accepts, in UTF-8 bytes.
///
/// Toxicity analysis is the real-time family that takes a LIST of strings rather
/// than one document, and its own modelled member documentation states the per-string
/// maximum — `DetectToxicContentRequest$TextSegments`: "A list of up to 10 text
/// strings. Each string has a maximum size of 1 KB, and the maximum size of the list
/// is 10 KB." That rounding is read here exactly as
/// [`DOMINANT_LANGUAGE_TEXT_QUOTA_BYTES`] and [`PII_TEXT_QUOTA_BYTES`] read theirs, as
/// a decimal count of BYTES: a 1001-byte string is refused with
/// `TextSizeLimitExceededException`, a 1000-byte one is accepted. The same reading is
/// what makes the two documented ceilings agree — ten strings of this quota each are
/// exactly the documented 10 KB the whole list may carry — so no separate check of
/// the list's total size is needed.
pub const TOXIC_CONTENT_SEGMENT_QUOTA_BYTES: usize = 1_000;

/// The `LanguageCode` values Comprehend's PII analysis accepts.
///
/// ONE set for the operations that DOCUMENT a set — `ContainsPiiEntities` and the
/// asynchronous detection job — so they can never drift apart on which languages PII
/// analysis accepts. The set is the one the member documentation that names a set
/// states, `StartPiiEntitiesDetectionJobRequest$LanguageCode` and
/// `DetectPiiEntitiesRequest$LanguageCode`: "Enter the language code for English
/// (en) or Spanish (es)." `ContainsPiiEntitiesRequest$LanguageCode` is "The language
/// of the input documents" — it names no set, so it neither widens nor narrows this
/// one. Every other value the `LanguageCode` enum names is refused with the error the
/// calling operation models for it, rather than answered with an empty result that no
/// analysis produced.
///
/// `DetectPiiEntities` is the one member of the family that does NOT check this set:
/// recorded behaviour
/// (`comprehend.detect-pii-entities-unsupported-language-still-succeeds`) is that it
/// answers 200 for a language its own documentation does not name, so the check lives
/// in each caller rather than in the recognizer.
pub const PII_LANGUAGES: &[&str] = &["en", "es"];

/// The refusal message for a document past `quota_bytes`: `Some(message)` when
/// `text` exceeds the quota, `None` when it fits.
///
/// ONE size check and ONE message for every real-time inference operation, because
/// the message is built from the REQUEST — it reports the quota that applies and
/// the submitted document's UTF-8 byte length, which is the pair an adaptive client
/// re-chunks against — and only the quota differs between operations. Callers pair
/// it with their own modelled `TextSizeLimitExceededException`, whose HTTP status
/// (400) and error code the model fixes.
pub fn text_size_refusal(text: &str, quota_bytes: usize) -> Option<String> {
    let actual_bytes = text.len();
    (actual_bytes > quota_bytes).then(|| {
        format!(
            "Input text size exceeds limit. Max length of request text allowed is \
             {quota_bytes} bytes while in this request the text size is {actual_bytes} bytes"
        )
    })
}

/// The most documents one synchronous batch inference request may carry.
///
/// ONE ceiling for the whole `BatchDetect*` family, which is what the model
/// documents: every batch operation's `TextList` says "The list can contain a
/// maximum of 25 documents" and the `BatchSizeLimitExceededException` they all
/// declare names the same 25. The model's `@length` trait states only the floor of
/// 1, so the ceiling is enforced in the handlers, through this constant and
/// [`batch_size_limit_refusal`].
pub const MAX_BATCH_DOCUMENTS: usize = 25;

/// The message `BatchSizeLimitExceededException` carries, for every batch operation.
///
/// CONSTANT text: the service reports the quota that was broken and nothing about
/// the request that broke it — no document count, no operation name — so the same
/// quota violation reads identically whichever batch operation was called, and a
/// caller matching the sentence sees one condition rather than one per operation.
/// The only substitution is [`MAX_BATCH_DOCUMENTS`] itself, so the quota stays
/// single-sourced.
pub fn batch_size_limit_refusal() -> String {
    format!(
        "Please use a smaller number of items in a single batch. Maximum batch size \
         is {MAX_BATCH_DOCUMENTS}."
    )
}

/// The request-level size refusal for a batch: `Some(message)` when ANY submitted
/// document is past `quota_bytes`, `None` when every one of them fits.
///
/// The per-document byte quota is a PRECONDITION OF THE REQUEST, not a per-item
/// outcome: a `TextList` that mixes well-formed documents with one oversized
/// document is refused in its entirety with the caller's own modelled
/// `TextSizeLimitExceededException` (HTTP 400) and no `ResultList`/`ErrorList` body
/// at all, so the batch is either analysed whole or refused whole and a caller can
/// rely on `ResultList` being aligned with the list it submitted. The identical
/// document submitted alone is refused by the same quota, through the same
/// [`text_size_refusal`] sentence — which names the quota and the OFFENDING
/// document's byte length, the pair an adaptive client re-chunks against. When more
/// than one document is oversized the first one in submission order is the one
/// reported, because it is the first the service reads.
pub fn batch_text_size_refusal<'a, I>(texts: I, quota_bytes: usize) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    texts
        .into_iter()
        .find_map(|text| text_size_refusal(text, quota_bytes))
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

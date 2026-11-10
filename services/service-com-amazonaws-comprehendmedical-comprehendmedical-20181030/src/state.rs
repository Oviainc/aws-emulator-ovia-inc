// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-comprehendmedical-comprehendmedical-20181030 service.
//!
//! The asynchronous detection/inference operations of Amazon Comprehend Medical all
//! share one record shape ([`AsyncJob`]), one status machine ([`JobStatus`]) and one
//! per-family store ([`JobFamilyStore`]), so all of them live here rather than in any
//! one operation's file: the entities-detection and PHI-detection families are
//! implemented today and the ICD-10-CM / RxNorm / SNOMED CT families reuse these
//! structures when they are implemented.

use std::collections::{BTreeMap, HashMap};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    ComprehendMedicalAsyncJobFilter, ComprehendMedicalAsyncJobProperties, InputDataConfig,
    OutputDataConfig,
};

/// The version this emulator reports for the model behind an asynchronous job.
/// The model documents the shape of the value ("The version number looks like X.X.X"),
/// not a particular version, so one constant is the single source for every job record
/// of every job family.
pub const ASYNC_JOB_MODEL_VERSION: &str = "2.0.0";

/// How long after `StartEntitiesDetectionV2Job` a job leaves `SUBMITTED`.
///
/// A started job is reported in a non-terminal status, so the move out of it is a
/// transition the clock owes rather than something a caller triggers. The span is
/// deliberately non-zero: a caller that describes or stops the job straight after
/// starting it still observes `SUBMITTED`.
pub const JOB_SUBMITTED_SPAN_SECS: f64 = 0.3;

/// How long a job stays `IN_PROGRESS` before the clock owes its terminal status.
pub const JOB_RUNNING_SPAN_SECS: f64 = 0.3;

/// How long after `StopEntitiesDetectionV2Job` a job leaves `STOP_REQUESTED` for
/// `STOPPED`. Non-zero for the same reason as [`JOB_SUBMITTED_SPAN_SECS`]: a caller
/// that describes the job straight after asking for the stop sees `STOP_REQUESTED`.
pub const JOB_STOPPING_SPAN_SECS: f64 = 0.3;

/// How long job metadata is kept before it is dropped, which is what the modelled
/// `ExpirationTime` reports: "The date and time that job metadata is deleted from the
/// server. […] After the metadata is deleted, the job will no longer appear in the
/// results of the `ListEntitiesDetectionV2Job` […] operation." AWS publishes no value
/// for the span itself, so this one is the emulator's own choice.
pub const JOB_METADATA_RETENTION_SECS: f64 = 90.0 * 24.0 * 60.0 * 60.0;

/// The status of an asynchronous job, as the model's `JobStatus` enum defines it.
///
/// The wire spellings in [`JobStatus::as_wire`] are exactly the members of
/// `JOB_STATUS_VALUES` (`crate::ops::types::JOB_STATUS_VALUES`, generated from the
/// model's `JobStatus` enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Submitted,
    InProgress,
    Completed,
    PartialSuccess,
    Failed,
    StopRequested,
    Stopped,
}

impl JobStatus {
    /// This status as the model spells it on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Submitted => "SUBMITTED",
            Self::InProgress => "IN_PROGRESS",
            Self::Completed => "COMPLETED",
            Self::PartialSuccess => "PARTIAL_SUCCESS",
            Self::Failed => "FAILED",
            Self::StopRequested => "STOP_REQUESTED",
            Self::Stopped => "STOPPED",
        }
    }

    /// The status a wire value names, or `None` when it names none.
    pub fn from_wire(value: &str) -> Option<Self> {
        [
            Self::Submitted,
            Self::InProgress,
            Self::Completed,
            Self::PartialSuccess,
            Self::Failed,
            Self::StopRequested,
            Self::Stopped,
        ]
        .into_iter()
        .find(|status| status.as_wire() == value)
    }

    /// Whether the job has finished: a terminal status never changes again, so there is
    /// nothing left for a stop request to do.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::PartialSuccess | Self::Failed | Self::Stopped
        )
    }
}

/// A stop request that named a job which had already reached a terminal status.
///
/// Carries the sentence the service states for it, so every job family's stop operation
/// refuses that request with one wording rather than each inventing its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobAlreadyTerminal;

impl JobAlreadyTerminal {
    /// What the refusal reports: the job had finished, so it cannot be stopped.
    pub fn message(self) -> &'static str {
        "Job is already in terminal state."
    }
}

/// A request that named a job the account has no record of in the family it was sent to:
/// an id nothing ever minted, an id another family minted, or one whose metadata has
/// already been dropped past its `ExpirationTime`.
///
/// Carries the sentence the service states for it, so every family's per-id operations
/// refuse an absent job with one wording. The whole message is FIXED text with no
/// variable part: the identifier the caller sent is NOT echoed back, so this refusal
/// reveals nothing about which ids the account holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobNotFound;

impl JobNotFound {
    /// What the refusal reports: the request named no job the service can find.
    pub fn message(self) -> &'static str {
        "Could not find specified job."
    }
}

/// Wall-clock now, in epoch seconds — the clock every job time is measured on.
pub fn now_epoch_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs_f64())
        .unwrap_or_default()
}

/// A stored timestamp as epoch seconds, for comparing it with [`now_epoch_secs`] and
/// with a filter's bound.
pub fn epoch_secs(at: &DateTime) -> f64 {
    at.as_secs_f64()
}

/// Epoch seconds as the modelled timestamp type.
pub fn date_time(epoch_secs: f64) -> DateTime {
    DateTime::from_secs_f64(epoch_secs)
}

/// One asynchronous job, holding exactly what the start call was given plus the status
/// the clock has moved it to.
#[derive(Debug, Clone)]
pub struct AsyncJob {
    pub job_id: String,
    pub job_name: Option<String>,
    pub job_status: JobStatus,
    pub message: Option<String>,
    pub submit_time: DateTime,
    pub end_time: Option<DateTime>,
    pub expiration_time: Option<DateTime>,
    pub input_data_config: InputDataConfig,
    pub output_data_config: OutputDataConfig,
    pub language_code: String,
    pub data_access_role_arn: String,
    pub manifest_file_path: Option<String>,
    pub kms_key: Option<String>,
    pub model_version: String,
    pub client_request_token: Option<String>,
    /// When a caller asked for the stop, which is what the move to `STOPPED` is due
    /// from. `None` until a stop is requested.
    pub stop_requested_time: Option<DateTime>,
}

/// What a start call was given, carried into the stored record.
///
/// Every asynchronous job family's start operation takes the same members (each one's
/// generated input shape repeats them), so one struct carries them from any of those
/// handlers into [`AsyncJob::submitted`] and no handler repeats the record literal.
#[derive(Debug, Clone)]
pub struct JobSubmission {
    pub job_name: Option<String>,
    pub input_data_config: InputDataConfig,
    pub output_data_config: OutputDataConfig,
    pub language_code: String,
    pub data_access_role_arn: String,
    pub kms_key: Option<String>,
    pub client_request_token: Option<String>,
}

impl AsyncJob {
    /// The record a start call creates for `job_id`: exactly what the caller asked for,
    /// in the non-terminal status a job AWS has accepted but not finished is reported
    /// in, with the instant its metadata expires already fixed.
    pub fn submitted(job_id: String, now: f64, request: JobSubmission) -> Self {
        Self {
            job_id,
            job_name: request.job_name,
            // A job AWS has accepted but not finished is reported in a non-terminal
            // status; [`AsyncJob::advance`] owns every move out of it.
            job_status: JobStatus::Submitted,
            message: None,
            submit_time: date_time(now),
            end_time: None,
            expiration_time: Some(date_time(now + JOB_METADATA_RETENTION_SECS)),
            input_data_config: request.input_data_config,
            output_data_config: request.output_data_config,
            language_code: request.language_code,
            data_access_role_arn: request.data_access_role_arn,
            // `ManifestFilePath` is "the path to the file that describes the results of
            // a batch job": there are no results until a run writes some, so none is
            // reported here.
            manifest_file_path: None,
            kms_key: request.kms_key,
            model_version: ASYNC_JOB_MODEL_VERSION.to_string(),
            client_request_token: request.client_request_token,
            stop_requested_time: None,
        }
    }

    /// Materialise every status change the clock already owes at `now`.
    ///
    /// This is called by the reporting operations (describe, list) and before a stop
    /// request is applied: each transition was due at a fixed instant whether or not
    /// anybody called, so applying it on read changes nothing a later caller would
    /// otherwise have seen.
    ///
    /// The terminal status of a started job is `FAILED`: running the job means reading
    /// the input documents from the Amazon S3 location the caller named and writing
    /// analysed output back, and this emulator serves no object storage for batch jobs,
    /// so no document is ever read and no output is ever written. The job therefore ends
    /// in the modelled failure state with a `Message` that says exactly that, rather
    /// than reporting a `COMPLETED` run that nothing performed.
    pub fn advance(&mut self, now: f64) {
        let submitted_at = epoch_secs(&self.submit_time);

        if self.job_status == JobStatus::Submitted && now >= submitted_at + JOB_SUBMITTED_SPAN_SECS
        {
            self.job_status = JobStatus::InProgress;
        }

        if self.job_status == JobStatus::InProgress {
            let due = submitted_at + JOB_SUBMITTED_SPAN_SECS + JOB_RUNNING_SPAN_SECS;
            if now >= due {
                self.job_status = JobStatus::Failed;
                self.end_time = Some(date_time(due));
                self.message = Some(self.unreadable_input_message());
            }
        }

        if self.job_status == JobStatus::StopRequested {
            if let Some(requested_at) = self.stop_requested_time.as_ref().map(epoch_secs) {
                let due = requested_at + JOB_STOPPING_SPAN_SECS;
                if now >= due {
                    self.job_status = JobStatus::Stopped;
                    self.end_time = Some(date_time(due));
                }
            }
        }
    }

    /// Record that a caller asked for this job to stop.
    ///
    /// A job that already reached a terminal status has nothing left to stop, so the
    /// request is REFUSED and the stored job is left exactly as it was: the stop
    /// operations document stopping a job "in progress", and a caller has to be able to
    /// tell a stop that took effect from one that named a job which had already
    /// finished. A stop repeated while the first is still pending is the same request
    /// again, so it is accepted and changes nothing.
    pub fn request_stop(&mut self, now: f64) -> Result<(), JobAlreadyTerminal> {
        if self.job_status.is_terminal() {
            return Err(JobAlreadyTerminal);
        }
        if self.job_status == JobStatus::StopRequested {
            return Ok(());
        }
        self.job_status = JobStatus::StopRequested;
        self.stop_requested_time = Some(date_time(now));
        self.message = Some("The job was stopped at the request of the caller.".to_string());
        Ok(())
    }

    /// Whether this job's metadata has been kept past its `ExpirationTime`.
    pub fn is_expired(&self, now: f64) -> bool {
        self.expiration_time
            .as_ref()
            .is_some_and(|at| now >= epoch_secs(at))
    }

    /// This job as the modelled `ComprehendMedicalAsyncJobProperties`, which is what
    /// both describe and list report.
    pub fn to_properties(&self) -> ComprehendMedicalAsyncJobProperties {
        ComprehendMedicalAsyncJobProperties {
            job_id: Some(self.job_id.clone()),
            job_name: self.job_name.clone(),
            job_status: Some(self.job_status.as_wire().to_string()),
            message: self.message.clone(),
            submit_time: Some(self.submit_time),
            end_time: self.end_time,
            expiration_time: self.expiration_time,
            input_data_config: Some(self.input_data_config.clone()),
            output_data_config: Some(self.output_data_config.clone()),
            language_code: Some(self.language_code.clone()),
            data_access_role_arn: Some(self.data_access_role_arn.clone()),
            manifest_file_path: self.manifest_file_path.clone(),
            kms_key: self.kms_key.clone(),
            model_version: Some(self.model_version.clone()),
        }
    }

    /// Why a started job fails: the input it was pointed at was never read.
    fn unreadable_input_message(&self) -> String {
        let input_key = self.input_data_config.s3_key.as_deref().unwrap_or("");
        let output_key = self.output_data_config.s3_key.as_deref().unwrap_or("");
        format!(
            "Could not read the input documents from s3://{}/{}: this emulator does not serve \
             object storage for asynchronous batch jobs, so no document was analyzed and no \
             output was written to s3://{}/{}.",
            self.input_data_config.s3_bucket,
            input_key,
            self.output_data_config.s3_bucket,
            output_key,
        )
    }
}

/// One family of asynchronous jobs: the jobs it holds, and the request tokens that
/// created them.
///
/// Each family (entities detection, PHI detection, …) keeps its own store, so a `JobId`
/// or a `ClientRequestToken` resolves only within the family whose operations issued
/// it: a PHI job never answers a describe, stop or list of another family.
#[derive(Debug, Default)]
pub struct JobFamilyStore {
    /// This family's jobs, keyed by `JobId`. A minted id begins with the submit time
    /// (see
    /// [`ComAmazonawsComprehendmedicalComprehendmedical20181030State::mint_job_id`]), so
    /// the map's key order IS submit order, which is the order a list operation pages
    /// through.
    pub jobs: BTreeMap<String, AsyncJob>,
    /// `ClientRequestToken` -> the `JobId` it created, so repeating a start call with
    /// one token returns the first job instead of creating a second.
    pub idempotency_tokens: HashMap<String, String>,
}

impl JobFamilyStore {
    /// The job a `ClientRequestToken` already created, if it created one.
    pub fn job_for_token(&self, token: &str) -> Option<&str> {
        self.idempotency_tokens.get(token).map(String::as_str)
    }

    /// Store a started job, remembering the request token that created it.
    pub fn record(&mut self, job: AsyncJob) {
        if let Some(token) = job.client_request_token.clone() {
            self.idempotency_tokens.insert(token, job.job_id.clone());
        }
        self.jobs.insert(job.job_id.clone(), job);
    }

    /// The job `job_id` names, with everything the clock already owes it applied —
    /// metadata kept past its `ExpirationTime` is dropped first, then the job's own status
    /// progression is materialised — or [`JobNotFound`] when this family holds no such
    /// job. Both writes were due at instants fixed when the jobs were started, so applying
    /// them while reporting shows a later caller nothing it would not otherwise have seen.
    ///
    /// Every per-id operation of a family (describe, stop) resolves its `JobId` through
    /// here, so one id resolves — and one absent id is refused — the same way on each of
    /// them. Only THIS family's metadata is swept: another family materialises what the
    /// clock owes it on its own next call, so no caller sees a job that should have
    /// expired.
    pub fn job_mut(&mut self, job_id: &str, now: f64) -> Result<&mut AsyncJob, JobNotFound> {
        self.expire(now);
        let job = self.jobs.get_mut(job_id).ok_or(JobNotFound)?;
        job.advance(now);
        Ok(job)
    }

    /// Materialise everything the clock already owes this family at `now`: metadata kept
    /// past its `ExpirationTime` is dropped and every remaining job's status progression
    /// is applied. Both were due at instants fixed when the jobs were started, so
    /// applying them while reporting shows a later caller nothing it would not otherwise
    /// have seen.
    pub fn settle(&mut self, now: f64) {
        self.expire(now);
        for job in self.jobs.values_mut() {
            job.advance(now);
        }
    }

    /// Drop the jobs whose metadata has been kept past its `ExpirationTime`, which the
    /// model says stops them appearing in a list, along with the request tokens that
    /// named them.
    fn expire(&mut self, now: f64) {
        let dropped_tokens: Vec<String> = self
            .jobs
            .values()
            .filter(|job| job.is_expired(now))
            .filter_map(|job| job.client_request_token.clone())
            .collect();
        for token in dropped_tokens {
            self.idempotency_tokens.remove(&token);
        }
        self.jobs.retain(|_, job| !job.is_expired(now));
    }
}

/// The page size a list operation uses when the caller names none: "The maximum number
/// of results to return in each page. The default is 100." (the model's own `MaxResults`
/// documentation).
pub const DEFAULT_MAX_RESULTS: usize = 100;

/// Whether `job` satisfies every member the caller set on the filter. An absent member
/// filters nothing; a member that is set must match.
///
/// The model documents the two time members as "Returns only jobs submitted
/// before/after the specified time" without saying what a job submitted AT the bound
/// does, so both bounds are inclusive here: a job on the boundary instant is returned
/// by either filter rather than lost by both.
fn matches_filter(job: &AsyncJob, filter: Option<&ComprehendMedicalAsyncJobFilter>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    let submitted_at = epoch_secs(&job.submit_time);
    let name_matches = filter
        .job_name
        .as_ref()
        .is_none_or(|wanted| job.job_name.as_deref() == Some(wanted.as_str()));
    // The filter's `JobStatus` is a modelled enum, so the boundary validator has
    // already refused a value outside it; parsing it back into the stored status keeps
    // the comparison on one representation.
    let status_matches = filter.job_status.as_ref().is_none_or(|wanted| {
        JobStatus::from_wire(wanted).is_some_and(|status| status == job.job_status)
    });
    let before_matches = filter
        .submit_time_before
        .as_ref()
        .is_none_or(|before| submitted_at <= epoch_secs(before));
    let after_matches = filter
        .submit_time_after
        .as_ref()
        .is_none_or(|after| submitted_at >= epoch_secs(after));
    name_matches && status_matches && before_matches && after_matches
}

/// The page token that resumes after `job_id`: the resume key, hex-encoded so a client
/// treats it as the opaque string the model describes rather than something to build.
pub fn page_token(job_id: &str) -> String {
    hex::encode(job_id)
}

/// The resume key a page token carries, or `None` when it is not a token this service
/// issued.
///
/// A token must decode to something shaped like a minted `JobId` (32 lowercase hex
/// characters, see
/// [`ComAmazonawsComprehendmedicalComprehendmedical20181030State::mint_job_id`]);
/// anything else is refused rather than silently restarting at the first page. A
/// well-formed cursor that names no stored job is NOT an error — it is a position, so it
/// resumes after that position, which is what an opaque cursor means.
pub fn resume_key(token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let minted_shape = decoded.len() == 32
        && decoded
            .chars()
            .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch));
    minted_shape.then_some(decoded)
}

/// One page of a family's jobs: those that satisfy every member the caller set on the
/// filter, in the order the filter documents, cut at `page_size`, together with the
/// token that resumes after the page — `None` when nothing follows it.
///
/// The order comes from the model's own filter documentation: `SubmitTimeBefore`
/// returns jobs "in ascending order, oldest to newest" and `SubmitTimeAfter` "in
/// descending order, newest to oldest", so a list bounded only from below is
/// newest-first and every other list keeps the ascending submit order the family's keys
/// already sort in.
pub fn page_of_jobs(
    family: &JobFamilyStore,
    filter: Option<&ComprehendMedicalAsyncJobFilter>,
    resume_after: Option<&str>,
    page_size: usize,
) -> (Vec<ComprehendMedicalAsyncJobProperties>, Option<String>) {
    let newest_first = filter.is_some_and(|filter| {
        filter.submit_time_after.is_some() && filter.submit_time_before.is_none()
    });
    let mut matching: Vec<&AsyncJob> = family
        .jobs
        .values()
        .filter(|job| matches_filter(job, filter))
        .collect();
    if newest_first {
        matching.reverse();
    }

    // A resume key is a position in THIS order, so the comparison follows the order:
    // the page continues at the first job past the key in whichever direction the jobs
    // are being returned.
    let start = match resume_after {
        Some(key) => matching
            .iter()
            .position(|job| {
                if newest_first {
                    job.job_id.as_str() < key
                } else {
                    job.job_id.as_str() > key
                }
            })
            .unwrap_or(matching.len()),
        None => 0,
    };
    let page = &matching[start..matching.len().min(start + page_size)];
    let next_token = matching
        .get(start + page.len())
        .and(page.last())
        .map(|last| page_token(&last.job_id));

    (
        page.iter().map(|job| job.to_properties()).collect(),
        next_token,
    )
}

/// Per-service state for com-amazonaws-comprehendmedical-comprehendmedical-20181030. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsComprehendmedicalComprehendmedical20181030State {
    /// The entities-detection (V2) jobs.
    pub entities_detection_v2_jobs: JobFamilyStore,
    /// The protected-health-information detection jobs.
    pub phi_detection_jobs: JobFamilyStore,
    /// Distinguishes two jobs minted within the same microsecond.
    pub next_job_seq: u64,
}

impl ComAmazonawsComprehendmedicalComprehendmedical20181030State {
    /// A fresh `JobId`: 32 lowercase hex characters, within the modelled length
    /// (1..32) and pattern. The leading 16 characters are the submit time in
    /// microseconds and the trailing 16 a sequence number, so ids sort in submit order
    /// and two jobs started in the same microsecond still differ.
    ///
    /// The time prefix is clamped into the cast's own domain rather than left to a
    /// saturating `as` conversion: a clock that answered a non-finite or negative value
    /// would otherwise land every id on prefix 0 and break the key-order-is-submit-order
    /// invariant pagination reads. Uniqueness does not depend on the clock — the
    /// sequence number carries it — so the clamp degrades ordering, never identity.
    pub fn mint_job_id(&mut self, now: f64) -> String {
        let seq = self.next_job_seq;
        self.next_job_seq = self.next_job_seq.wrapping_add(1);
        /// The largest microsecond prefix the 16 hex characters hold.
        const MAX_MICROS: f64 = u64::MAX as f64;
        let micros = if now.is_finite() {
            (now * 1_000_000.0).clamp(0.0, MAX_MICROS) as u64
        } else {
            0
        };
        format!("{micros:016x}{seq:016x}")
    }

    /// Drop the jobs of every family whose metadata has been kept past its
    /// `ExpirationTime`, which the model says stops them appearing in a list, along with
    /// the request tokens that named them. Expiry is due at an instant fixed when the job
    /// was created, so materialising it on a read is not a mutation.
    pub fn expire_jobs(&mut self, now: f64) {
        self.entities_detection_v2_jobs.expire(now);
        self.phi_detection_jobs.expire(now);
    }
}

/// The Region a request is scoped to: the one its SigV4 credential scope names, or
/// [`DEFAULT_REGION`] for a request that carries no scope (an unsigned call).
///
/// A job record carries no Region of its own, so the only thing the Region decides here is
/// which emulated Region the in-process IAM question below is addressed to.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region a request with no SigV4 credential scope is scoped to. Matches the Region
/// the emulator's own generated client config defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The service principal a `DataAccessRoleArn` has to let assume it: an asynchronous job
/// reads its input documents and writes its output ACTING AS that role, so the role is
/// handed to this service and this service is the principal that assumes it.
pub const DATA_ACCESS_ROLE_SERVICE_PRINCIPAL: &str = "comprehendmedical.amazonaws.com";

/// What a start call reports for a well-formed `DataAccessRoleArn` naming a role this
/// service cannot assume — the whole sentence, with no variable part: the failure is about
/// the trust relationship, so the caller is pointed at the documentation that states which
/// trust policy an asynchronous job's role needs rather than at the value it sent.
///
/// The sibling failure — an ARN outside the modelled shape — shares the modelled error
/// code and is worded by `crate::service::validation`, so the MESSAGE is what tells the two
/// apart: that one asks the caller to correct the string, this one to re-trust the role.
pub const DATA_ACCESS_ROLE_NOT_TRUSTED: &str =
    "The provided IAM role does not trust Comprehend Medical service principle. See: \
     https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam";

/// Whether this emulator's own IAM ANSWERED that `role_arn` is a role this service cannot
/// assume: this account holds no such role, or the trust policy it carries does not admit
/// [`DATA_ACCESS_ROLE_SERVICE_PRINCIPAL`].
///
/// An asynchronous job RUNS AS its `DataAccessRoleArn`, so every start operation resolves
/// that role before it mints a `JobId` or records anything: a role this service cannot
/// assume backs no job at all. The whole question — which role of this account the value
/// names, what IAM answers about it, and whether the trust document it carries admits a
/// service principal — is the ONE reading every emulated service that resolves a
/// caller-supplied role shares (`service_common::iam_role::role_is_unassumable`), so this
/// crate states only WHICH principal has to be admitted and WHICH operations ask.
///
/// IAM is reached the way any emulated service reaches another: the AWS SDK over the
/// emulator's OWN in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`]), so nothing here opens a socket
/// or reads an ambient credential. `false` — do not refuse — for every case where nothing
/// is KNOWN about the role, including a value naming another account's role and an
/// emulator with no IAM mounted to ask: a refusal has to be grounded in what IAM answered,
/// never assumed. Awaited with NO store guard held.
pub async fn data_access_role_is_unassumable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> bool {
    ::service_common::iam_role::role_is_unassumable(
        ctx,
        region,
        role_arn,
        DATA_ACCESS_ROLE_SERVICE_PRINCIPAL,
    )
    .await
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsComprehendmedicalComprehendmedical20181030State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsComprehendmedicalComprehendmedical20181030State>)
}

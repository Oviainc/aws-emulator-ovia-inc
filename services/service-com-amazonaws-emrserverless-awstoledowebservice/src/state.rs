// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-emrserverless-awstoledowebservice service.
//!
//! The application is the primary resource of the EMR Serverless control plane, and a
//! job run lives inside one application. [`ApplicationRecord`] is the ONE copy of an
//! application every handler reads and writes: `CreateApplication` mints it,
//! `GetApplication` / `ListApplications` report it, `StartApplication` /
//! `StopApplication` move its state, `DeleteApplication` removes it, and
//! `StartJobRun` / `GetJobRun` / `ListJobRuns` / `CancelJobRun` work on the job runs it
//! owns.
//!
//! Each record holds the MODELLED shape (`Application`, `JobRun`) verbatim, so a
//! reporting operation returns exactly what the mutating one stored rather than
//! re-deriving anything from a request.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    Application, ApplicationSummary, AutoStartConfig, AutoStopConfig, IdentityCenterConfiguration,
    IdentityCenterConfigurationInput, ImageConfiguration, ImageConfigurationInput, JobRun,
    JobRunSummary, ResourceNotFoundException, WorkerTypeSpecification,
    WorkerTypeSpecificationInput,
};

/// This service's ARN namespace, and the `sigv4Name` the model declares
/// (`smithy_query(query="service_info")`): every ARN this emulator mints for an
/// application or a job run carries it.
pub const ARN_NAMESPACE: &str = "emr-serverless";

/// The region an ARN is built for when the request carries no SigV4 credential scope
/// to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The largest page either list operation serves, and the page size it uses when the
/// request names no `maxResults`.
///
/// Source: `ListApplicationsRequest$maxResults` and `ListJobRunsRequest$maxResults`
/// are both modelled `@range(min: 1, max: 50)` with no declared default.
pub const PAGE_LIMIT: i32 = 50;

/// How many applications the store holds before `CreateApplication` refuses.
///
/// EMR Serverless publishes an adjustable per-account quota on applications, which
/// this emulator does not model, so this is its OWN bound on the store rather than a
/// claim about any account's limit: set far above any workflow an emulator serves,
/// while keeping the state a caller can grow bounded.
pub const MAX_APPLICATIONS: usize = 1_000;

/// The same bound, for the job runs one application holds.
pub const MAX_JOB_RUNS_PER_APPLICATION: usize = 1_000;

/// The job run timeout stored when `StartJobRun` names none.
///
/// `JobRun$executionTimeoutMinutes` documents it: "Returns the job run timeout value
/// from the `StartJobRun` call. If you didn't specify a timeout, this value defaults
/// to 720 minutes."
pub const DEFAULT_EXECUTION_TIMEOUT_MINUTES: i64 = 720;

/// Whether an application automatically starts on job submission when
/// `autoStartConfiguration` names nothing.
///
/// `AutoStartConfig$enabled` documents it: "Enables the application to automatically
/// start on job submission. Defaults to true."
pub const DEFAULT_AUTO_START_ENABLED: bool = true;

/// Whether an application automatically stops when idle, when
/// `autoStopConfiguration` names nothing.
///
/// `AutoStopConfig$enabled` documents it: "Enables the application to automatically
/// stop after a certain amount of time being idle. Defaults to true."
pub const DEFAULT_AUTO_STOP_ENABLED: bool = true;

/// The idle timeout stored when `autoStopConfiguration` names none.
///
/// `AutoStopConfig$idleTimeoutMinutes` documents it: "The amount of idle time in
/// minutes after which your application will automatically stop. Defaults to 15
/// minutes."
pub const DEFAULT_IDLE_TIMEOUT_MINUTES: i32 = 15;

/// How long a resource stays in the state the call that started the work returned
/// (`CREATING` after a create, `STARTING` after a start, `STOPPING` after a stop,
/// `CANCELLING` after a cancel) before the clock owes it the settled state.
///
/// EMR Serverless takes seconds to minutes; the emulator CHOOSES this span, and it is
/// deliberately not zero so a caller acting straight after the call still observes the
/// transient state. The due instant is stored on the record and materialised by
/// whichever later call reads it — nothing sleeps.
pub const LIFECYCLE_TRANSITION: Duration = Duration::from_millis(500);

/// `JobRun$stateDetails` for a job run that has been accepted but not yet run.
pub const SUBMITTED_STATE_DETAILS: &str = "Job run submitted.";

/// `JobRun$stateDetails` for the state the clock owes every submitted job run.
///
/// This emulator is a control plane: it records a job run faithfully, but it runs no
/// Spark or Hive workload, so a job run can never reach `SUCCESS` — it ends in the
/// modelled failure state with a reason that says exactly that.
pub const NOT_EXECUTED_STATE_DETAILS: &str =
    "This emulator records job runs but executes no Spark or Hive workload, so the job \
     run ended without running.";

/// `JobRun$stateDetails` for a job run a caller cancelled.
pub const CANCELLED_STATE_DETAILS: &str = "Job run cancelled at the request of the caller.";

/// The `JobRunState` values from which a job run never moves again.
///
/// Source: the `JobRunState` enum (`SUBMITTED`, `PENDING`, `SCHEDULED`, `RUNNING`,
/// `SUCCESS`, `FAILED`, `CANCELLING`, `CANCELLED`, `QUEUED`) — the three that name a
/// finished run.
pub const TERMINAL_JOB_RUN_STATES: &[&str] = &["SUCCESS", "FAILED", "CANCELLED"];

/// The `ApplicationState` values `DeleteApplication` admits.
///
/// Source: the operation's own documentation — "An application has to be in a stopped
/// or created state in order to be deleted."
pub const DELETABLE_APPLICATION_STATES: &[&str] = &["CREATED", "STOPPED"];

/// The prefix a pagination cursor carries, so a token this service did not mint is
/// refused rather than read as a resume point.
const PAGE_TOKEN_PREFIX: &str = "v1_";

/// True when `state` names a `JobRunState` a job run never leaves.
pub fn is_terminal_job_run_state(state: &str) -> bool {
    TERMINAL_JOB_RUN_STATES.contains(&state)
}

/// A state change the clock owes a resource: the instant it falls due, the state the
/// resource carries from then on, and the detail text that goes with it.
#[derive(Debug, Clone)]
pub struct PendingState {
    pub due: SystemTime,
    pub state: String,
    pub state_details: Option<String>,
}

/// One job run, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct JobRunRecord {
    /// The modelled `JobRun` shape `GetJobRun` returns verbatim.
    pub job_run: JobRun,
    /// `JobRunSummary$type` — the engine type of the application the run was
    /// submitted to. `JobRun` carries no such member, so it is held beside it.
    pub job_run_type: String,
    /// The state change the clock owes this job run, if any.
    pub pending: Option<PendingState>,
}

impl JobRunRecord {
    /// Materialise the state change the clock owes this job run, if `now` is past its
    /// due instant.
    ///
    /// `updatedAt` moves to the DUE instant rather than to `now`: the change happened
    /// when it fell due, whether or not anybody was reading. `endedAt` is set only
    /// when the settled state is one a job run never leaves.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if now < pending.due {
            return;
        }
        let at = DateTime::from(pending.due);
        self.job_run.state = pending.state.clone();
        if let Some(details) = pending.state_details.as_ref() {
            self.job_run.state_details = details.clone();
        }
        self.job_run.updated_at = at;
        self.job_run.attempt_updated_at = Some(at);
        if is_terminal_job_run_state(&self.job_run.state) {
            self.job_run.ended_at = Some(at);
        }
        self.pending = None;
    }

    /// The `JobRunSummary` projection `ListJobRuns` returns: the stored record's own
    /// members, never request input.
    pub fn summary(&self) -> JobRunSummary {
        let run = &self.job_run;
        JobRunSummary {
            application_id: run.application_id.clone(),
            id: run.job_run_id.clone(),
            name: run.name.clone(),
            mode: run.mode.clone(),
            arn: run.arn.clone(),
            created_by: run.created_by.clone(),
            created_at: run.created_at,
            updated_at: run.updated_at,
            execution_role: run.execution_role.clone(),
            state: run.state.clone(),
            state_details: run.state_details.clone(),
            release_label: run.release_label.clone(),
            r#type: Some(self.job_run_type.clone()),
            attempt: run.attempt,
            attempt_created_at: run.attempt_created_at,
            attempt_updated_at: run.attempt_updated_at,
        }
    }
}

/// One EMR Serverless application, as this emulator persists it, together with the job
/// runs submitted to it.
#[derive(Debug, Clone)]
pub struct ApplicationRecord {
    /// The modelled `Application` shape `GetApplication` returns verbatim.
    pub application: Application,
    /// The state change the clock owes this application, if any.
    pub pending: Option<PendingState>,
    /// This application's job runs, keyed by id. A `BTreeMap` is what gives
    /// `ListJobRuns` the stable total order its cursor indexes into — ids sort in the
    /// order they were minted.
    pub job_runs: BTreeMap<String, JobRunRecord>,
    /// A `StartJobRun` `clientToken` to the job run that token started, so a retried
    /// submit answers with the first job run instead of a second one.
    pub job_run_tokens: HashMap<String, String>,
    /// The counter job run ids are minted from.
    pub next_job_run_seq: u64,
}

impl ApplicationRecord {
    /// Materialise every state change the clock owes this application and its job
    /// runs, if `now` is past their due instants.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.as_ref() {
            if now >= pending.due {
                self.application.state = pending.state.clone();
                if let Some(details) = pending.state_details.as_ref() {
                    self.application.state_details = Some(details.clone());
                }
                self.application.updated_at = DateTime::from(pending.due);
                self.pending = None;
            }
        }
        for record in self.job_runs.values_mut() {
            record.settle(now);
        }
    }

    /// Mint an unused job run id matching `JobRunId`'s `^[0-9a-z]+$`.
    ///
    /// Sixteen lowercase hex digits, like the ids EMR Serverless issues; the loop
    /// skips an id already taken, which is what makes the counter's wrap safe.
    pub fn mint_job_run_id(&mut self) -> String {
        loop {
            self.next_job_run_seq = self.next_job_run_seq.wrapping_add(1);
            let id = format!("{:016x}", self.next_job_run_seq);
            if !self.job_runs.contains_key(&id) {
                return id;
            }
        }
    }

    /// Whether this application starts by itself when a job run is submitted to it.
    ///
    /// `AutoStartConfig$enabled` documents the default as true, and
    /// `CreateApplication` materialises it, so an absent configuration here is read
    /// the same way.
    pub fn auto_start_enabled(&self) -> bool {
        self.application
            .auto_start_configuration
            .as_ref()
            .and_then(|config| config.enabled)
            .unwrap_or(DEFAULT_AUTO_START_ENABLED)
    }

    /// The job runs of this application that have not finished.
    pub fn unfinished_job_run_ids(&self) -> Vec<String> {
        self.job_runs
            .values()
            .filter(|record| !is_terminal_job_run_state(&record.job_run.state))
            .map(|record| record.job_run.job_run_id.clone())
            .collect()
    }

    /// The `ApplicationSummary` projection `ListApplications` returns.
    pub fn summary(&self) -> ApplicationSummary {
        let application = &self.application;
        ApplicationSummary {
            id: application.application_id.clone(),
            name: application.name.clone(),
            arn: application.arn.clone(),
            release_label: application.release_label.clone(),
            r#type: application.r#type.clone(),
            state: application.state.clone(),
            state_details: application.state_details.clone(),
            created_at: application.created_at,
            updated_at: application.updated_at,
            architecture: application.architecture.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-emrserverless-awstoledowebservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEmrserverlessAwstoledowebserviceState {
    /// Every stored application, keyed by its id. A `BTreeMap` so `ListApplications`
    /// walks them in one stable order and a cursor taken on one page resumes in the
    /// right place on the next.
    pub applications: BTreeMap<String, ApplicationRecord>,
    /// A `CreateApplication` `clientToken` to the application that token created, so a
    /// repeated create answers with the first application instead of a second one.
    pub create_application_tokens: HashMap<String, String>,
    /// The counter application ids are minted from.
    pub next_application_seq: u64,
}

impl ComAmazonawsEmrserverlessAwstoledowebserviceState {
    /// Mint an unused application id matching `ApplicationId`'s `^[0-9a-z]+$`.
    pub fn mint_application_id(&mut self) -> String {
        loop {
            self.next_application_seq = self.next_application_seq.wrapping_add(1);
            let id = format!("{:016x}", self.next_application_seq);
            if !self.applications.contains_key(&id) {
                return id;
            }
        }
    }

    /// The application `id` names, with every state change the clock owes it (and its
    /// job runs) already materialised.
    pub fn settled_mut(&mut self, id: &str, now: SystemTime) -> Option<&mut ApplicationRecord> {
        let record = self.applications.get_mut(id)?;
        record.settle(now);
        Some(record)
    }

    /// Materialise every state change the clock owes, for a caller about to report on
    /// all of them.
    pub fn settle_all(&mut self, now: SystemTime) {
        for record in self.applications.values_mut() {
            record.settle(now);
        }
    }
}

/// The ARN of an application:
/// `arn:aws:emr-serverless:<region>:<account>:/applications/<applicationId>`.
///
/// Source: `ApplicationArn`'s pattern
/// `^arn:(aws[a-zA-Z0-9-]*):emr-serverless:.+:(\d{12}):\/applications\/[0-9a-zA-Z]+$`
/// — note the leading slash on the resource segment.
pub fn application_arn(region: &str, application_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("/applications/{application_id}"),
    )
}

/// The ARN of a job run, which `JobArn`'s pattern defines as its application's ARN
/// followed by `/jobruns/<jobRunId>`.
pub fn job_run_arn(application_arn: &str, job_run_id: &str) -> String {
    format!("{application_arn}/jobruns/{job_run_id}")
}

/// The region this request was signed for — the region every ARN it mints names.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The IAM principal this request was signed by: the `createdBy` of what it creates.
///
/// `RequestIdentityUserArn`'s pattern is
/// `^arn:(aws[a-zA-Z0-9-]*):(iam|sts)::(\d{12})?:[\w/+=,.@-]+$`, so the ARN carries no
/// region. The emulator models no identity store beyond the access key id a request is
/// signed with, which is exactly what names the caller here; it is filtered to the
/// characters the pattern's resource segment admits.
pub fn caller_arn(parts: &::http::request::Parts) -> String {
    let akid = ::engine::extract_akid(&parts.headers);
    let user: String = akid
        .chars()
        .filter(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '=' | ',' | '.' | '@' | '-')
        })
        .collect();
    let user = if user.is_empty() {
        "default".to_string()
    } else {
        user
    };
    ::engine::emulator_arn("iam", "", &format!("user/{user}"))
}

/// The `autoStartConfiguration` to persist: the caller's, with
/// `AutoStartConfig$enabled`'s documented default filled in.
pub fn adopt_auto_start(config: Option<AutoStartConfig>) -> Option<AutoStartConfig> {
    let config = config.unwrap_or_default();
    Some(AutoStartConfig {
        enabled: Some(config.enabled.unwrap_or(DEFAULT_AUTO_START_ENABLED)),
    })
}

/// The `autoStopConfiguration` to persist: the caller's, with `AutoStopConfig`'s two
/// documented defaults filled in.
pub fn adopt_auto_stop(config: Option<AutoStopConfig>) -> Option<AutoStopConfig> {
    let config = config.unwrap_or_default();
    Some(AutoStopConfig {
        enabled: Some(config.enabled.unwrap_or(DEFAULT_AUTO_STOP_ENABLED)),
        idle_timeout_minutes: Some(
            config
                .idle_timeout_minutes
                .unwrap_or(DEFAULT_IDLE_TIMEOUT_MINUTES),
        ),
    })
}

/// The `imageConfiguration` to persist for the `ImageConfigurationInput` a request
/// carried.
///
/// `ImageConfiguration$imageUri` is required, so an input naming no image stores
/// nothing. `resolvedImageDigest` stays unset: resolving it means pulling the image,
/// which this emulator does not do, so there is no digest to report.
pub fn adopt_image_configuration(
    input: Option<ImageConfigurationInput>,
) -> Option<ImageConfiguration> {
    let input = input?;
    let image_uri = input.image_uri?;
    Some(ImageConfiguration {
        image_uri,
        resolved_image_digest: None,
        application_level_digest_resolution: input.application_level_digest_resolution,
    })
}

/// The `workerTypeSpecifications` to persist for the input map a request carried.
pub fn adopt_worker_type_specifications(
    input: Option<HashMap<String, WorkerTypeSpecificationInput>>,
) -> Option<HashMap<String, WorkerTypeSpecification>> {
    let input = input?;
    Some(
        input
            .into_iter()
            .map(|(worker_type, specification)| {
                (
                    worker_type,
                    WorkerTypeSpecification {
                        image_configuration: adopt_image_configuration(
                            specification.image_configuration,
                        ),
                    },
                )
            })
            .collect(),
    )
}

/// The `identityCenterConfiguration` to persist for the input a request carried.
///
/// `identityCenterApplicationArn` names an IAM Identity Center application this
/// emulator does not create, so it stays unset rather than invented.
pub fn adopt_identity_center_configuration(
    input: Option<IdentityCenterConfigurationInput>,
) -> Option<IdentityCenterConfiguration> {
    let input = input?;
    Some(IdentityCenterConfiguration {
        identity_center_instance_arn: input.identity_center_instance_arn,
        identity_center_application_arn: None,
        user_background_sessions_enabled: input.user_background_sessions_enabled,
    })
}

/// The page size a list operation serves: the caller's `maxResults`, or [`PAGE_LIMIT`]
/// when the request named none. The modelled `@range(1..50)` is already enforced at the
/// boundary, so this only has to turn it into a usable count.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(PAGE_LIMIT);
    usize::try_from(requested.clamp(1, PAGE_LIMIT)).unwrap_or(1)
}

/// True when `value` satisfies `ApplicationId` / `JobRunId`'s `^[0-9a-z]+$` with their
/// 1..64 length. The boundary validator already refuses a modelled id member, so this
/// is for a value the model does not constrain: the id carried inside a `nextToken`.
pub fn is_resource_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
}

/// Encode the id a page resumes at as an opaque `nextToken`.
///
/// `NextToken`'s pattern is `^[A-Za-z0-9_=-]+$`, which the `v1_` prefix and lowercase
/// hex body both satisfy.
pub fn encode_page_token(id: &str) -> String {
    format!("{PAGE_TOKEN_PREFIX}{}", hex::encode(id))
}

/// The id a `nextToken` resumes at, or `None` when the token is not one this service
/// minted (the caller answers that with a `ValidationException`).
pub fn decode_page_token(token: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let id = String::from_utf8(hex::decode(body).ok()?).ok()?;
    is_resource_id(&id).then_some(id)
}

/// The `ResourceNotFoundException` payload for an application id that names none.
pub fn application_not_found(application_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Application {application_id} does not exist"),
    }
}

/// The `ResourceNotFoundException` payload for a job run id that names none in the
/// given application.
pub fn job_run_not_found(application_id: &str, job_run_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Job run {job_run_id} does not exist in application {application_id}"),
    }
}

/// Now, as the wire timestamp type every modelled timestamp member uses.
pub fn now_timestamp(now: SystemTime) -> DateTime {
    DateTime::from(now)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEmrserverlessAwstoledowebserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEmrserverlessAwstoledowebserviceState>)
}

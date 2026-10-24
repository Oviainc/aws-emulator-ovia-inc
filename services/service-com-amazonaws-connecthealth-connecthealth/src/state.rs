// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connecthealth-connecthealth service.
//!
//! The Domain -> Subscription lifecycle lives here. Every field a response carries is
//! read back OUT of these records: a handler persists what it was given, derives what
//! the service itself owns (ids, ARNs, timestamps, status), and then renders the stored
//! record. Nothing is echoed straight from a request.
//!
//! Helpers several operations share (id minting, ARN building, the opaque pagination
//! cursor, the `SubscriptionDescription` projection) live here rather than being copied
//! into each `src/ops/*.rs`: `src/lib.rs` is generated, so this module is the crate's
//! one shared, editable home.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    InputDataConfig, InsightsContext, InsightsOutput, MedicalScribeChannelDefinition,
    MedicalScribeListeningSessionDetails, MedicalScribePostStreamActionSettingsResponse,
    MedicalScribePostStreamActionsResult, OutputDataConfig, PatientInsightsEncounterContext,
    PatientInsightsPatientContext, SubscriptionDescription, UserContext,
};

/// The region an emulated ARN carries when the request names none.
const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace / SigV4 signing name (`OPERATIONS.md`, "Wire protocol").
const ARN_NAMESPACE: &str = "health-agent";

/// How long a `Domain` stays in `DELETING` before the deletion the clock owes is
/// materialised — the record is purged, and `GetDomain` then answers
/// `ResourceNotFoundException`.
///
/// AWS documents no duration for this, so the span is CHOSEN, and chosen inside the
/// emulator's transient window (250-750 ms): a caller that reads straight after
/// `DeleteDomain` still observes the `DELETING` state rather than an instant purge.
///
/// Only a domain whose deletion CANNOT finish inside the call waits this span — see
/// [`Domain::deletion_completes_in_call`].
pub const DOMAIN_DELETION_SPAN: Duration = Duration::from_millis(500);

/// The most `Domain` records ONE ACCOUNT holds at a time IN ONE REGION — the cap
/// `CreateDomain` is subject to.
///
/// RECORDED behaviour: `CreateDomain` past the cap is refused with a
/// `ValidationException` whose text names the account and this number
/// ("Account <accountId> has reached the maximum limit of 10 domains."). The cap is a
/// LIVE count of the domains the account currently holds in the Region the create is
/// addressed to, not a monotonic counter and not a cross-Region total, so a
/// `DeleteDomain` frees a slot for the next create and a domain in another Region
/// consumes none of this Region's quota.
///
/// It is a quota, NOT this store's growth bound: the Region a record is filed under is
/// read from the request, so a count per Region is a count per value a caller chose.
/// [`MAX_DOMAINS_HELD`] is what bounds the store.
pub const MAX_DOMAINS_PER_ACCOUNT: usize = 10;

/// How many distinct Regions this store files `Domain` records under.
///
/// The EMULATOR's own ceiling, not a rule of the service: it is set above the number of
/// Regions AWS operates, so no use of the emulator across real Regions reaches it.
pub const MAX_DOMAIN_REGIONS: usize = 64;

/// The most `Domain` records this store holds at once, across every account and every
/// Region: [`MAX_DOMAINS_PER_ACCOUNT`] in each of [`MAX_DOMAIN_REGIONS`] Regions.
///
/// THIS is what bounds the store's growth, and it is an absolute count rather than a
/// count per key read off the request: a caller looping `CreateDomain` cannot grow the
/// shared store past it whatever Region, and however many Regions, it signs for. It
/// bounds everything held UNDER a domain with it, since a domain holds at most
/// [`MAX_SUBSCRIPTIONS_PER_DOMAIN`] subscriptions and
/// [`MAX_PATIENT_INSIGHTS_JOBS_PER_DOMAIN`] jobs, and a tag entry is keyed by the ARN of
/// one of those records.
///
/// Enforced in `CreateDomain`, the one operation that adds a `Domain`, which refuses
/// with the `ServiceQuotaExceededException` its model declares once the store is full.
pub const MAX_DOMAINS_HELD: usize = MAX_DOMAINS_PER_ACCOUNT * MAX_DOMAIN_REGIONS;

/// How many `domainId`s this service remembers having allocated, beyond the domains it
/// currently holds.
///
/// A delete needs to tell an id this service once allocated — whose domain is simply
/// gone — from one that was never allocated to anybody, because the two answer
/// DIFFERENT errors (see `DeleteDomain`'s handler, which cites the recording for each).
/// The history is therefore kept past the domain's own purge, and BOUNDED here: a caller
/// looping create/delete would otherwise grow the store without limit, which is the one
/// thing every ceiling in this module exists to prevent.
///
/// The bound is the EMULATOR's retention, not a rule of the service: once the history is
/// full the oldest id is dropped, and a delete of that id then answers as it did before
/// the id was ever minted. It is set two orders of magnitude above
/// [`MAX_DOMAINS_PER_ACCOUNT`], so an account has to create and delete this many domains
/// before its first id is forgotten.
pub const MAX_DOMAIN_ALLOCATION_HISTORY: usize = 1024;

/// The most `Subscription` records one `Domain` holds, so the product
/// (domains x subscriptions) is bounded too.
pub const MAX_SUBSCRIPTIONS_PER_DOMAIN: usize = 100;

/// The most `PatientInsightsJob` records one `Domain` holds, so the product
/// (domains x jobs) is bounded as well: a caller looping `StartPatientInsightsJob`
/// cannot grow the shared store without limit. A job record stays reachable for as long
/// as its domain does, so nothing drops one behind a caller's back; past this count a
/// start is refused instead (see `StartPatientInsightsJob`).
pub const MAX_PATIENT_INSIGHTS_JOBS_PER_DOMAIN: usize = 100;

/// How long a patient insights job sits in ONE non-terminal `JobStatus` before the
/// transition the clock owes it is materialised (`SUBMITTED` -> `IN_PROGRESS`, then
/// `IN_PROGRESS` -> a terminal status).
///
/// AWS documents no duration for a patient insights job, so the span is CHOSEN, and
/// chosen inside the emulator's transient window (250-750 ms): a caller that reads
/// straight after `StartPatientInsightsJob` still observes `SUBMITTED` rather than a job
/// that was already finished by the time the start returned.
pub const PATIENT_INSIGHTS_JOB_STEP_SPAN: Duration = Duration::from_millis(400);

/// Why every patient insights job this emulator accepts ends in `FAILED`.
///
/// A patient insights job is an INFERENCE run: the real service reads the patient data
/// the request points at and generates clinical insights from it, which nothing inside
/// this emulator can do — there is no model to run and no S3 object to write. So the
/// job's control plane is real (the record, its id, ARN, timestamps and status
/// transitions are all stored and reported from state) while the run itself is reported
/// as the failure it is, in the service's own modelled failure status and with a
/// `statusDetails` that says so. Nothing is synthesised to look computed: a job that
/// generated no insights carries no `insightsOutput`.
pub const PATIENT_INSIGHTS_INFERENCE_UNAVAILABLE: &str =
    "Patient insights generation is not available in this emulator: no insights model ran \
     for this job, so no output was written to the configured output location.";

/// The most tag entries one resource ARN carries — AWS's standard tagging limit of 50,
/// which also bounds what one request can add to the tag store. `TagMap` itself declares
/// no `@length`, so this count is the SERVICE's own rule.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The MODELLED length bounds of one tag entry, both ends of each: `TagKey` is
/// `@length 1..128` and `TagValue` is `@length 0..256`
/// (`smithy_query(query="shape",shape="TagMap,TagKey,TagValue")`), which are also AWS's
/// standard tagging limits. Measured in CHARACTERS, as `@length` on a string is defined.
///
/// Stated here, enforced in [`crate::service::tag_bounds`] — the one check every
/// operation that accepts a `TagMap` runs, because a constraint on a map's key or value
/// is not one the framework's boundary walk refuses.
pub const MIN_TAG_KEY_LEN: usize = 1;
pub const MAX_TAG_KEY_LEN: usize = 128;
pub const MIN_TAG_VALUE_LEN: usize = 0;
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// A `Domain`'s lifecycle, as the model's `DomainStatus` enum declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainStatus {
    Active,
    Deleting,
    /// The TERMINAL state: the deletion has been applied. A domain reaches it only as
    /// the record is purged — either within the deleting call (when nothing outside
    /// this service had to be torn down) or when the `DELETING` span falls due — so a
    /// `DELETED` domain is gone rather than sitting in the store.
    Deleted,
}

impl DomainStatus {
    /// The wire value — one of `types::DOMAIN_STATUS_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
            Self::Deleted => "DELETED",
        }
    }

    /// The status a wire value names, or `None` when it names none.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ACTIVE" => Some(Self::Active),
            "DELETING" => Some(Self::Deleting),
            "DELETED" => Some(Self::Deleted),
            _ => None,
        }
    }
}

/// A `Subscription`'s lifecycle, as the model's `SubscriptionStatus` enum declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionStatus {
    Active,
    Inactive,
    Deleted,
}

impl SubscriptionStatus {
    /// The wire value — one of `types::SUBSCRIPTION_STATUS_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Inactive => "INACTIVE",
            Self::Deleted => "DELETED",
        }
    }
}

/// Which provisioning generation a `Domain` was onboarded through.
///
/// RECORDED behaviour, not modelled: no shape in this model declares an
/// `onboardingGeneration` member (`smithy_query(query="shape",
/// shape="CreateDomainOutput,GetDomainOutput")` lists neither), while every recorded
/// domain response carries the discriminator a client branches on to decide which
/// provisioning path a domain uses. It is a property of the domain, fixed when the
/// domain is created and stored on the record, never recomputed per read.
///
/// This emulator provisions a domain through exactly one path — the legacy web-app /
/// Identity Center onboarding every `CreateDomain` input in this model describes
/// (`webAppSetupConfiguration` naming an Identity Center instance) — so `LEGACY` is
/// the generation it records. The enum, rather than a bare string, is what keeps the
/// value a closed discriminator with room for a later generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingGeneration {
    Legacy,
}

impl OnboardingGeneration {
    /// The wire value.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Legacy => "LEGACY",
        }
    }
}

/// How a `Domain`'s data is encrypted, as the model's `EncryptionType` enum declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionType {
    AwsOwnedKey,
    CustomerManagedKey,
}

impl EncryptionType {
    /// The wire value — one of `types::ENCRYPTION_TYPE_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::AwsOwnedKey => "AWS_OWNED_KEY",
            Self::CustomerManagedKey => "CUSTOMER_MANAGED_KEY",
        }
    }
}

/// The stored web-application configuration of a `Domain` — the OUTPUT form
/// (`WebAppConfiguration`), derived once from the `CreateWebAppConfiguration` the
/// caller supplied: the Identity Center *instance* id it names is exchanged for the
/// Identity Center *application* id this service creates in that instance.
#[derive(Debug, Clone)]
pub struct WebAppConfigurationRecord {
    pub ehr_role: String,
    pub idc_application_id: String,
    pub idc_region: String,
}

/// A persisted `Domain`.
#[derive(Debug, Clone)]
pub struct Domain {
    pub domain_id: String,
    pub arn: String,
    /// The account that OWNS this domain — the account the create was signed for.
    /// Stored on the record, so every response projects the domain's own account
    /// rather than recomputing one per read.
    pub account_id: String,
    /// The REGION this domain lives in: the Region the create was signed for, and the
    /// region segment of [`Domain::arn`].
    ///
    /// This service's endpoint is REGIONAL (`smithy_query(query="service_info")` reports
    /// `endpointPrefix`/`arnNamespace` `health-agent` with standard regional endpoints,
    /// and every modelled ARN pattern carries a region segment), so a domain is part of
    /// exactly one Region's inventory. Every lookup in this module matches on it, so a
    /// resource created in one Region cannot be listed, read, mutated or deleted through
    /// another Region's endpoint, and the per-Region quotas count only that Region's
    /// records.
    pub region: String,
    /// The provisioning generation this domain was onboarded through.
    pub onboarding_generation: OnboardingGeneration,
    pub name: String,
    pub kms_key_arn: Option<String>,
    pub encryption_type: EncryptionType,
    pub status: DomainStatus,
    /// When the owed `DELETING` -> purged transition becomes due; `None` unless the
    /// domain is being deleted.
    pub purge_due_at: Option<SystemTime>,
    /// When this domain's deletion was RECORDED — the instant the call that deleted it
    /// was served; `None` while the domain is live. Written once, so a repeated delete
    /// reports the instant the first one recorded.
    pub deleted_at: Option<SystemTime>,
    pub web_app_url: Option<String>,
    pub web_app_configuration: Option<WebAppConfigurationRecord>,
    pub created_at: SystemTime,
    pub subscriptions: BTreeMap<String, Subscription>,
    /// The patient insights jobs started in this domain, by `jobId`. Held HERE rather
    /// than in a map beside the registry, so a job is scoped to its own domain and goes
    /// away with it.
    pub patient_insights_jobs: BTreeMap<String, PatientInsightsJob>,
    /// The `jobId` each `clientToken` a `StartPatientInsightsJob` carried is bound to, so
    /// a retried start with the same token resolves to the job the first one created
    /// instead of starting a second. Scoped to the domain, as the job it names is, and
    /// bounded with it: an entry is written only when a job is created.
    pub patient_insights_job_ids_by_client_token: BTreeMap<String, String>,
}

impl Domain {
    /// Whether deleting this domain finishes inside the deleting call.
    ///
    /// Deleting a domain tears down everything the service provisioned for it. What it
    /// has to wait for is the resource provisioned OUTSIDE this service: a domain with
    /// a web application owns an Identity Center application in another service, whose
    /// teardown outlives the call, so such a domain is reported in the non-terminal
    /// `DELETING` and the purge the clock owes applies the deletion afterwards. A domain
    /// with nothing provisioned outside this service holds only records of this store
    /// (its subscriptions and tags), which this call removes itself, so its deletion
    /// completes within the call and is reported in the terminal `DELETED`.
    pub fn deletion_completes_in_call(&self) -> bool {
        self.web_app_configuration.is_none()
    }
}

/// A patient insights job's lifecycle, as the model's `JobStatus` enum declares it
/// (`smithy_query(query="shape",shape="JobStatus")`: `SUBMITTED`, `IN_PROGRESS`,
/// `FAILED`, `SUCCEEDED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Submitted,
    InProgress,
    Failed,
    Succeeded,
}

impl JobStatus {
    /// The wire value — one of `types::JOB_STATUS_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Submitted => "SUBMITTED",
            Self::InProgress => "IN_PROGRESS",
            Self::Failed => "FAILED",
            Self::Succeeded => "SUCCEEDED",
        }
    }

    /// The status a wire value names, or `None` when it names none.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "SUBMITTED" => Some(Self::Submitted),
            "IN_PROGRESS" => Some(Self::InProgress),
            "FAILED" => Some(Self::Failed),
            "SUCCEEDED" => Some(Self::Succeeded),
            _ => None,
        }
    }
}

/// The job parameters a `StartPatientInsightsJob` request carries — every member of that
/// request except the `domainId` it is scoped by and the `clientToken` that identifies
/// the request rather than the job.
///
/// Stored once and rendered back by `GetPatientInsightsJob`, which reports each of them
/// as a required member: the reader projects the STORED parameters, so what a start
/// persisted is what a later get reads.
#[derive(Debug, Clone)]
pub struct PatientInsightsJobSpec {
    pub patient_context: PatientInsightsPatientContext,
    pub insights_context: InsightsContext,
    pub encounter_context: PatientInsightsEncounterContext,
    pub user_context: UserContext,
    pub input_data_config: InputDataConfig,
    pub output_data_config: OutputDataConfig,
}

impl PatientInsightsJobSpec {
    /// A digest of every parameter, so a repeated `clientToken` can be told from a token
    /// reused for DIFFERENT parameters without keeping a second copy of the request
    /// around.
    ///
    /// Every value is length-prefixed before it is folded in, so two different parameter
    /// sets cannot collide by running their text together, and an optional member
    /// contributes a part whether or not it was supplied.
    pub fn fingerprint(&self) -> String {
        let mut parts: Vec<String> = vec![
            self.patient_context.patient_id.clone(),
            self.patient_context.date_of_birth.clone().unwrap_or_default(),
            self.patient_context.pronouns.clone().unwrap_or_default(),
            self.insights_context.insights_type.clone(),
            self.encounter_context.encounter_reason.clone(),
            self.user_context.role.clone(),
            self.user_context.user_id.clone(),
            self.user_context.specialty.clone().unwrap_or_default(),
            self.output_data_config.s3_output_path.clone(),
        ];
        match &self.input_data_config.fhir_server {
            Some(server) => {
                parts.push("fhirServer".to_string());
                parts.push(server.fhir_endpoint.clone());
                parts.push(server.oauth_token.clone().unwrap_or_default());
            }
            None => parts.push(String::new()),
        }
        for source in self.input_data_config.s3_sources.iter().flatten() {
            parts.push(source.uri.clone());
        }
        let mut hasher = Sha256::new();
        for part in parts {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        hex::encode(hasher.finalize())
    }
}

/// A persisted patient insights job, stored under the `Domain` it was started in — which
/// is what SCOPES it: a job is reachable only through the `domainId` it belongs to, so
/// one domain's job cannot be read through another's, and a domain's jobs go away with
/// the domain.
#[derive(Debug, Clone)]
pub struct PatientInsightsJob {
    pub job_id: String,
    pub domain_id: String,
    pub arn: String,
    pub status: JobStatus,
    /// The text reported for the CURRENT status; `None` while there is nothing to say
    /// about it.
    pub status_details: Option<String>,
    /// Where the generated insights were written. `None` unless a run actually produced
    /// them — see [`PATIENT_INSIGHTS_INFERENCE_UNAVAILABLE`] for why no job this emulator
    /// runs ever does.
    pub insights_output: Option<InsightsOutput>,
    pub created_at: SystemTime,
    /// When the record last changed — the instant of its last status transition, which is
    /// the creation instant for a job that has not moved yet.
    pub updated_at: SystemTime,
    /// When the next status transition the clock owes becomes due; `None` once the job is
    /// in a terminal status and owes none.
    pub next_transition_at: Option<SystemTime>,
    /// The parameters the start persisted, reported back by the reader.
    pub spec: PatientInsightsJobSpec,
}

impl PatientInsightsJob {
    /// Apply every status transition the CLOCK has already made due.
    ///
    /// A non-terminal job owes exactly one transition at a time, one
    /// [`PATIENT_INSIGHTS_JOB_STEP_SPAN`] after the previous one: `SUBMITTED` ->
    /// `IN_PROGRESS`, then `IN_PROGRESS` -> `FAILED`, which is where a run this emulator
    /// cannot perform ends ([`PATIENT_INSIGHTS_INFERENCE_UNAVAILABLE`]). Each transition
    /// is stamped with the instant it FELL DUE rather than the instant somebody happened
    /// to look, so the record reads the same however late the first read comes.
    fn apply_due_transitions(&mut self, now: SystemTime) {
        while let Some(due) = self.next_transition_at.filter(|due| *due <= now) {
            match self.status {
                JobStatus::Submitted => {
                    self.status = JobStatus::InProgress;
                    self.next_transition_at = Some(due + PATIENT_INSIGHTS_JOB_STEP_SPAN);
                }
                JobStatus::InProgress => {
                    self.status = JobStatus::Failed;
                    self.status_details = Some(PATIENT_INSIGHTS_INFERENCE_UNAVAILABLE.to_string());
                    self.next_transition_at = None;
                }
                // A terminal status owes nothing; a due instant left on one is dropped
                // rather than acted on.
                JobStatus::Failed | JobStatus::Succeeded => self.next_transition_at = None,
            }
            self.updated_at = due;
        }
    }
}

/// A persisted `Subscription`, stored under its own `Domain`.
///
/// `activated_at` / `deactivated_at` are a HISTORY of the explicit
/// `ActivateSubscription` / `DeactivateSubscription` transitions, not a view of
/// `status`: each is stamped when its transition happens and is never cleared, so both
/// boundaries of a billing interval remain readable after the opposite transition. A
/// freshly created subscription has made neither transition, so it carries neither.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub domain_id: String,
    pub subscription_id: String,
    pub arn: String,
    pub status: SubscriptionStatus,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    pub activated_at: Option<SystemTime>,
    pub deactivated_at: Option<SystemTime>,
    /// The Medical Scribe listening sessions started on this subscription, by
    /// `sessionId`. Held HERE rather than in a map beside the registry, so a session is
    /// scoped to its own subscription and goes away with it. See [`ScribeSession`] for
    /// why nothing in this crate writes it yet.
    pub scribe_sessions: BTreeMap<String, ScribeSession>,
}

/// A Medical Scribe listening session's stream status, as the model's
/// `MedicalScribeStreamStatus` enum declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScribeStreamStatus {
    InProgress,
    Paused,
    Failed,
    Completed,
}

impl ScribeStreamStatus {
    /// The wire value — one of `types::MEDICAL_SCRIBE_STREAM_STATUS_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Paused => "PAUSED",
            Self::Failed => "FAILED",
            Self::Completed => "COMPLETED",
        }
    }
}

/// A persisted Medical Scribe listening session, stored under the `Subscription` it was
/// started on — which is what SCOPES it: a session is reachable only through the
/// (`domainId`, `subscriptionId`) pair it belongs to, so one subscription's session
/// cannot be read through another's (see [`ComAmazonawsConnecthealthConnecthealthState::live_scribe_session`]).
///
/// A session is written by the operation that starts one,
/// `StartMedicalScribeListeningSession`, which this emulator does not serve: the
/// generator skips it (`src/ops/SKIPPED_STUBS.md` — its input carries a `@streaming`
/// event-stream member, which needs wire framing the generator does not emit), so this
/// crate has no handler for it and no growth site to bound. `GetMedicalScribeListeningSession`
/// nevertheless resolves a session out of this map rather than answering a constant, so
/// a stored session is reported from state and an absent one is the modelled 404.
#[derive(Debug, Clone)]
pub struct ScribeSession {
    pub session_id: String,
    pub domain_id: String,
    pub subscription_id: String,
    pub language_code: String,
    pub media_sample_rate_hertz: i32,
    pub media_encoding: String,
    pub channel_definitions: Vec<MedicalScribeChannelDefinition>,
    pub post_stream_action_settings: Option<MedicalScribePostStreamActionSettingsResponse>,
    pub post_stream_action_result: Option<MedicalScribePostStreamActionsResult>,
    pub encounter_context_provided: bool,
    pub stream_status: ScribeStreamStatus,
    pub stream_creation_time: SystemTime,
    /// When the stream ENDED; `None` while it is still running, so a session in a
    /// non-terminal status carries no end instant.
    pub stream_end_time: Option<SystemTime>,
}

impl ScribeSession {
    /// This record as the modelled `MedicalScribeListeningSessionDetails`.
    pub fn details(&self) -> MedicalScribeListeningSessionDetails {
        MedicalScribeListeningSessionDetails {
            session_id: Some(self.session_id.clone()),
            domain_id: Some(self.domain_id.clone()),
            subscription_id: Some(self.subscription_id.clone()),
            language_code: Some(self.language_code.clone()),
            media_sample_rate_hertz: Some(self.media_sample_rate_hertz),
            media_encoding: Some(self.media_encoding.clone()),
            channel_definitions: Some(self.channel_definitions.clone()),
            post_stream_action_settings: self.post_stream_action_settings.clone(),
            post_stream_action_result: self.post_stream_action_result.clone(),
            encounter_context_provided: Some(self.encounter_context_provided),
            stream_status: Some(self.stream_status.as_wire().to_string()),
            stream_creation_time: Some(to_timestamp(self.stream_creation_time)),
            stream_end_time: self.stream_end_time.map(to_timestamp),
        }
    }
}

impl Subscription {
    /// Whether this record is still live: one `DeleteDomain` already marked `DELETED`
    /// is gone as far as every reader and every transition is concerned.
    pub fn is_live(&self) -> bool {
        self.status != SubscriptionStatus::Deleted
    }

    /// This record as the modelled `SubscriptionDescription` four operations return.
    pub fn description(&self) -> SubscriptionDescription {
        SubscriptionDescription {
            domain_id: self.domain_id.clone(),
            subscription_id: self.subscription_id.clone(),
            arn: self.arn.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_timestamp(self.created_at),
            last_updated_at: to_timestamp(self.last_updated_at),
            activated_at: self.activated_at.map(to_timestamp),
            deactivated_at: self.deactivated_at.map(to_timestamp),
        }
    }
}

/// Per-service state for com-amazonaws-connecthealth-connecthealth. A handler reaches it through [`service_state`].
///
/// `BTreeMap` throughout, so `ListDomains` / `ListSubscriptions` walk a deterministic,
/// key-ordered sequence and a `nextToken` naming the last key returned is a stable
/// cursor into it.
#[derive(Debug, Default)]
pub struct ComAmazonawsConnecthealthConnecthealthState {
    /// Every live `Domain`, keyed by `domainId` (ids are minted unique across the whole
    /// store, so one key never names two domains). The Region each one belongs to is on
    /// the record (`Domain::region`) and every lookup matches it, so this one map holds a
    /// separate inventory per Region.
    pub domains: BTreeMap<String, Domain>,
    /// The ONE tag store, keyed by resource ARN so domain and subscription ARNs both
    /// work. `CreateDomain` seeds it and `TagResource` merges into it;
    /// `UntagResource`/`ListTagsForResource` (still scaffolded) share it.
    pub tags_by_arn: BTreeMap<String, BTreeMap<String, String>>,
    /// Every `domainId` this service has allocated and still remembers, whether or not
    /// the domain itself is still held — the ALLOCATION HISTORY a delete consults to
    /// tell "this domain is gone" from "this id was never valid anywhere".
    pub allocated_domain_ids: BTreeSet<String>,
    /// The same ids in allocation order, so the bound in
    /// [`MAX_DOMAIN_ALLOCATION_HISTORY`] drops the oldest first.
    pub allocation_order: VecDeque<String>,
    /// Monotonic per-process sequence folded into every minted id, so two ids minted
    /// in the same nanosecond still differ.
    pub next_id_seq: u64,
}

impl ComAmazonawsConnecthealthConnecthealthState {
    /// The next value of the id sequence.
    fn next_seq(&mut self) -> u64 {
        let seq = self.next_id_seq;
        self.next_id_seq = self.next_id_seq.wrapping_add(1);
        seq
    }

    /// Mint an unused `domainId`: `dom-` plus 16 lowercase hex characters, which is 20
    /// characters long and matches `^(hai-|dom-)[a-z0-9]+$` (len 20..25).
    ///
    /// Unused means unused by a domain this store holds AND by one it remembers
    /// allocating, so an id never names two domains and the allocation history cannot
    /// be made ambiguous by a re-mint.
    pub fn mint_domain_id(&mut self) -> String {
        loop {
            let seq = self.next_seq();
            let candidate = mint_id("dom-", 16, seq);
            if !self.domains.contains_key(&candidate)
                && !self.allocated_domain_ids.contains(&candidate)
            {
                return candidate;
            }
        }
    }

    /// How many `Domain` records `account_id` currently holds IN `region` — the LIVE
    /// count the account's per-Region cap is measured against, which a purge of a deleted
    /// domain lowers again. Every record carries its owning account and its Region, so
    /// the count is of that account's domains in that Region alone and a domain in
    /// another Region consumes nothing here.
    pub fn domains_held_by(&self, region: &str, account_id: &str) -> usize {
        self.domains_in(region)
            .filter(|domain| domain.account_id == account_id)
            .count()
    }

    /// Every `Domain` record `region` holds, whatever its status, in `domainId` order.
    ///
    /// The ONE place the Region scope of this store is expressed: a reader that
    /// enumerates domains goes through here, so an enumeration and a point lookup
    /// ([`Self::held_domain`]) can never disagree about which Region holds what.
    pub fn domains_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a Domain> + 'a {
        self.domains
            .values()
            .filter(move |domain| domain.region == region)
    }

    /// Remember that `domain_id` was allocated, dropping the oldest remembered id once
    /// the history is at [`MAX_DOMAIN_ALLOCATION_HISTORY`].
    pub fn record_domain_allocation(&mut self, domain_id: &str) {
        if !self.allocated_domain_ids.insert(domain_id.to_string()) {
            return;
        }
        self.allocation_order.push_back(domain_id.to_string());
        while self.allocation_order.len() > MAX_DOMAIN_ALLOCATION_HISTORY {
            if let Some(evicted) = self.allocation_order.pop_front() {
                self.allocated_domain_ids.remove(&evicted);
            }
        }
    }

    /// Whether `domain_id` was ever allocated to a domain this service created.
    pub fn has_allocated_domain(&self, domain_id: &str) -> bool {
        self.allocated_domain_ids.contains(domain_id)
    }

    /// Mint a `subscriptionId` unused within `domain_id`: `sub-` plus 21 hex
    /// characters, which is exactly 25 characters and matches `^sub-[a-zA-Z0-9]{21}$`.
    pub fn mint_subscription_id(&mut self, domain_id: &str) -> String {
        loop {
            let seq = self.next_seq();
            let candidate = mint_id("sub-", 21, seq);
            let taken = self
                .domains
                .get(domain_id)
                .is_some_and(|domain| domain.subscriptions.contains_key(&candidate));
            if !taken {
                return candidate;
            }
        }
    }

    /// Mint a `jobId` unused within `domain_id`: 32 hex characters, which is inside the
    /// modelled `JobId` bound of 1..36 and inside the `[-a-zA-Z0-9_/.]+` segment the
    /// `JobArn` pattern allows for a job.
    pub fn mint_patient_insights_job_id(&mut self, domain_id: &str) -> String {
        loop {
            let seq = self.next_seq();
            let candidate = mint_id("", 32, seq);
            let taken = self
                .domains
                .get(domain_id)
                .is_some_and(|domain| domain.patient_insights_jobs.contains_key(&candidate));
            if !taken {
                return candidate;
            }
        }
    }

    /// Mint the Identity Center application id this service creates for a web app.
    pub fn mint_idc_application_id(&mut self) -> String {
        let seq = self.next_seq();
        mint_id("apl-", 16, seq)
    }

    /// Materialise every deletion the CLOCK has already made due: a `Domain` whose
    /// `DELETING` span has elapsed is purged, with its subscriptions and every tag
    /// entry keyed by one of their ARNs.
    ///
    /// Called at the top of every handler, including the reporting ones: it writes only
    /// what was owed at this instant whether or not anybody called.
    pub fn purge_due_deletions(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .domains
            .values()
            .filter(|domain| domain.purge_due_at.is_some_and(|due| due <= now))
            .map(|domain| domain.domain_id.clone())
            .collect();
        for domain_id in due {
            if let Some(domain) = self.domains.remove(&domain_id) {
                self.tags_by_arn.remove(&domain.arn);
                for subscription in domain.subscriptions.values() {
                    self.tags_by_arn.remove(&subscription.arn);
                }
            }
        }
    }

    /// Materialise every patient-insights-job status transition the CLOCK has already
    /// made due, in every domain.
    ///
    /// Called at the top of every handler beside [`Self::purge_due_deletions`], including
    /// the reporting ones: it writes only what was owed at this instant whether or not
    /// anybody called, so a job observed late reads exactly as it would have at the
    /// moment its transition fell due.
    pub fn advance_due_patient_insights_jobs(&mut self, now: SystemTime) {
        for domain in self.domains.values_mut() {
            for job in domain.patient_insights_jobs.values_mut() {
                job.apply_due_transitions(now);
            }
        }
    }

    /// The patient insights job `job_id` names under EXACTLY the domain `domain_id`
    /// names.
    ///
    /// `None` when either link of that chain is missing — the domain is absent or no
    /// longer live, or it holds no job of that id — so a job started in one domain is
    /// never reachable through another, and both misses are the one answer
    /// `GetPatientInsightsJob` gives: `ResourceNotFoundException`.
    pub fn patient_insights_job(
        &self,
        region: &str,
        domain_id: &str,
        job_id: &str,
    ) -> Option<&PatientInsightsJob> {
        self.live_domain(region, domain_id)?
            .patient_insights_jobs
            .get(job_id)
    }

    /// The tags recorded for `arn`, in key order.
    pub fn tags_for(&self, arn: &str) -> BTreeMap<String, String> {
        self.tags_by_arn.get(arn).cloned().unwrap_or_default()
    }

    /// The tags a RESPONSE reports for `arn`: always a map, EMPTY when the resource
    /// carries none.
    ///
    /// A modelled tag map is a map of the resource's tags, and a resource with no tags
    /// has an empty one — an absent member says "unknown" instead, so a client
    /// iterating the member would have to tell the two apart. Every response that
    /// reports a resource's tags projects them through here.
    pub fn reported_tags(&self, arn: &str) -> std::collections::HashMap<String, String> {
        self.tags_for(arn).into_iter().collect()
    }

    /// Whether `arn` names a resource this service currently holds IN `region`: the ARN
    /// of a `Domain` record this Region still holds, or of a live `Subscription` under
    /// one.
    ///
    /// Holding a record is what makes a resource addressable, NOT the lifecycle status
    /// that record reports: a `Domain` whose deletion is in flight is a resource the
    /// service still holds — its own reader answers it ([`Self::held_domain`] is what
    /// `GetDomain` looks it up through, and `ListDomains` lists it under its non-terminal
    /// status) — so its ARN resolves for exactly as long as the record does. The one
    /// place an ARN stops resolving is [`Self::purge_due_deletions`], which drops the
    /// record together with the tag entries keyed by its ARN. An ARN this resolves is
    /// therefore always one some reader resolves too, and the ARN-addressed operations
    /// and the resource's own reader never disagree about whether it exists.
    ///
    /// The Region scope is the readers' as well ([`Self::domains_in`]): an ARN rooted in
    /// another Region resolves nowhere here, exactly as the domain it names resolves
    /// through no other Region's endpoint.
    ///
    /// A `Subscription` is resolved through its own reader's liveness
    /// ([`Self::live_subscription`]): a subscription already `DELETED` is gone for that
    /// reader and gone here, which covers every subscription of a domain being deleted —
    /// `DeleteDomain` marks them all `DELETED` as it records the deletion.
    pub fn holds_resource_arn(&self, region: &str, arn: &str) -> bool {
        self.domains_in(region).any(|domain| {
            domain.arn == arn
                || (domain.status == DomainStatus::Active
                    && domain
                        .subscriptions
                        .values()
                        .any(|subscription| subscription.is_live() && subscription.arn == arn))
        })
    }

    /// The tag set `arn` would carry once `tags` were merged into it, without writing
    /// anything: the entries already recorded, with every key `tags` names taking the
    /// value `tags` gives it.
    ///
    /// Separate from [`Self::merge_tags`] so a caller can bound the RESULT of a merge —
    /// the limit is on the resource's tag set, not on one request — and refuse before
    /// the store has been touched.
    pub fn tags_after_merge(
        &self,
        arn: &str,
        tags: impl IntoIterator<Item = (String, String)>,
    ) -> BTreeMap<String, String> {
        let mut merged = self.tags_for(arn);
        merged.extend(tags);
        merged
    }

    /// Record `tags` as the tag set of `arn`.
    ///
    /// The caller passes the already-merged set ([`Self::tags_after_merge`]). An EMPTY
    /// set is stored as no entry at all, so "no tags" has one representation in the
    /// store whether the resource was never tagged or had every tag removed.
    pub fn set_tags(&mut self, arn: &str, tags: BTreeMap<String, String>) {
        if tags.is_empty() {
            self.tags_by_arn.remove(arn);
        } else {
            self.tags_by_arn.insert(arn.to_string(), tags);
        }
    }

    /// The live domain `domain_id` names IN `region`, for reading.
    ///
    /// `None` when `region` holds no such domain (it is absent, or it belongs to another
    /// Region) or when the one it holds is no longer `ACTIVE` — the cases every operation
    /// on a domain's subscriptions answers with `ResourceNotFoundException`. ONE liveness
    /// rule and ONE Region scope, so a subscription a caller can enumerate is one it can
    /// also read.
    pub fn live_domain(&self, region: &str, domain_id: &str) -> Option<&Domain> {
        self.held_domain(region, domain_id)
            .filter(|domain| domain.status == DomainStatus::Active)
    }

    /// The live domain `domain_id` names in `region`, for writing. Liveness is exactly
    /// [`Self::live_domain`]'s.
    pub fn live_domain_mut(&mut self, region: &str, domain_id: &str) -> Option<&mut Domain> {
        self.held_domain_mut(region, domain_id)
            .filter(|domain| domain.status == DomainStatus::Active)
    }

    /// The domain `region` holds under `domain_id`, WHATEVER its status — the reader a
    /// reporting operation uses when a non-`ACTIVE` domain is still something it reports
    /// (`GetDomain` on a domain that is `DELETING`).
    ///
    /// `None` when this Region holds no such domain, whether because the id names
    /// nothing at all or because the domain it names belongs to ANOTHER Region: a
    /// regional endpoint sees only its own Region's inventory, so both are the same miss
    /// and both answer `ResourceNotFoundException`.
    pub fn held_domain(&self, region: &str, domain_id: &str) -> Option<&Domain> {
        self.domains
            .get(domain_id)
            .filter(|domain| domain.region == region)
    }

    /// The domain `region` holds under `domain_id`, for writing. The scope is exactly
    /// [`Self::held_domain`]'s.
    pub fn held_domain_mut(&mut self, region: &str, domain_id: &str) -> Option<&mut Domain> {
        self.domains
            .get_mut(domain_id)
            .filter(|domain| domain.region == region)
    }

    /// The live subscription `subscription_id` carries under `domain_id`, for reading.
    ///
    /// `None` when the domain is not live, or when the subscription is absent or
    /// already `DELETED` — the cases every operation on a subscription answers with
    /// `ResourceNotFoundException`.
    pub fn live_subscription(
        &self,
        region: &str,
        domain_id: &str,
        subscription_id: &str,
    ) -> Option<&Subscription> {
        self.live_domain(region, domain_id)?
            .subscriptions
            .get(subscription_id)
            .filter(|subscription| subscription.is_live())
    }

    /// The Medical Scribe listening session `session_id` names under EXACTLY this
    /// (`domain_id`, `subscription_id`) scope.
    ///
    /// `None` when any link of that chain is missing — the domain, the subscription, or
    /// the session under that subscription — so a session started on one subscription is
    /// never reachable through another, and every miss is the one answer
    /// `GetMedicalScribeListeningSession` gives: `ResourceNotFoundException`.
    pub fn live_scribe_session(
        &self,
        region: &str,
        domain_id: &str,
        subscription_id: &str,
        session_id: &str,
    ) -> Option<&ScribeSession> {
        self.live_subscription(region, domain_id, subscription_id)?
            .scribe_sessions
            .get(session_id)
    }

    /// The live subscription `subscription_id` carries under `domain_id`, for writing.
    /// Liveness is exactly [`Self::live_subscription`]'s.
    pub fn live_subscription_mut(
        &mut self,
        region: &str,
        domain_id: &str,
        subscription_id: &str,
    ) -> Option<&mut Subscription> {
        self.live_domain_mut(region, domain_id)?
            .subscriptions
            .get_mut(subscription_id)
            .filter(|subscription| subscription.is_live())
    }
}

/// Mint an id: `prefix` followed by `body_len` characters of a SHA-256 digest over the
/// prefix, the caller's sequence number, this process and the current instant.
///
/// Hex digits are lowercase alphanumerics, so one minting rule satisfies every id
/// pattern this service declares, and folding in both the sequence and the clock keeps
/// ids unique within a process and across processes.
fn mint_id(prefix: &str, body_len: usize, seq: u64) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    hasher.update(seq.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    hasher.update(std::process::id().to_be_bytes());
    let body = hex::encode(hasher.finalize());
    format!("{prefix}{}", &body[..body_len])
}

/// The ARN of a `Domain`:
/// `arn:aws:health-agent:<region>:<account>:domain/<domainId>`.
pub fn domain_arn(region: &str, domain_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("domain/{domain_id}"))
}

/// The account every resource of this emulator belongs to: the canonical emulator
/// account the vendored runtime owns, which is also the account segment of every ARN
/// [`domain_arn`] builds. A domain's owning account is stored on its record at creation
/// (`Domain::account_id`), so the responses project the domain's own account.
pub fn owning_account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// One member the RECORDED service carries beside a modelled response, with the member
/// schema it is written under.
///
/// None of these members is in the model (`smithy_query(query="shape",
/// shape="CreateDomainOutput,GetDomainOutput,DeleteDomainOutput")` lists neither
/// `accountId`, `onboardingGeneration` nor `deletedAt`), so no generated field exists
/// to fill: [`RecordedMembers`] writes them after the modelled payload's own members,
/// each under a member schema the answering operation declares. The modelled shapes and
/// the model file stay exactly as shipped, and a client that knows only the modelled
/// shape ignores the extra keys.
#[derive(Debug)]
pub enum RecordedMember {
    /// A string-valued member.
    Text(&'static ::aws_smithy_schema::Schema, String),
    /// A timestamp-valued member, serialized in the protocol's timestamp format.
    Instant(
        &'static ::aws_smithy_schema::Schema,
        ::aws_smithy_types::DateTime,
    ),
}

/// A modelled response plus the members the recorded service carries beside it.
#[derive(Debug)]
pub struct RecordedMembers<T> {
    /// The modelled response — the only part an SDK-generated client reads.
    pub payload: T,
    /// The recorded-only members, written in order after the modelled ones.
    pub members: Vec<RecordedMember>,
}

impl<T: ::aws_smithy_schema::serde::SerializableStruct>
    ::aws_smithy_schema::serde::SerializableStruct for RecordedMembers<T>
{
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> Result<(), ::aws_smithy_schema::serde::SerdeError> {
        self.payload.serialize_members(serializer)?;
        for member in &self.members {
            match member {
                RecordedMember::Text(schema, value) => serializer.write_string(schema, value)?,
                RecordedMember::Instant(schema, value) => {
                    serializer.write_timestamp(schema, value)?
                }
            }
        }
        Ok(())
    }
}

/// The ARN of a `Subscription`: its domain's ARN plus `/subscription/<subscriptionId>`.
pub fn subscription_arn(domain_arn: &str, subscription_id: &str) -> String {
    format!("{domain_arn}/subscription/{subscription_id}")
}

/// The ARN of a patient insights job: its domain's ARN plus
/// `/patient-insights-job/<jobId>`, which is the resource form the modelled `JobArn`
/// pattern declares (`smithy_query(query="shape",shape="JobArn")`:
/// `^arn:aws[-a-z]*:health-agent:[-a-z0-9]+:[0-9]{12}:domain/[-a-zA-Z0-9-]+/patient-insights-job/[-a-zA-Z0-9_/.]+$`).
pub fn patient_insights_job_arn(domain_arn: &str, job_id: &str) -> String {
    format!("{domain_arn}/patient-insights-job/{job_id}")
}

/// The web-app URL of a `Domain`, derived from the domain's own id and region.
pub fn web_app_url(region: &str, domain_id: &str) -> String {
    format!("https://{domain_id}.{ARN_NAMESPACE}.{region}.on.aws")
}

/// The longest credential-scope region field this service files a record under.
///
/// Every region code AWS publishes is far shorter; the bound exists because the field is
/// read off the request, so without it one request could file a record under a key of
/// arbitrary size. A longer field names no region this service serves, and is treated
/// exactly like a scope that names none at all.
const MAX_REGION_LEN: usize = 32;

/// The region this request was signed for, or [`DEFAULT_REGION`] when it names none.
///
/// Constrained to the `[a-z0-9-]+` segment every ARN pattern in this model declares and
/// to [`MAX_REGION_LEN`], so a request signed for a nonsense region cannot mint an ARN
/// that fails its own pattern, nor file a record under a key of its own choosing.
pub fn region_of(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= MAX_REGION_LEN
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A stored instant as the modelled timestamp type.
pub fn to_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The opaque `nextToken` for a page that stopped at `last_key`.
///
/// Hex of the key, so the token is opaque to a client yet resumes EXACTLY where the
/// page stopped: the next page starts at the first key strictly after it, whether or
/// not that key still exists.
pub fn encode_page_token(last_key: &str) -> String {
    hex::encode(last_key.as_bytes())
}

/// The key a `nextToken` resumes after, or `None` when the token is not one this
/// service issued.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConnecthealthConnecthealthState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConnecthealthConnecthealthState>)
}

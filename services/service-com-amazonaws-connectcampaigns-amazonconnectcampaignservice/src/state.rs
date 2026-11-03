// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connectcampaigns-amazonconnectcampaignservice service.

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::{
    DialerConfig, EncryptionConfig, InstanceOnboardingJobStatus, OutboundCallConfig,
};

/// This service's sigv4 signing name and ARN namespace (see `OPERATIONS.md`), used
/// for every ARN the service mints.
pub const ARN_NAMESPACE: &str = "connect-campaigns";

/// The region to stamp into an ARN when the request carries no sigv4 credential
/// scope to read one from (an unsigned, host-routed request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many campaigns one Amazon Connect instance may hold. AWS publishes a
/// per-instance campaign quota for Amazon Connect outbound campaigns (adjustable, 100
/// by default), and `CreateCampaign` models `ServiceQuotaExceededException` (HTTP 402)
/// for exceeding it, so the store is capped rather than grown without limit.
pub const CAMPAIGNS_PER_INSTANCE_QUOTA: usize = 100;

/// How many campaigns this emulator's store holds across every instance. AWS publishes
/// no such account-wide number: this is the emulator's own memory bound, because the
/// store lives in memory for the life of the process and only `DeleteCampaign` releases
/// an entry. Past it `CreateCampaign` answers the modelled
/// `ServiceQuotaExceededException` — the nearest modelled refusal — with a message that
/// says it is this store's limit rather than an AWS quota.
pub const CAMPAIGN_STORE_CAPACITY: usize = 10_000;

/// The states every [`CampaignState`] renders as, in the order the model lists them.
/// Kept beside the enum so a test can check it against the model's own
/// `CAMPAIGN_STATE_VALUES`.
pub const ALL_CAMPAIGN_STATES: &[CampaignState] = &[
    CampaignState::Initialized,
    CampaignState::Running,
    CampaignState::Paused,
    CampaignState::Stopped,
    CampaignState::Failed,
];

/// The run state of a campaign: the `CampaignState` enum of the Smithy model
/// (`Failed | Initialized | Paused | Running | Stopped`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampaignState {
    Failed,
    Initialized,
    Paused,
    Running,
    Stopped,
}

impl CampaignState {
    /// The value this state takes on the wire — the model's enum value, so a
    /// response member and an `InvalidCampaignStateException.state` agree.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Failed => "Failed",
            Self::Initialized => "Initialized",
            Self::Paused => "Paused",
            Self::Running => "Running",
            Self::Stopped => "Stopped",
        }
    }
}

/// A stored campaign: every member of the modelled `Campaign` shape plus the run
/// state the start/pause/stop operations move and `GetCampaignState` reports.
#[derive(Debug, Clone)]
pub struct StoredCampaign {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub connect_instance_id: String,
    pub dialer_config: DialerConfig,
    pub outbound_call_config: OutboundCallConfig,
    pub tags: HashMap<String, String>,
    pub state: CampaignState,
    /// When `CreateCampaign` stored it. No operation of this slice reports a creation
    /// time (the modelled `Campaign` shape carries none); it is kept because the
    /// instance-onboarding and update operations that are still 501 report one, and
    /// recording it at creation is the only moment it can be recorded.
    pub created_at: SystemTime,
    /// The store's creation counter at the moment this campaign was stored: the order
    /// `ListCampaigns` pages over. `created_at` cannot serve as that key, because its
    /// resolution is coarser than two consecutive creates.
    ///
    /// Assigned by [`ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState::insert_campaign`];
    /// whatever a caller puts here is overwritten.
    pub sequence: u64,
}

/// Per-service state for com-amazonaws-connectcampaigns-amazonconnectcampaignservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState {
    /// Every campaign, keyed by its minted campaign id.
    pub campaigns: HashMap<String, StoredCampaign>,
    /// The onboarding job of each Amazon Connect instance this service has onboarded to
    /// Campaigns, keyed by that instance's id — one record per instance, written by
    /// `StartInstanceOnboardingJob` and bounded at [`MAX_ONBOARDED_INSTANCES`] by
    /// [`Self::start_instance_onboarding`].
    ///
    /// The one record of which instances are onboarded and of what their campaign data
    /// is encrypted with, so `GetInstanceOnboardingJobStatus` and every later reader of
    /// an instance's configuration cannot disagree about an instance.
    pub instance_onboarding: HashMap<String, StoredInstanceOnboarding>,
    /// Campaign ARN -> campaign id, so an ARN-keyed lookup resolves without scanning
    /// `campaigns`. Maintained by [`Self::insert_campaign`] and [`Self::remove_campaign`]
    /// — the only two writers of `campaigns` — so it cannot drift from them; its first
    /// readers are the tag operations (`TagResource`, `UntagResource`,
    /// `ListTagsForResource`), which take an ARN and are still 501.
    pub arn_index: HashMap<String, String>,
    /// Creation counter: the `sequence` the next stored campaign takes.
    pub next_sequence: u64,
}

impl ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState {
    /// Store a campaign and index its ARN, stamping it with the next `sequence`.
    /// Returns the campaign as stored.
    pub fn insert_campaign(&mut self, mut campaign: StoredCampaign) -> StoredCampaign {
        campaign.sequence = self.next_sequence;
        self.next_sequence += 1;
        self.arn_index
            .insert(campaign.arn.clone(), campaign.id.clone());
        self.campaigns.insert(campaign.id.clone(), campaign.clone());
        campaign
    }

    /// How many campaigns the named Amazon Connect instance holds — what
    /// [`CAMPAIGNS_PER_INSTANCE_QUOTA`] caps.
    pub fn campaigns_for_instance(&self, connect_instance_id: &str) -> usize {
        self.campaigns
            .values()
            .filter(|campaign| campaign.connect_instance_id == connect_instance_id)
            .count()
    }

    /// Drop a campaign and its ARN index entry. `None` when no such campaign.
    pub fn remove_campaign(&mut self, id: &str) -> Option<StoredCampaign> {
        let removed = self.campaigns.remove(id)?;
        self.arn_index.remove(&removed.arn);
        Some(removed)
    }

    /// Move one campaign's run state from any of `allowed_from` to `to`, returning the
    /// state it now holds.
    ///
    /// The one place a run-state transition is applied, so start, pause and stop cannot
    /// disagree about what a transition does: each names only the states it may move
    /// from. A campaign already in `to` is NOT a special case — a request to enter the
    /// state it is already in is refused unless `to` is itself in `allowed_from`.
    pub fn transition_campaign(
        &mut self,
        id: &str,
        allowed_from: &[CampaignState],
        to: CampaignState,
    ) -> Result<CampaignState, TransitionRefused> {
        let campaign = self
            .campaigns
            .get_mut(id)
            .ok_or(TransitionRefused::NoSuchCampaign)?;
        if !allowed_from.contains(&campaign.state) {
            return Err(TransitionRefused::InvalidState(campaign.state));
        }
        campaign.state = to;
        Ok(to)
    }

    /// Start the onboarding job of `connect_instance_id`, persisting `encryption_config`
    /// as that instance's, or report why no job was started.
    ///
    /// The one place this store takes an onboarding record, so the two refusals an
    /// onboarding call owes are decided once rather than per handler: an instance this
    /// service already holds a job for is not onboarded a second time, and the store is
    /// bounded. Nothing is written on either refusal.
    pub fn start_instance_onboarding(
        &mut self,
        connect_instance_id: &str,
        encryption_config: EncryptionConfig,
        now: SystemTime,
    ) -> Result<StoredInstanceOnboarding, OnboardingRefused> {
        if self.instance_onboarding.contains_key(connect_instance_id) {
            return Err(OnboardingRefused::AlreadyOnboarded);
        }
        if self.instance_onboarding.len() >= MAX_ONBOARDED_INSTANCES {
            return Err(OnboardingRefused::StoreFull);
        }
        let record = StoredInstanceOnboarding::starting(
            connect_instance_id.to_string(),
            encryption_config,
            now,
        );
        self.instance_onboarding
            .insert(record.connect_instance_id.clone(), record.clone());
        Ok(record)
    }

    /// The onboarding record this service holds for `connect_instance_id`, or `None`
    /// when no call has onboarded that instance.
    ///
    /// The one lookup of that record, so a reporting operation reads what
    /// [`Self::start_instance_onboarding`] wrote and nothing else.
    pub fn instance_onboarding(
        &self,
        connect_instance_id: &str,
    ) -> Option<&StoredInstanceOnboarding> {
        self.instance_onboarding.get(connect_instance_id)
    }

    /// Every campaign in creation order — the order `ListCampaigns` pages over.
    pub fn campaigns_in_creation_order(&self) -> Vec<&StoredCampaign> {
        let mut all: Vec<&StoredCampaign> = self.campaigns.values().collect();
        // `sequence` is the creation counter, so it orders creations exactly — unlike
        // `created_at`, whose resolution is coarser than two consecutive creates.
        all.sort_by(|a, b| a.sequence.cmp(&b.sequence).then_with(|| a.id.cmp(&b.id)));
        all
    }
}

/// Why a requested run-state transition did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionRefused {
    /// No campaign with that id is stored (`ResourceNotFoundException`, HTTP 404).
    NoSuchCampaign,
    /// The campaign is in a state this transition cannot be made from
    /// (`InvalidCampaignStateException`, HTTP 409), carrying the state it is in.
    InvalidState(CampaignState),
}

/// The `InstanceOnboardingJobStatusCode` value an onboarding job that has not finished
/// yet reports, as the model's catalog declares it (see
/// `INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES` in [`crate::ops::types`]).
pub const ONBOARDING_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `InstanceOnboardingJobStatusCode` value a finished onboarding job reports, from
/// the same catalog.
pub const ONBOARDING_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// How long after the `StartInstanceOnboardingJob` call that started it an onboarding
/// job is due to report [`ONBOARDING_STATUS_SUCCEEDED`].
///
/// AWS documents no duration for the job, so this span is the emulator's own: long
/// enough that a caller which polls straight after the start call still observes the
/// transient status a real job has, short enough that a poller reaches the finished one
/// without waiting on anything. Nothing sleeps for it — the record carries the instant
/// the job is due and [`StoredInstanceOnboarding::status_at`] reads the status off the
/// clock.
pub const ONBOARDING_JOB_DURATION: ::std::time::Duration = ::std::time::Duration::from_secs(3);

/// The most Amazon Connect instances this emulator's store holds onboarding records
/// for, after which `StartInstanceOnboardingJob` refuses to take another.
///
/// The store grows one record per newly onboarded instance and the instance id is the
/// caller's, so without a ceiling a caller could repeat the call under fresh ids until
/// the process runs out of memory. AWS publishes no such limit, so this is the
/// emulator's own, set far above any plausible workload.
pub const MAX_ONBOARDED_INSTANCES: usize = 1_000;

/// The onboarding job of one Amazon Connect instance: the encryption config the
/// `StartInstanceOnboardingJob` call persisted for it, and the instant that job is due
/// to have finished.
#[derive(Debug, Clone)]
pub struct StoredInstanceOnboarding {
    pub connect_instance_id: String,
    /// What this instance's campaign data is encrypted with, exactly as the call that
    /// started the job supplied it.
    pub encryption_config: EncryptionConfig,
    /// The instant this job reaches [`ONBOARDING_STATUS_SUCCEEDED`]: the start instant
    /// plus [`ONBOARDING_JOB_DURATION`]. Until then the job is still in progress.
    pub succeeds_at: SystemTime,
}

impl StoredInstanceOnboarding {
    /// A job just started for `connect_instance_id` at `now`, due to have finished one
    /// [`ONBOARDING_JOB_DURATION`] later.
    pub fn starting(
        connect_instance_id: String,
        encryption_config: EncryptionConfig,
        now: SystemTime,
    ) -> Self {
        Self {
            connect_instance_id,
            encryption_config,
            succeeds_at: now + ONBOARDING_JOB_DURATION,
        }
    }

    /// The job's status at `now`: finished once the clock has passed the due instant,
    /// in progress until then.
    ///
    /// A pure read of the stored instant, so reporting a job's status writes nothing and
    /// two callers reading at the same moment see the same value.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now < self.succeeds_at {
            ONBOARDING_STATUS_IN_PROGRESS
        } else {
            ONBOARDING_STATUS_SUCCEEDED
        }
    }

    /// This record as the modelled `InstanceOnboardingJobStatus` the onboarding
    /// operations report, so the start and the status read cannot word one job two ways.
    ///
    /// `failureCode` is absent: the onboarding this emulator performs is writing the
    /// record, which either happened or was refused before any job started, so no
    /// stored job is in the modelled failed state.
    pub fn to_job_status(&self, now: SystemTime) -> InstanceOnboardingJobStatus {
        InstanceOnboardingJobStatus {
            connect_instance_id: self.connect_instance_id.clone(),
            status: self.status_at(now).to_string(),
            failure_code: None,
        }
    }
}

/// Why a requested onboarding job was not started, as
/// [`ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState::start_instance_onboarding`]
/// reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingRefused {
    /// The instance already holds an onboarding job, so there is no onboarding left to
    /// start for it: the modelled `ConflictException` (HTTP 409).
    AlreadyOnboarded,
    /// This emulator's store already holds [`MAX_ONBOARDED_INSTANCES`] records, so it
    /// takes no further instance.
    StoreFull,
}

/// What this service answers when the Amazon Connect instance a request names is
/// already onboarded to Campaigns, so its onboarding job cannot be started again.
///
/// The sentence the model documents for `ConflictException` — "The request could not be
/// processed because of conflict in the current state of the resource" — which is what
/// this refusal is: the state of the instance, and not the request, is what stops the
/// call. A FIXED service string carrying nothing from the request, so every refusal of
/// this class reads the same.
pub const INSTANCE_ALREADY_ONBOARDED_MESSAGE: &str =
    "The request could not be processed because of conflict in the current state of the resource.";

/// What this service answers when the Amazon Connect instance a request asks it to
/// onboard is not one the account holds: the id names no instance to onboard.
///
/// Composed from the request, so the refusal names the instance the caller asked about —
/// the same shape as [`campaign_not_found_message`], which is how this service words a
/// resource its request addresses by id. `CreateCampaign`, whose own resource is the
/// campaign and which only REQUIRES a usable instance, carries
/// [`INSTANCE_NOT_USABLE_MESSAGE`] for the same registry outcome, because each operation
/// answers it in the words recorded for that operation.
pub fn connect_instance_not_found_message(connect_instance_id: &str) -> String {
    format!("ConnectInstanceId not found {connect_instance_id}.")
}

/// What this service answers when an Amazon Connect instance has no onboarding job to
/// report on: it was never onboarded, or its job has since been deleted.
///
/// The onboarding RECORD is the resource that is missing here, not the instance, and the
/// record is scoped to an account — so the sentence names the caller's account id and the
/// instance the request named, and carries no trailing period. The one text the
/// operations whose own resource IS that record share, so they cannot disagree about a
/// request they both refuse.
pub fn onboarding_record_absent_message(connect_instance_id: &str) -> String {
    format!(
        "Instance onboarding record does not exist for aws account id {} and instance id {connect_instance_id}",
        ::emulator_server::emulator_account_id()
    )
}

/// The region a request was signed for, which is the region of every ARN the
/// handler serving it mints. Falls back to [`DEFAULT_REGION`] for a request that
/// carries no sigv4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A fresh campaign id.
///
/// The model shapes `CampaignId` as a non-whitespace string of at most 256 characters
/// (`len 0..256, pattern ^[\S]*$`), so a 32-character lowercase hex id satisfies it. The
/// bytes are never derived from the request: this process's OS-seeded
/// [`std::collections::hash_map::RandomState`] keys, the clock and a process-wide mint
/// counter are hashed together, so two creates carrying identical bodies still get
/// different ids and an id cannot be guessed from what was sent. (The same entropy
/// recipe as `services/service-com-amazonaws-vpclattice-mercurycontrolplane`'s
/// `fresh_entropy`.)
pub fn fresh_campaign_id() -> String {
    use ::sha2::{Digest, Sha256};
    use ::std::hash::{BuildHasher, Hasher};
    use ::std::sync::atomic::{AtomicU64, Ordering};

    /// How many ids this process has minted, so two ids minted in the same clock tick
    /// still differ.
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let minted = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let os_seeded = ::std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let digest = Sha256::digest(format!("{nanos}:{minted}:{os_seeded}").as_bytes());
    ::hex::encode(&digest[..16])
}

/// The codec every `ListCampaigns` `nextToken` is written with: the standard base64
/// alphabet, so the token a caller is handed back is the opaque base64 value this
/// service's pagination token is, and a token that is not base64 at all fails to
/// DECODE before anything is resolved from it.
///
/// Padding is accepted but not required on the way in, so a token whose trailing `=`
/// a client dropped still decodes — the same latitude a decoder that pads on demand
/// gives.
const PAGE_TOKEN_CODEC: ::base64::engine::GeneralPurpose = ::base64::engine::GeneralPurpose::new(
    &::base64::alphabet::STANDARD,
    ::base64::engine::GeneralPurposeConfig::new()
        .with_decode_padding_mode(::base64::engine::DecodePaddingMode::Indifferent),
);

/// How many hex digits the position inside a `nextToken` is written with, so every
/// token this service mints is the same length whatever position it names.
const PAGE_TOKEN_POSITION_DIGITS: usize = 16;

/// The `ListCampaigns` `nextToken` that resumes a walk at creation counter `sequence`.
///
/// A token names a POSITION in the creation order, not a campaign: deleting the campaign
/// a page was going to start at leaves the token resumable, and the token carries no
/// campaign id for a caller to read out of it. It is handed out base64-encoded, which
/// is the form [`page_token_sequence`] decodes before it resolves anything.
pub fn page_token(sequence: u64) -> String {
    use ::base64::Engine as _;

    PAGE_TOKEN_CODEC.encode(format!("{sequence:0width$x}", width = PAGE_TOKEN_POSITION_DIGITS))
}

/// Why a `nextToken` a caller sent names no position to resume a walk from.
///
/// Two outcomes, because a pagination token is resolved in two steps and each is
/// refused in its own words: a value that is not base64 does not survive the DECODE,
/// and a value that decodes but names no position this service issued does not
/// RESOLVE. [`Self::message`] is the one place either sentence lives, so every paged
/// list of this service refuses alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageTokenRejected {
    /// The token is not a base64 value, carrying the decoder's own account of why.
    Undecodable(String),
    /// The token decodes, but it is not one [`page_token`] minted.
    Unissued,
}

impl PageTokenRejected {
    /// The sentence this rejection is refused with.
    pub fn message(&self) -> String {
        match self {
            Self::Undecodable(why) => why.clone(),
            // This emulator's own wording, not a recorded one: a token only this
            // emulator can mint has no recorded refusal to reproduce. It carries
            // nothing from the request — a token a caller supplied is not echoed back
            // into the refusal — and it is deliberately not a decode failure's
            // sentence, which is the distinction the refusal exists to draw.
            Self::Unissued => "nextToken is not a page token this service issued".to_string(),
        }
    }
}

/// How a base64 decoder reports a value carrying a character its alphabet does not
/// hold: it stops at that character and names it by the hex code of its byte, so the
/// sentence is composed from the token the request carried.
pub fn illegal_base64_character_message(byte: u8) -> String {
    format!("Illegal base64 character {byte:x}")
}

/// How a base64 decoder reports a value whose last 4-byte unit carries too few bits to
/// decode — a dangling character left over after the whole units.
pub const DANGLING_BASE64_UNIT_MESSAGE: &str = "Last unit does not have enough valid bits";

/// How a base64 decoder reports a value whose last 4-byte unit is not one: padding
/// where no padding belongs, or too little of it.
pub const MALFORMED_BASE64_UNIT_MESSAGE: &str = "Input byte array has wrong 4-byte ending unit";

/// The padding character of the standard base64 alphabet, which closes a value's last
/// unit rather than carrying data — so it is not an illegal character, and a value that
/// misplaces it is refused as a malformed unit instead.
const BASE64_PAD: u8 = b'=';

/// The bytes `token` decodes to, or why it is not a base64 value at all.
///
/// Every account of WHY comes from the codec's own error rather than from a second,
/// hand-rolled pass over the token: the decoder stops at the first byte its alphabet does
/// not hold, and that is the byte the refusal names.
fn decode_page_token(token: &str) -> Result<Vec<u8>, PageTokenRejected> {
    use ::base64::Engine as _;

    match PAGE_TOKEN_CODEC.decode(token) {
        Ok(bytes) => Ok(bytes),
        Err(::base64::DecodeError::InvalidByte(_, byte)) if byte != BASE64_PAD => Err(
            PageTokenRejected::Undecodable(illegal_base64_character_message(byte)),
        ),
        Err(::base64::DecodeError::InvalidLength(_)) => Err(PageTokenRejected::Undecodable(
            DANGLING_BASE64_UNIT_MESSAGE.to_string(),
        )),
        Err(::base64::DecodeError::InvalidByte(..) | ::base64::DecodeError::InvalidPadding) => Err(
            PageTokenRejected::Undecodable(MALFORMED_BASE64_UNIT_MESSAGE.to_string()),
        ),
        // The value decodes; only its last unit carries bits a decoder that pads on
        // demand discards. Whatever those bytes are, they are not a token this service
        // minted, so it is refused as one it never issued rather than as a decode
        // failure.
        Err(::base64::DecodeError::InvalidLastSymbol(..)) => Err(PageTokenRejected::Unissued),
    }
}

/// The creation counter a `nextToken` resumes at, or why that token names no position.
///
/// The token is DECODED first — a value that is not base64 is refused as the decode
/// failure it is — and only then resolved against the form [`page_token`] mints.
pub fn page_token_sequence(token: &str) -> Result<u64, PageTokenRejected> {
    let decoded = decode_page_token(token)?;
    let position = String::from_utf8(decoded).map_err(|_| PageTokenRejected::Unissued)?;
    if position.len() != PAGE_TOKEN_POSITION_DIGITS {
        return Err(PageTokenRejected::Unissued);
    }
    u64::from_str_radix(&position, 16).map_err(|_| PageTokenRejected::Unissued)
}

/// Record one state-changing campaign operation: when (the record's own timestamp), who
/// asked (the access key id the request presented), what changed and how it ended.
///
/// The one place this service writes an audit record, so create, delete and every
/// run-state transition record the same fields. Only wire metadata and server-minted
/// identifiers go in: never a campaign's name, its tags or any credential material.
pub fn audit_campaign_change(
    parts: &::http::request::Parts,
    operation: &'static str,
    campaign_id: &str,
    outcome: &str,
) {
    ::tracing::info!(
        audit = true,
        operation,
        outcome,
        caller_akid = %::engine::extract_akid(&parts.headers),
        campaign_id,
        "campaign changed"
    );
}

/// Record one state-changing Connect-instance operation — starting an instance's
/// onboarding job — with the same fields as [`audit_campaign_change`]: when, who asked,
/// which instance and how the call ended.
///
/// Called on EVERY path such a call returns through, the accepted one and each refusal,
/// named by the modelled error the refusal answers with — so an attempt to change what
/// this service holds for an instance is as visible when it was refused as when it
/// succeeded. Only wire metadata and the instance id go in: never an encryption key,
/// an ARN a caller supplied or any credential material.
pub fn audit_onboarding_change(
    parts: &::http::request::Parts,
    operation: &'static str,
    connect_instance_id: &str,
    outcome: &str,
) {
    ::tracing::info!(
        audit = true,
        operation,
        outcome,
        caller_akid = %::engine::extract_akid(&parts.headers),
        connect_instance_id,
        "connect instance onboarding changed"
    );
}

/// The ARN of a campaign: `arn:aws:connect-campaigns:<region>:<account>:campaign/<id>`.
pub fn campaign_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("campaign/{id}"))
}

/// What this service answers when a well-formed campaign id names no campaign, for the
/// operations that RESOLVE the campaign resource that id names — the describe and the
/// delete, addressed to `/campaigns/{id}` itself.
///
/// Composed from the request, so the refusal names the id the caller asked for. The one
/// text those operations share, so two of them cannot disagree about a lookup they both
/// refuse. The operations addressed to the campaign's RUN STATE word the same miss
/// differently and take [`campaign_absent_message`] instead.
pub fn campaign_not_found_message(id: &str) -> String {
    format!("CampaignId not found {id}.")
}

/// What this service answers when a well-formed campaign id names no campaign, for the
/// operations addressed to the campaign's RUN STATE (`GetCampaignState`,
/// `StartCampaign`, `PauseCampaign`, `StopCampaign`).
///
/// Also composed from the request, and deliberately NOT the sentence a campaign-resource
/// lookup answers ([`campaign_not_found_message`]): the service words the two classes
/// differently, so they are kept apart rather than unified. The one text those
/// operations share.
pub fn campaign_absent_message(id: &str) -> String {
    format!("Campaign: {id} doesn't exist")
}

/// The member one batch state read names its campaign ids in, as the clause below spells
/// it: the Smithy member name, lowerCamel-ed by the framework's own path spelling.
const CAMPAIGN_IDS_MEMBER: &str = "campaignIds";

/// The most campaign ids one batch state read may name.
///
/// The cap the SERVICE enforces, which is why it lives here and not in the model: a batch
/// of up to this many well-formed ids is PROCESSED — every id answered on its own,
/// resolved or not — and only a batch past it is refused. This service's Smithy model
/// therefore declares `GetCampaignStateBatchRequest$campaignIds` with a `@length` minimum
/// of one and no maximum, so the boundary validator still refuses the empty list while
/// the cap is applied by the handler, which is the only layer that holds the submitted
/// ids and can state them in the refusal ([`campaign_ids_over_cap_message`]).
pub const CAMPAIGN_IDS_PER_BATCH: usize = 500;

/// The longest `CampaignId` this service's model admits (`@length(min 0, max 256)`), which
/// the boundary validator applies to every entry of a batch before the handler runs.
const CAMPAIGN_ID_MAX_LENGTH: usize = 256;

/// How long a rendering of a submitted batch one clause quotes in FULL.
///
/// A MODEL constant, not a request-sized one: the longest list a batch one id past
/// [`CAMPAIGN_IDS_PER_BATCH`] can render as, every id at [`CAMPAIGN_ID_MAX_LENGTH`] and
/// separated by `, ` inside the brackets. So the refusal quotes in full every batch that is
/// an attempt at the cap — which is what AWS's own clause states — and a batch many times
/// larger than the cap is quoted as the shared module elides any value, rather than turning
/// a request into a response sized by it.
const MAX_QUOTED_CAMPAIGN_IDS: usize =
    (CAMPAIGN_IDS_PER_BATCH + 1) * (CAMPAIGN_ID_MAX_LENGTH + ", ".len()) + "[]".len();

/// What this service answers when one batch state read names more campaign ids than
/// [`CAMPAIGN_IDS_PER_BATCH`].
///
/// The envelope, the member path and the requirement come from the framework's own
/// constraint renderer, so a cap the service enforces is stated exactly as one the model
/// declares. The value half is the list the caller submitted, rendered the way a list
/// renders ([`service_common::validation::rendered_list`]) and held to
/// [`MAX_QUOTED_CAMPAIGN_IDS`] by the same elision the shared module applies to any value
/// ([`service_common::validation::quoted_echo_up_to`]).
pub fn campaign_ids_over_cap_message(campaign_ids: &[String]) -> String {
    let elements: Vec<&str> = campaign_ids.iter().map(String::as_str).collect();
    let submitted = ::service_common::validation::rendered_list(&elements);
    let violation = ::aws_smithy_schema_server::Violation {
        path: ::aws_smithy_schema_server::constraints::path::member(CAMPAIGN_IDS_MEMBER),
        kind: ::aws_smithy_schema_server::ViolationKind::MaxLength(CAMPAIGN_IDS_PER_BATCH as i64),
        value: Some(
            ::service_common::validation::quoted_echo_up_to(&submitted, MAX_QUOTED_CAMPAIGN_IDS)
                .into_owned(),
        ),
        sensitive: false,
    };
    // One violation always renders an envelope; the fallback keeps this infallible rather
    // than answering a refusal with no sentence at all.
    ::aws_smithy_schema_server::envelope(&[violation]).unwrap_or_default()
}

/// What this service answers when the Amazon Connect instance a request names is not one
/// Campaigns can be used with: the account holds no instance under that id, or the
/// instance it holds is not enabled for Campaigns.
///
/// A FIXED service string — it names no instance and carries nothing from the request —
/// so every refusal of this class reads the same and the response is deterministic.
pub const INSTANCE_NOT_USABLE_MESSAGE: &str =
    "Connect Instance does not exist or is not enabled to use Campaigns.";

/// What this service answers when the Amazon Connect instance registry — another
/// service's store, reached through the shared engine — cannot be read at all, so the
/// instance question has no answer here.
pub const INSTANCE_REGISTRY_UNAVAILABLE_MESSAGE: &str =
    "the Amazon Connect instance registry is unavailable";

/// Why the Amazon Connect instance a request names is not a campaign target, as
/// [`connect_instance_refusal`] reports it.
///
/// Two outcomes, because they are answered differently: an id the account's registry
/// does not hold is a request fault, while a registry this emulator cannot read at all
/// is a server fault and never a reason to treat the instance as absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceRefusal {
    /// The account holds no Connect instance under the id the request names, so there
    /// is nothing Campaigns could be used with: the modelled `ValidationException`
    /// (HTTP 400) carrying [`INSTANCE_NOT_USABLE_MESSAGE`].
    NotUsable,
    /// The registry could not be read, so the existence question has no answer: the
    /// modelled server error carrying [`INSTANCE_REGISTRY_UNAVAILABLE_MESSAGE`].
    RegistryUnavailable,
}

/// Whether the Connect instance `connect_instance_id` names is one this service's
/// operations may be used with: `None` when it is, and otherwise which refusal the
/// caller owes.
///
/// Amazon Connect owns the instance registry — its `CreateInstance` is the only call
/// that mints an instance id and its `DeleteInstance` the only one that drops it — so
/// the answer is read from THAT service's own store through the one shared engine rather
/// than kept a second time here: an instance created or deleted over the Connect API is
/// a campaign target, or not, with no propagation step. It resolves by instance id
/// alone, which is how every Connect operation that takes an instance id resolves one.
///
/// The one place this gate is decided, so the operations that require a usable instance
/// cannot disagree about a request they all refuse.
///
/// LOCK ORDER: this takes the Connect store's lock, holds no other while it does, and
/// hands back an owned value rather than a guard — so it is called with NO guard of this
/// service's own store held. Keeping the cross-service read one-directional and
/// guard-free is what makes a lock cycle between the two stores unreachable.
pub fn connect_instance_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    connect_instance_id: &str,
) -> Option<InstanceRefusal> {
    let Some(state) =
        ::service_com_amazonaws_connect_amazonconnectservice::state::service_state(engine)
    else {
        return Some(InstanceRefusal::RegistryUnavailable);
    };
    let held = state.lock().instances.contains_key(connect_instance_id);
    (!held).then_some(InstanceRefusal::NotUsable)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState>)
}

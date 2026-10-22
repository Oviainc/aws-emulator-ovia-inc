// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connectcampaigns-amazonconnectcampaignservice service.

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::{DialerConfig, OutboundCallConfig};

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

/// The `ListCampaigns` `nextToken` that resumes a walk at creation counter `sequence`.
///
/// A token names a POSITION in the creation order, not a campaign: deleting the campaign
/// a page was going to start at leaves the token resumable, and the token carries no
/// campaign id for a caller to read out of it.
pub fn page_token(sequence: u64) -> String {
    format!("{sequence:016x}")
}

/// The creation counter a `nextToken` resumes at, or `None` when the token is not one
/// [`page_token`] issued.
pub fn page_token_sequence(token: &str) -> Option<u64> {
    if token.len() != 16 {
        return None;
    }
    u64::from_str_radix(token, 16).ok()
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

/// The ARN of a campaign: `arn:aws:connect-campaigns:<region>:<account>:campaign/<id>`.
pub fn campaign_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("campaign/{id}"))
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

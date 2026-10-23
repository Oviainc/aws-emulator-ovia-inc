// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2 service.
//!
//! The campaign store is the single source of truth for the campaign lifecycle:
//! `CreateCampaign` writes a record here, `DescribeCampaign` / `ListCampaigns` /
//! `GetCampaignState` project it, `Start`/`Pause`/`StopCampaign` move its
//! [`CampaignState`], and `DeleteCampaign` removes it. No handler keeps a copy of
//! its own, so every response describes what the store holds.
//!
//! The instance-onboarding store does the same for a Connect instance's onboarding
//! job: `StartInstanceOnboardingJob` writes a record here with the encryption
//! config it was given, and `GetInstanceOnboardingJobStatus` reports the status
//! that record is at.

use std::collections::{BTreeMap, HashMap};

use crate::ops::types::{
    Campaign, CampaignSummary, ChannelSubtypeConfig, CommunicationLimitsConfig,
    CommunicationTimeConfig, EncryptionConfig, EntryLimitsConfig, InstanceOnboardingJobStatus,
    Schedule, Source,
};

/// The ARN namespace (and SigV4 signing name) this service's ARNs carry, as the
/// Smithy model's `aws.api#service` trait declares it.
pub const ARN_NAMESPACE: &str = "connect-campaigns";

/// The region an ARN is rooted in when a request carries no SigV4 credential
/// scope to read one from (an anonymous or host-routed call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for: the SigV4 credential scope's region,
/// which is the region every ARN minted while serving it belongs to.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a campaign: `arn:aws:connect-campaigns:<region>:<account>:campaign/<id>`.
///
/// Built through [`emulator_server::emulator_arn`] so the account segment is the
/// emulator's canonical placeholder account rather than a literal.
pub fn campaign_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("campaign/{id}"))
}

/// Encode a `ListCampaigns` cursor: the creation sequence number of the last
/// campaign on the page just returned, hex-encoded so the token is opaque rather
/// than a readable position.
///
/// The cursor is a sequence number rather than an id so that a campaign deleted
/// between two pages does not invalidate a token already handed out: the next
/// page is simply everything created after that point.
pub fn encode_next_token(last_sequence: u64) -> String {
    ::hex::encode(last_sequence.to_string())
}

/// Decode a cursor produced by [`encode_next_token`], or `None` when the token
/// was not produced by it.
pub fn decode_next_token(token: &str) -> Option<u64> {
    String::from_utf8(::hex::decode(token).ok()?)
        .ok()?
        .parse()
        .ok()
}

/// The lifecycle state of a stored campaign.
///
/// The variants are exactly the `CampaignState` enum the Smithy model declares —
/// see `CAMPAIGN_STATE_VALUES` in [`crate::ops::types`], which
/// [`CampaignState::as_wire`] renders a value of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampaignState {
    Completed,
    Failed,
    Initialized,
    Paused,
    Running,
    Stopped,
}

impl CampaignState {
    /// The wire value a response carries for this state.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Initialized => "Initialized",
            Self::Paused => "Paused",
            Self::Running => "Running",
            Self::Stopped => "Stopped",
        }
    }

    /// The state this one moves to under `action`, or `None` when `action` is not
    /// legal from here (which the caller answers with
    /// `InvalidCampaignStateException`).
    ///
    /// Start runs a campaign that is not running yet; Pause suspends a running
    /// one; Stop ends one that has not finished. Nothing moves out of a terminal
    /// state (`Completed`, `Failed`), and no action is a no-op: repeating one is
    /// refused rather than silently accepted.
    pub fn after(self, action: CampaignAction) -> Option<Self> {
        match (self, action) {
            (Self::Initialized | Self::Paused | Self::Stopped, CampaignAction::Start) => {
                Some(Self::Running)
            }
            (Self::Running, CampaignAction::Pause) => Some(Self::Paused),
            (Self::Initialized | Self::Running | Self::Paused, CampaignAction::Stop) => {
                Some(Self::Stopped)
            }
            _ => None,
        }
    }
}

/// A lifecycle action a `StartCampaign` / `PauseCampaign` / `StopCampaign` call
/// asks for, as [`CampaignState::after`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampaignAction {
    Start,
    Pause,
    Stop,
}

impl CampaignAction {
    /// The verb this action is refused with, for the
    /// `InvalidCampaignStateException` message.
    pub fn verb(self) -> &'static str {
        match self {
            Self::Start => "started",
            Self::Pause => "paused",
            Self::Stop => "stopped",
        }
    }
}

/// A campaign as stored: everything `CreateCampaign` was given, plus the server's
/// own id, ARN, lifecycle state and creation instant.
#[derive(Debug, Clone)]
pub struct StoredCampaign {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub connect_instance_id: String,
    pub channel_subtype_config: Option<ChannelSubtypeConfig>,
    /// The modelled `type` member (`ExternalCampaignType`); `type` is a keyword.
    pub campaign_type: Option<String>,
    pub source: Option<Source>,
    pub connect_campaign_flow_arn: Option<String>,
    pub schedule: Option<Schedule>,
    pub entry_limits_config: Option<EntryLimitsConfig>,
    pub communication_time_config: Option<CommunicationTimeConfig>,
    pub communication_limits_override: Option<CommunicationLimitsConfig>,
    /// Ordered so the tag map a response carries is built deterministically.
    pub tags: BTreeMap<String, String>,
    pub state: CampaignState,
    /// When this campaign was created. Part of the record, not of any response:
    /// no modelled member in this slice carries it.
    pub created_at: std::time::SystemTime,
}

impl StoredCampaign {
    /// The stored tags as a response `TagMap`, absent when the campaign carries
    /// none (the member is optional, and AWS omits an empty map).
    pub fn tags_for_response(&self) -> Option<HashMap<String, String>> {
        if self.tags.is_empty() {
            None
        } else {
            Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
        }
    }

    /// The channel subtypes this campaign is configured for, as the model's
    /// `ChannelSubtype` values.
    ///
    /// Derived from the stored `channelSubtypeConfig`: one entry per channel block
    /// present. The values AND their order come from the model's own catalog,
    /// `CHANNEL_SUBTYPE_VALUES` in [`crate::ops::types`], so the list is
    /// deterministic and satisfies `ChannelSubtypeList`'s `@uniqueItems`.
    pub fn channel_subtypes(&self) -> Vec<String> {
        let Some(config) = self.channel_subtype_config.as_ref() else {
            return Vec::new();
        };
        crate::ops::types::CHANNEL_SUBTYPE_VALUES
            .iter()
            .filter(|subtype| match **subtype {
                "EMAIL" => config.email.is_some(),
                "SMS" => config.sms.is_some(),
                "TELEPHONY" => config.telephony.is_some(),
                "WHATSAPP" => config.whats_app.is_some(),
                // A catalog value with no block on `ChannelSubtypeConfig` is a
                // channel this emulator cannot be configured for, so it is not
                // reported as configured.
                _ => false,
            })
            .map(|subtype| (*subtype).to_string())
            .collect()
    }

    /// This record as the modelled `Campaign` shape `DescribeCampaign` returns.
    pub fn to_campaign(&self) -> Campaign {
        Campaign {
            id: self.id.clone(),
            arn: self.arn.clone(),
            name: self.name.clone(),
            connect_instance_id: self.connect_instance_id.clone(),
            channel_subtype_config: self.channel_subtype_config.clone(),
            r#type: self.campaign_type.clone(),
            source: self.source.clone(),
            connect_campaign_flow_arn: self.connect_campaign_flow_arn.clone(),
            schedule: self.schedule.clone(),
            entry_limits_config: self.entry_limits_config.clone(),
            communication_time_config: self.communication_time_config.clone(),
            communication_limits_override: self.communication_limits_override.clone(),
            tags: self.tags_for_response(),
        }
    }

    /// This record as the modelled `CampaignSummary` shape `ListCampaigns`
    /// returns.
    pub fn to_summary(&self) -> CampaignSummary {
        CampaignSummary {
            id: self.id.clone(),
            arn: self.arn.clone(),
            name: self.name.clone(),
            connect_instance_id: self.connect_instance_id.clone(),
            channel_subtypes: self.channel_subtypes(),
            r#type: self.campaign_type.clone(),
            schedule: self.schedule.clone(),
            entry_limits_config: self.entry_limits_config.clone(),
            connect_campaign_flow_arn: self.connect_campaign_flow_arn.clone(),
        }
    }
}

/// Record one state-changing instance-onboarding operation: who asked (the access
/// key id the request presented), which Connect instance it named and how the call
/// ended.
///
/// The one place this service writes an onboarding audit record, so a later
/// onboarding mutator records the same fields. Only wire metadata and the request's
/// instance id go in: never the stored encryption config, a key ARN or any
/// credential material.
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
        caller_akid = %::emulator_server::extract_akid(&parts.headers),
        connect_instance_id,
        "connect instance onboarding changed"
    );
}

/// The `InstanceOnboardingJobStatusCode` value a job that has not finished
/// onboarding the instance yet reports, as the model's catalog declares it (see
/// `INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES` in [`crate::ops::types`]).
pub const ONBOARDING_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `InstanceOnboardingJobStatusCode` value a finished onboarding job reports,
/// from the same catalog.
pub const ONBOARDING_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// How long after the `StartInstanceOnboardingJob` call that started it an
/// onboarding job is due to reach `SUCCEEDED`.
///
/// AWS documents no duration for the job, so this is the emulator's own: long
/// enough that a caller which polls immediately still observes `IN_PROGRESS` (the
/// transient state the real job has), short enough that a poller reaches
/// `SUCCEEDED` straight away. Nothing sleeps for it — the due instant is stored
/// and the status is read off the clock.
pub const ONBOARDING_JOB_DURATION: std::time::Duration = std::time::Duration::from_millis(500);

/// The onboarding job of one Connect instance: the encryption config the
/// `StartInstanceOnboardingJob` call persisted, and the instant its job is due to
/// have finished.
#[derive(Debug, Clone)]
pub struct StoredInstanceOnboarding {
    pub connect_instance_id: String,
    /// What the instance's campaign data is encrypted with, exactly as the call
    /// that started the job supplied it.
    pub encryption_config: EncryptionConfig,
    /// The instant this job reaches `SUCCEEDED`: the start instant plus
    /// [`ONBOARDING_JOB_DURATION`]. Before it the job reports `IN_PROGRESS`.
    pub succeeds_at: std::time::SystemTime,
}

impl StoredInstanceOnboarding {
    /// A job just started for `connect_instance_id` at `now`, due to succeed one
    /// [`ONBOARDING_JOB_DURATION`] later.
    pub fn starting(
        connect_instance_id: String,
        encryption_config: EncryptionConfig,
        now: std::time::SystemTime,
    ) -> Self {
        Self {
            connect_instance_id,
            encryption_config,
            succeeds_at: now + ONBOARDING_JOB_DURATION,
        }
    }

    /// The job's status at `now`: `SUCCEEDED` once the clock has passed the due
    /// instant, `IN_PROGRESS` until then.
    ///
    /// A pure read of the stored instant, so reporting the status writes nothing
    /// and two callers reading at the same moment see the same value.
    pub fn status_at(&self, now: std::time::SystemTime) -> &'static str {
        if now < self.succeeds_at {
            ONBOARDING_STATUS_IN_PROGRESS
        } else {
            ONBOARDING_STATUS_SUCCEEDED
        }
    }

    /// This record as the modelled `InstanceOnboardingJobStatus` shape the
    /// onboarding operations return.
    ///
    /// `failureCode` is absent: the onboarding this emulator performs is writing
    /// the record, which either happened or was refused before the job started, so
    /// no stored job is in the modelled `FAILED` state.
    pub fn to_job_status(&self, now: std::time::SystemTime) -> InstanceOnboardingJobStatus {
        InstanceOnboardingJobStatus {
            connect_instance_id: self.connect_instance_id.clone(),
            status: self.status_at(now).to_string(),
            failure_code: None,
        }
    }
}

/// The most campaigns this emulator's store holds, after which `CreateCampaign`
/// refuses with the modelled `ServiceQuotaExceededException` (HTTP 402) — the
/// error the model declares for "Request would cause a service quota to be
/// exceeded".
///
/// The store grows one record per `CreateCampaign` and shrinks only on an
/// explicit `DeleteCampaign`, so without a ceiling a caller could repeat the
/// create until the process runs out of memory. AWS's own per-instance campaign
/// quota is an adjustable Service Quota, so no particular figure would be
/// faithful to it; this one is the emulator's own, set far above any plausible
/// workload and enforced through the error AWS answers a quota breach with.
pub const MAX_CAMPAIGNS: usize = 1_000;

/// The most Connect instances this emulator's store holds onboarding records for,
/// after which `StartInstanceOnboardingJob` refuses to take another with the
/// modelled `ConflictException` (HTTP 409) — of the errors that operation declares,
/// the one that fits a refusal the service's own state and not the request causes.
/// A later fix that models a real onboarding quota should answer it the same way.
///
/// The store grows one record per newly onboarded instance, so without a ceiling a
/// caller could repeat the call with fresh instance ids until the process runs out
/// of memory. AWS publishes no such limit, so this is the emulator's own, set far
/// above any plausible workload.
pub const MAX_ONBOARDED_INSTANCES: usize = 1_000;

/// Why the store refused to take a campaign, as
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::insert_new_campaign`]
/// reports it. Nothing is stored in either case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertRejected {
    /// The store already holds [`MAX_CAMPAIGNS`] campaigns. The caller answers
    /// this with the modelled `ServiceQuotaExceededException` (HTTP 402).
    StoreFull,
    /// A campaign is already stored under that id, so taking this one would
    /// discard the existing record. A freshly minted id cannot collide, so the
    /// caller answers this as a server fault rather than a client error.
    IdTaken,
}

/// Per-service state for com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2. A handler reaches it through [`service_state`].
///
/// The map answers a `{id}` lookup in O(1); `order` carries the creation order as
/// `(sequence number, id)` pairs, which the methods below keep in step with the
/// map, so `ListCampaigns` paginates deterministically and its `nextToken` can
/// name the sequence number the last page ended at. Growth is bounded:
/// [`MAX_CAMPAIGNS`] records, enforced by [`Self::insert_new_campaign`].
#[derive(Debug, Default)]
pub struct ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State {
    campaigns: HashMap<String, StoredCampaign>,
    order: Vec<(u64, String)>,
    next_sequence: u64,
    /// One onboarding record per Connect instance, keyed by instance id. Bounded by
    /// [`MAX_ONBOARDED_INSTANCES`], enforced by [`Self::start_instance_onboarding`].
    instance_onboarding: HashMap<String, StoredInstanceOnboarding>,
}

impl ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State {
    /// The id the next campaign this store takes will carry: its creation
    /// sequence number, zero-padded to a fixed width.
    ///
    /// Minted from the store's own counter, never from anything in the request,
    /// and unique because the counter never repeats. The text is within
    /// `CampaignId`'s pattern `^[-:/a-zA-Z0-9]+$` and its 1..256 length bound.
    /// Call it holding the guard you then insert through, so two concurrent
    /// creates cannot be handed the same id.
    pub fn next_campaign_id(&self) -> String {
        format!("{:012}", self.next_sequence)
    }

    /// Store `campaign` at the end of the creation order and hand back the record
    /// now in the store — so a handler builds its response from the store rather
    /// than from the request it just read.
    ///
    /// Refuses, storing nothing, when the store is full or the id is already
    /// taken (see [`InsertRejected`]). The check and the insert happen under the
    /// one lock the caller holds, so concurrent creates cannot both pass them.
    pub fn insert_new_campaign(
        &mut self,
        campaign: StoredCampaign,
    ) -> Result<StoredCampaign, InsertRejected> {
        let full = self.campaigns.len() >= MAX_CAMPAIGNS;
        let id = campaign.id.clone();
        match self.campaigns.entry(id.clone()) {
            // Taking this campaign would discard the one already under that id.
            std::collections::hash_map::Entry::Occupied(_) => Err(InsertRejected::IdTaken),
            std::collections::hash_map::Entry::Vacant(_) if full => Err(InsertRejected::StoreFull),
            std::collections::hash_map::Entry::Vacant(slot) => {
                let stored = slot.insert(campaign).clone();
                // Only now, with the record actually in the map, does the order
                // gain an entry and a sequence number — so "a refusal stores
                // nothing" holds structurally on every arm above.
                self.order.push((self.next_sequence, id));
                self.next_sequence = self.next_sequence.saturating_add(1);
                Ok(stored)
            }
        }
    }

    /// The campaign stored under `id`, or `None` when there is none.
    pub fn campaign(&self, id: &str) -> Option<&StoredCampaign> {
        self.campaigns.get(id)
    }

    /// The campaign stored under `id`, for a handler that moves its state.
    pub fn campaign_mut(&mut self, id: &str) -> Option<&mut StoredCampaign> {
        self.campaigns.get_mut(id)
    }

    /// Remove the campaign stored under `id`, returning it when it was there.
    pub fn remove_campaign(&mut self, id: &str) -> Option<StoredCampaign> {
        let removed = self.campaigns.remove(id)?;
        self.order.retain(|(_, stored)| stored != id);
        Some(removed)
    }

    /// Every stored campaign, in creation order, each with its creation sequence
    /// number — the value a `ListCampaigns` cursor is built from.
    pub fn campaigns_in_order(&self) -> impl Iterator<Item = (u64, &StoredCampaign)> {
        self.order
            .iter()
            .filter_map(|(sequence, id)| self.campaigns.get(id).map(|c| (*sequence, c)))
    }

    /// Start the onboarding job for `connect_instance_id` with `encryption_config`,
    /// and hand back the record now in the store.
    ///
    /// An instance with no record gets a job that starts at `now`. An instance that
    /// already has one keeps that job — repeating the call does not re-onboard an
    /// instance, which is what makes the operation idempotent — and takes the
    /// request's encryption config, since the call carries the instance's config.
    ///
    /// `None` means the store already holds [`MAX_ONBOARDED_INSTANCES`] instances
    /// and nothing was written.
    pub fn start_instance_onboarding(
        &mut self,
        connect_instance_id: &str,
        encryption_config: EncryptionConfig,
        now: std::time::SystemTime,
    ) -> Option<StoredInstanceOnboarding> {
        if let Some(existing) = self.instance_onboarding.get_mut(connect_instance_id) {
            existing.encryption_config = encryption_config;
            return Some(existing.clone());
        }
        if self.instance_onboarding.len() >= MAX_ONBOARDED_INSTANCES {
            return None;
        }
        let record = StoredInstanceOnboarding::starting(
            connect_instance_id.to_string(),
            encryption_config,
            now,
        );
        self.instance_onboarding
            .insert(connect_instance_id.to_string(), record.clone());
        Some(record)
    }

    /// The onboarding record of `connect_instance_id`, or `None` when the instance
    /// has never been onboarded.
    pub fn instance_onboarding(
        &self,
        connect_instance_id: &str,
    ) -> Option<&StoredInstanceOnboarding> {
        self.instance_onboarding.get(connect_instance_id)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>)
}

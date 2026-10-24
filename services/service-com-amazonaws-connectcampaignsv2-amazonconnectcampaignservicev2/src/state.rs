// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2 service.
//!
//! The campaign store is the single source of truth for the campaign lifecycle:
//! `CreateCampaign` writes a record here, `DescribeCampaign` / `ListCampaigns` /
//! `GetCampaignState` project it, `Start`/`Pause`/`Resume`/`StopCampaign` move its
//! [`CampaignState`], and `DeleteCampaign` removes it. No handler keeps a copy of
//! its own, so every response describes what the store holds.
//!
//! A campaign's state is owed to the CLOCK as well as to those calls: a campaign
//! that has entered its run is `Completed` once its schedule's `endTime` has
//! passed, which [`StoredCampaign::state_at`] derives from the stored schedule on
//! every read, so no reader reports a state the schedule has already moved past.
//!
//! The instance-onboarding store beside it is the single source of truth for which
//! Connect instances are onboarded to Campaigns: `StartInstanceOnboardingJob` writes a
//! record here with the encryption config it was given,
//! `GetInstanceOnboardingJobStatus` reports the status that record is at,
//! `GetConnectInstanceConfig` projects the config it holds, and `CreateCampaign`
//! consults it before minting a campaign — an instance with no finished onboarding job
//! is not a campaign target.
//!
//! The integration store is the single source of truth for what an onboarded Connect
//! instance is integrated with: `PutConnectInstanceIntegration` writes one record per
//! integration the instance holds, keyed by the kind and resource ARN that identify
//! it, and `ListConnectInstanceIntegrations` projects those records.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    Campaign, CampaignSummary, ChannelSubtypeConfig, CommunicationLimits,
    CommunicationLimitsConfig, CommunicationTimeConfig, CustomerProfilesIntegrationSummary,
    EncryptionConfig, EntryLimitsConfig, InstanceConfig, InstanceOnboardingJobStatus,
    IntegrationConfig, IntegrationSummary, LambdaIntegrationSummary, OpenHours,
    QConnectIntegrationSummary, Schedule, Source, TimeWindow, DAY_OF_WEEK_VALUES,
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

/// The resource-type segment of the only ARN family this service's tag operations
/// recognise: a campaign, the one taggable resource its model mints an ARN for
/// (`CampaignArn`, built by [`campaign_arn`]).
pub const CAMPAIGN_RESOURCE_TYPE: &str = "campaign";

/// The wire error code ConnectCampaignsV2 answers a tag operation with when the ARN is
/// well formed but names no resource this account holds, at HTTP 404.
///
/// The recorded responses carry this code, which is NOT the name of the model's
/// corresponding shape (`ResourceNotFoundException`); a client branches on the code it
/// is sent, so the emulator sends this one through
/// [`crate::scaffolded::Scaffolded::wire_coded`].
pub const TAG_NOT_FOUND_CODE: &str = "NotFoundException";

/// The wire error code ConnectCampaignsV2 answers a tag operation with when the ARN's
/// resource path is not one of its own resources, at HTTP 400 — refused before any
/// existence check. Recorded in the same way as [`TAG_NOT_FOUND_CODE`], and not the
/// name of the model's corresponding shape (`ValidationException`).
pub const TAG_BAD_REQUEST_CODE: &str = "BadRequestException";

/// The most tags one resource holds.
///
/// Source: `TagKeyList`, the shape `UntagResource$tagKeys` takes, whose `@length` is
/// 0..50 — a resource whose every tag can be removed in one call holds at most 50.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The campaign id an `/v2/tags/{arn}` ARN names, or `None` when `arn` does not name
/// a connect-campaigns campaign at all.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`; the resource
/// portion may itself contain `:`, so only the first five fields are split off. The
/// ARN names a campaign when its service field is this service's ARN namespace and its
/// resource portion is `campaign/<id>` with a non-empty id. Anything else — another
/// service's ARN, an unknown resource type, a bare resource with no id — is a resource
/// path this service cannot resolve, which its tag operations refuse as a bad request
/// BEFORE asking whether any resource exists.
pub fn campaign_id_from_arn(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let ("arn", Some(_partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        fields.next()?,
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    if service != ARN_NAMESPACE {
        return None;
    }
    let (resource_type, id) = resource.split_once('/')?;
    (resource_type == CAMPAIGN_RESOURCE_TYPE && !id.is_empty()).then_some(id)
}

/// The most entries a `communicationLimitsList` carries.
///
/// Source: the `@length` of `CommunicationLimits$communicationLimitsList`, whose
/// maximum is 2.
pub const MAX_COMMUNICATION_LIMITS: usize = 2;

/// Why a communication-limits config is a bad request, or `None` when it is within
/// the model's bounds.
///
/// The one place this service checks a `CommunicationLimits` union against the
/// `@length` its list member carries, so every operation that accepts one
/// (`CreateCampaign`, `UpdateCampaignCommunicationLimits`) refuses an over-long list
/// the same way. `member` is the request path the refusal names, so the message is
/// built from the request rather than fixed text.
pub fn communication_limits_violation(
    member: &str,
    limits: Option<&CommunicationLimits>,
) -> Option<String> {
    let count = match limits? {
        CommunicationLimits::communicationLimitsList(list) => list.len(),
    };
    (count > MAX_COMMUNICATION_LIMITS).then(|| {
        format!(
            "Value at '{member}' failed to satisfy constraint: \
             Member must have length less than or equal to {MAX_COMMUNICATION_LIMITS}"
        )
    })
}

/// Why a communication-time config is a bad request, or `None` when every day its
/// windows open is one the model's `DayOfWeek` catalog holds (`DAY_OF_WEEK_VALUES`
/// in [`crate::ops::types`]).
///
/// The one place this service checks a `CommunicationTimeConfig`'s `dailyHours`
/// keys, so every operation that accepts one (`CreateCampaign`,
/// `UpdateCampaignCommunicationTime`) refuses a day naming no weekday the same way.
/// Those keys sit behind the `OpenHours` union, which the centralized constraint
/// walk does not reach, so without this check a campaign could be stored opening a
/// day the model does not model. `member` is the request path the refusal names, so
/// the message is built from the request rather than fixed text.
pub fn communication_time_violation(
    member: &str,
    config: &CommunicationTimeConfig,
) -> Option<String> {
    // Every per-channel window `CommunicationTimeConfig` carries, under the member
    // name a refusal reports it as.
    let windows: [(&str, Option<&TimeWindow>); 4] = [
        ("telephony", config.telephony.as_ref()),
        ("sms", config.sms.as_ref()),
        ("email", config.email.as_ref()),
        ("whatsApp", config.whats_app.as_ref()),
    ];
    for (channel, window) in windows {
        let Some(window) = window else {
            continue;
        };
        let OpenHours::dailyHours(daily_hours) = &window.open_hours;
        // Sorted so a config carrying more than one unmodelled day is always refused
        // for the same one, however the map happens to iterate.
        let mut days: Vec<&str> = daily_hours.keys().map(String::as_str).collect();
        days.sort_unstable();
        if let Some(day) = days
            .into_iter()
            .find(|day| !DAY_OF_WEEK_VALUES.contains(day))
        {
            return Some(format!(
                "Value '{day}' at '{member}.{channel}.openHours.dailyHours' failed to \
                 satisfy constraint: Member must satisfy enum value set: [{}]",
                DAY_OF_WEEK_VALUES.join(", ")
            ));
        }
    }
    None
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

/// `at` as the `(seconds, nanoseconds)` pair an `::aws_smithy_types::DateTime`
/// reports an instant as, so a wall-clock instant and a modelled timestamp compare
/// exactly rather than through a lossy float.
fn instant_parts(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
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
    /// one; Resume puts a suspended one back to work, so it is legal from `Paused`
    /// and from nowhere else; Stop ends one that has not finished. Nothing moves
    /// out of a terminal state (`Completed`, `Failed`), and no action is a no-op:
    /// repeating one is refused rather than silently accepted.
    ///
    /// A call is judged against the state the campaign is at NOW, which the clock
    /// has a say in — see [`StoredCampaign::state_at`], the one place a caller reads
    /// a campaign's state from.
    pub fn after(self, action: CampaignAction) -> Option<Self> {
        match (self, action) {
            (Self::Initialized | Self::Paused | Self::Stopped, CampaignAction::Start) => {
                Some(Self::Running)
            }
            (Self::Running, CampaignAction::Pause) => Some(Self::Paused),
            (Self::Paused, CampaignAction::Resume) => Some(Self::Running),
            (Self::Initialized | Self::Running | Self::Paused, CampaignAction::Stop) => {
                Some(Self::Stopped)
            }
            _ => None,
        }
    }

    /// Whether a campaign in this state still takes a change to its configuration
    /// (the flow it is associated with, its source, its schedule, its limits).
    ///
    /// `Completed` and `Failed` are the terminal states — [`Self::after`] moves
    /// nothing out of either — so a campaign in one of them will never run again and
    /// its configuration can no longer affect anything; a configuration change is
    /// refused there with `InvalidCampaignStateException`, which is exactly the error
    /// the model declares for every campaign-configuration operation
    /// (`UpdateCampaignFlowAssociation`, `UpdateCampaignSource`, …) and does NOT
    /// declare for `UpdateCampaignName`, the one update that is independent of how
    /// the campaign runs. Every non-terminal state takes the change.
    ///
    /// Shared by the configuration operations so two of them cannot disagree about
    /// which states are still configurable.
    pub fn accepts_configuration_change(self) -> bool {
        !matches!(self, Self::Completed | Self::Failed)
    }
}

/// A lifecycle action a `StartCampaign` / `PauseCampaign` / `ResumeCampaign` /
/// `StopCampaign` call asks for, as [`CampaignState::after`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampaignAction {
    Start,
    Pause,
    Resume,
    Stop,
}

impl CampaignAction {
    /// The verb this action is refused with, for the
    /// `InvalidCampaignStateException` message.
    pub fn verb(self) -> &'static str {
        match self {
            Self::Start => "started",
            Self::Pause => "paused",
            Self::Resume => "resumed",
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
    /// The state the lifecycle CALLS left this campaign in
    /// (`Start`/`Pause`/`Resume`/`StopCampaign`, through [`CampaignState::after`]).
    ///
    /// NOT what a reader reports: the campaign's state is owed to the clock as well
    /// as to those calls, so every reader asks [`Self::state_at`] for the state at
    /// the instant it is serving.
    pub lifecycle_state: CampaignState,
    /// When this campaign was created. Part of the record, not of any response:
    /// no modelled member in this slice carries it.
    pub created_at: std::time::SystemTime,
    /// The outbound requests this campaign has taken, keyed by the `clientToken`
    /// each was submitted under — the idempotency token `PutOutboundRequestBatch`
    /// ("This API is idempotent", per the model's own documentation) resolves a
    /// repeated submission through, so the same token is answered with the dial
    /// request id it was first given rather than a second one.
    pub outbound_requests: BTreeMap<String, StoredOutboundRequest>,
}

/// An outbound request a campaign has taken, as stored.
#[derive(Debug, Clone)]
pub struct StoredOutboundRequest {
    /// The `DialRequestId` this request was given. Minted from the campaign's own
    /// counter, never from anything in the request, and within `DialRequestId`'s
    /// pattern `^[-_.a-zA-Z0-9]+$` and its 1..256 length bound.
    pub id: String,
    /// The `ChannelSubtype` the request was submitted for.
    pub channel_subtype: String,
    /// The `expirationTime` the caller sent, as epoch seconds.
    pub expires_at_epoch_seconds: i64,
}

/// What a campaign's store did with one outbound request of a batch, as
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::take_outbound_request`]
/// reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundRequestOutcome {
    /// The request is in the campaign's buffer under this `DialRequestId` — either
    /// freshly minted, or the id an earlier submission under the same `clientToken`
    /// was given, in which case this repeat stored nothing new.
    Taken(String),
    /// The campaign's buffer already holds [`MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN`]
    /// requests, so nothing was stored.
    BufferFull,
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

    /// Whether this campaign's scheduled run window has closed at `now`.
    ///
    /// A campaign carries a window only when it carries a `Schedule`, whose
    /// `endTime` is `@required` — so a scheduled campaign always knows the instant
    /// its window closes, and an unscheduled one has no window to close.
    pub fn run_window_closed_at(&self, now: SystemTime) -> bool {
        let now = instant_parts(::aws_smithy_types::DateTime::from(now));
        self.schedule
            .as_ref()
            .is_some_and(|schedule| now >= instant_parts(schedule.end_time))
    }

    /// This campaign's lifecycle state at `now`.
    ///
    /// The state is owed to the clock as well as to the lifecycle calls: a campaign
    /// that has entered its run — `Running` or `Paused`, the states a
    /// `StartCampaign` leads to — has run to the end of its schedule once that
    /// schedule's `endTime` has passed, and a run that has reached its end is the
    /// terminal `Completed`. A campaign that never entered a run (`Initialized`) or
    /// whose run a caller ended (`Stopped`) is left where the calls put it, and a
    /// terminal state moves nowhere.
    ///
    /// A pure read of the stored schedule, so reporting the state writes nothing and
    /// two callers reading at the same instant see the same value — the same way
    /// [`StoredInstanceOnboarding::status_at`] reports an onboarding job's status.
    pub fn state_at(&self, now: SystemTime) -> CampaignState {
        match self.lifecycle_state {
            CampaignState::Running | CampaignState::Paused if self.run_window_closed_at(now) => {
                CampaignState::Completed
            }
            stored => stored,
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

    /// Remove the communication-time window this campaign holds for the channel
    /// `config_type` names.
    ///
    /// `config_type` is a `CommunicationTimeConfigType` value — the catalog
    /// `COMMUNICATION_TIME_CONFIG_TYPE_VALUES` in [`crate::ops::types`], which the
    /// caller refuses a value outside of before reaching here, so a value naming no
    /// channel removes nothing.
    ///
    /// Only the named channel's window goes. `CommunicationTimeConfig`'s
    /// `localTimeZoneConfig` is `@required`, so the config itself stays on the record
    /// with the time zone it was configured with and the other channels keep their
    /// windows; a campaign carrying no communication-time config at all holds no
    /// window for any channel, so there is nothing to remove and nothing is written.
    pub fn clear_communication_time_window(&mut self, config_type: &str) {
        let Some(config) = self.communication_time_config.as_mut() else {
            return;
        };
        let window = match config_type {
            "EMAIL" => &mut config.email,
            "SMS" => &mut config.sms,
            "TELEPHONY" => &mut config.telephony,
            "WHATSAPP" => &mut config.whats_app,
            // A catalog value with no window on `CommunicationTimeConfig` is a
            // channel this emulator cannot be configured for, so it holds nothing to
            // remove.
            _ => return,
        };
        *window = None;
    }

    /// Remove the communication-limits block this campaign's override holds for the
    /// config `config_type` names.
    ///
    /// `config_type` is a `CommunicationLimitsConfigType` value — the catalog
    /// `COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES` in [`crate::ops::types`], whose one
    /// entry `ALL_CHANNEL_SUBTYPES` is the `CommunicationLimitsConfig$allChannelSubtypes`
    /// block; the caller refuses a value outside that catalog before reaching here, so a
    /// value naming no block removes nothing.
    ///
    /// Only the named block goes: `instanceLimitsHandling` is not a config type any
    /// request can name, so a campaign that set it keeps it. An override left carrying
    /// no member at all carries nothing, so the record holds no override rather than an
    /// empty one — a campaign with no override to begin with is left exactly as it was,
    /// which is what makes the operation idempotent, as the model declares.
    pub fn clear_communication_limits(&mut self, config_type: &str) {
        let Some(config) = self.communication_limits_override.as_mut() else {
            return;
        };
        match config_type {
            "ALL_CHANNEL_SUBTYPES" => config.all_channel_subtypes = None,
            // A catalog value with no block on `CommunicationLimitsConfig` is a config
            // this emulator cannot be configured for, so it holds nothing to remove.
            _ => return,
        }
        if config.all_channel_subtypes.is_none() && config.instance_limits_handling.is_none() {
            self.communication_limits_override = None;
        }
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

/// Record one state-changing Connect-instance operation — onboarding a instance, or
/// configuring one: who asked (the access key id the request presented), which
/// Connect instance it named and how the call ended.
///
/// The one place this service writes an instance audit record, so a later instance
/// mutator records the same fields. Only wire metadata and the request's instance id
/// go in: never a stored encryption or integration config, a key or resource ARN or
/// any credential material.
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

/// The `InstanceOnboardingJobStatusCode` value a job that could not onboard the
/// instance reports, from the same catalog. It is the ONLY status
/// `DeleteInstanceOnboardingJob` accepts: "Instance onboarding job is not in failed
/// status. Only failed job can be deleted."
///
/// No job this emulator stores reaches it — the onboarding it performs is writing
/// the record, which either happened or was refused before the job started (see
/// [`StoredInstanceOnboarding::to_job_status`]) — so the delete edge it opens is the
/// one a failing job would use.
pub const ONBOARDING_STATUS_FAILED: &str = "FAILED";

/// Why `DeleteInstanceOnboardingJob` refuses to delete a job that has not failed.
///
/// A fixed service string: it names no instance and carries nothing from the
/// request, so every refusal of this kind reads the same, as the recorded responses
/// do.
pub const ONBOARDING_JOB_NOT_FAILED_MESSAGE: &str =
    "Instance onboarding job is not in failed status. Only failed job can be deleted.";

/// What this service answers when a well-formed campaign id names no campaign.
///
/// A FIXED service string — it does not embed the requested id, which is what the
/// recorded `DescribeCampaign` 404 carries — so every campaign-id lookup that
/// resolves to nothing reads the same and the response is deterministic. The one
/// not-found text the campaign-id operations share, so two of them cannot disagree.
/// (`PutOutboundRequestBatch` keeps its own recorded wording, which does name the
/// campaign: that operation's message is composed from the request.)
pub const CAMPAIGN_NOT_FOUND_MESSAGE: &str = "CampaignId not found.";

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

    /// This record as the modelled `InstanceConfig` shape `GetConnectInstanceConfig`
    /// returns: the instance, the encryption config this record persisted, and the
    /// account's Campaigns service-linked role (see [`service_linked_role_arn`]).
    ///
    /// A config exists once the instance's onboarding job has finished, so the caller
    /// checks [`Self::status_at`] before projecting one.
    pub fn to_config(&self) -> InstanceConfig {
        InstanceConfig {
            connect_instance_id: self.connect_instance_id.clone(),
            service_linked_role_arn: service_linked_role_arn(),
            encryption_config: self.encryption_config.clone(),
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

/// The most outbound requests one campaign's store holds, after which a further
/// request in a batch is reported as a per-request failure carrying the modelled
/// `FailureCode` `BufferLimitExceeded` — the code the model declares for a campaign
/// that cannot take another request right now.
///
/// A campaign's buffer grows one record per accepted request and never shrinks, so
/// without a ceiling a caller could submit batches until the process runs out of
/// memory. The figure is the emulator's own, set far above any plausible workload.
pub const MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN: usize = 10_000;

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

/// The service-linked role every onboarded instance's config names.
///
/// `InstanceConfig$serviceLinkedRoleArn` is required, and AWS fills it with the
/// Campaigns service-linked role of the caller's own account — one role per account,
/// not per instance — so it is built from the emulator's canonical account rather than
/// from anything in the request. IAM is global, hence the empty region segment.
pub fn service_linked_role_arn() -> String {
    ::emulator_server::emulator_arn(
        "iam",
        "",
        "role/aws-service-role/connect-campaigns.amazonaws.com/AWSServiceRoleForAmazonConnectCampaigns",
    )
}

/// What this service answers when a Connect instance id names no instance onboarded
/// to Campaigns.
///
/// The one text every operation that resolves an instance id against the
/// instance-onboarding store uses, so two of them cannot disagree about a request
/// they both refuse. Composed from the request, which is why it names the id it was
/// given.
pub fn instance_not_found_message(connect_instance_id: &str) -> String {
    format!(
        "Connect instance {connect_instance_id} does not exist or is not enabled to use Campaigns"
    )
}

/// The most integrations one Connect instance's store holds, after which a further
/// put is refused rather than taken.
///
/// AWS publishes no limit on the integrations of a Connect instance, so this figure
/// is the emulator's own: without a ceiling a caller could put integrations until the
/// process runs out of memory. It is set far above any plausible workload — the model
/// declares three integration kinds, and one instance integrates few resources of
/// each.
pub const MAX_INTEGRATIONS_PER_INSTANCE: usize = 100;

/// Which of the three integrations the `IntegrationConfig`, `IntegrationIdentifier`
/// and `IntegrationSummary` unions each declare a member for a record is.
///
/// The kind and the ARN of the integrated resource together identify one integration
/// of one Connect instance: that pair is exactly what the matching
/// `IntegrationIdentifier` member carries (`customerProfiles` a domain ARN, `qConnect`
/// a knowledge-base ARN, `lambda` a function ARN), so it is the key an integration is
/// stored under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationKind {
    CustomerProfiles,
    QConnect,
    Lambda,
}

impl IntegrationKind {
    /// The union member this kind is carried in, as the model names it — the key a
    /// request or response body carries the integration under.
    pub fn member_name(self) -> &'static str {
        match self {
            Self::CustomerProfiles => "customerProfiles",
            Self::QConnect => "qConnect",
            Self::Lambda => "lambda",
        }
    }
}

/// The identity of the integration `config` configures: its kind, and the ARN of the
/// resource it integrates (see [`IntegrationKind`]).
///
/// The one place that pair is read off a config, so the key a put stores under and
/// the key a reader matches on cannot disagree.
pub fn integration_key(config: &IntegrationConfig) -> (IntegrationKind, &str) {
    match config {
        IntegrationConfig::customerProfiles(config) => (
            IntegrationKind::CustomerProfiles,
            config.domain_arn.as_str(),
        ),
        IntegrationConfig::qConnect(config) => (
            IntegrationKind::QConnect,
            config.knowledge_base_arn.as_str(),
        ),
        IntegrationConfig::Lambda(config) => {
            (IntegrationKind::Lambda, config.function_arn.as_str())
        }
    }
}

/// One integration of one Connect instance: the config the
/// `PutConnectInstanceIntegration` call persisted, and where in the store's order it
/// sits.
#[derive(Debug, Clone)]
pub struct StoredIntegration {
    /// The store-wide sequence number this integration was first put at — the value
    /// a `ListConnectInstanceIntegrations` cursor names. Updating an integration
    /// keeps it, so a put does not reorder a page a caller is part way through.
    pub sequence: u64,
    /// The integration config, exactly as the call that put it supplied it.
    pub config: IntegrationConfig,
}

impl StoredIntegration {
    /// Which integration this is: the kind and resource ARN it is stored under (see
    /// [`integration_key`]).
    pub fn key(&self) -> (IntegrationKind, &str) {
        integration_key(&self.config)
    }

    /// This record as the modelled `IntegrationSummary` `ListConnectInstanceIntegrations`
    /// returns: the same union member as the stored config, carrying the values that
    /// config holds.
    pub fn to_summary(&self) -> IntegrationSummary {
        match &self.config {
            IntegrationConfig::customerProfiles(config) => {
                IntegrationSummary::customerProfiles(CustomerProfilesIntegrationSummary {
                    domain_arn: config.domain_arn.clone(),
                    object_type_names: config.object_type_names.clone(),
                })
            }
            IntegrationConfig::qConnect(config) => {
                IntegrationSummary::qConnect(QConnectIntegrationSummary {
                    knowledge_base_arn: config.knowledge_base_arn.clone(),
                })
            }
            IntegrationConfig::Lambda(config) => {
                IntegrationSummary::Lambda(LambdaIntegrationSummary {
                    function_arn: config.function_arn.clone(),
                })
            }
        }
    }
}

/// Why the store refused to take a campaign, as
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::insert_new_campaign`]
/// reports it. Nothing is stored in any case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertRejected {
    /// The store already holds [`MAX_CAMPAIGNS`] campaigns. The caller answers
    /// this with the modelled `ServiceQuotaExceededException` (HTTP 402).
    StoreFull,
    /// A campaign is already stored under that id, so taking this one would
    /// discard the existing record. A freshly minted id cannot collide, so the
    /// caller answers this as a server fault rather than a client error.
    IdTaken,
    /// The Connect instance this campaign names already holds a campaign of that
    /// name, and a campaign name identifies a campaign within its instance (see
    /// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::campaign_named`]).
    /// The caller answers this with the modelled `ConflictException` (HTTP 409).
    NameTaken,
}

/// Per-service state for com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2. A handler reaches it through [`service_state`].
///
/// The map answers a `{id}` lookup in O(1); `order` carries the creation order as
/// `(sequence number, id)` pairs, which the methods below keep in step with the
/// map, so `ListCampaigns` paginates deterministically and its `nextToken` can
/// name the sequence number the last page ended at. Growth is bounded:
/// [`MAX_CAMPAIGNS`] records, enforced by [`Self::insert_new_campaign`].
///
/// `instance_onboarding` is the second store: one onboarding record per Connect
/// instance, keyed by its instance id, bounded at [`MAX_ONBOARDED_INSTANCES`] by
/// [`Self::start_instance_onboarding`]. It is the one place "is this instance a
/// campaign target?" is answered, so `CreateCampaign`'s gate and
/// `GetConnectInstanceConfig` / `GetInstanceOnboardingJobStatus` cannot disagree.
///
/// `instance_integrations` is the third: the integrations of each Connect instance,
/// which `PutConnectInstanceIntegration` writes and
/// `ListConnectInstanceIntegrations` projects, so the list reports what the puts
/// stored and nothing else.
#[derive(Debug, Default)]
pub struct ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State {
    campaigns: HashMap<String, StoredCampaign>,
    order: Vec<(u64, String)>,
    next_sequence: u64,
    next_dial_sequence: u64,
    /// One onboarding record per Connect instance, keyed by instance id. Bounded by
    /// [`MAX_ONBOARDED_INSTANCES`], enforced by [`Self::start_instance_onboarding`].
    instance_onboarding: HashMap<String, StoredInstanceOnboarding>,
    /// The integrations of each Connect instance, keyed by instance id, each
    /// instance's in the order they were first put — the order
    /// `ListConnectInstanceIntegrations` pages through. Bounded at
    /// [`MAX_INTEGRATIONS_PER_INSTANCE`] per instance by
    /// [`Self::put_instance_integration`].
    instance_integrations: HashMap<String, Vec<StoredIntegration>>,
    next_integration_sequence: u64,
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
    /// Refuses, storing nothing, when the campaign's Connect instance already
    /// holds a campaign of that name, when the store is full, or when the id is
    /// already taken (see [`InsertRejected`]). The checks and the insert happen
    /// under the one lock the caller holds, so concurrent creates cannot both pass
    /// them — of two creates racing on one name, exactly one stores a campaign.
    pub fn insert_new_campaign(
        &mut self,
        campaign: StoredCampaign,
    ) -> Result<StoredCampaign, InsertRejected> {
        // A campaign name identifies a campaign within its Connect instance, so a
        // record whose (instance, name) pair is already held is refused before any
        // capacity question: the resource it names exists, and taking a second one
        // would leave the instance with two campaigns answering to one name. This is
        // the one place that invariant is enforced, so every writer of a campaign
        // name upholds it.
        if self
            .campaign_named(&campaign.connect_instance_id, &campaign.name)
            .is_some()
        {
            return Err(InsertRejected::NameTaken);
        }
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

    /// The campaign `connect_instance_id` holds under `name`, or `None` when that
    /// instance holds none.
    ///
    /// A campaign name is scoped to its Connect instance: the same name under two
    /// instances names two campaigns, and the comparison is exact — a name differing
    /// in case or in surrounding whitespace is a different name, since the model puts
    /// no normalization on `CampaignName`.
    ///
    /// Walked in creation order, so the record answered for is the oldest one, not
    /// whichever the hash map happens to yield first.
    pub fn campaign_named(
        &self,
        connect_instance_id: &str,
        name: &str,
    ) -> Option<&StoredCampaign> {
        self.campaigns_in_order()
            .map(|(_, campaign)| campaign)
            .find(|campaign| {
                campaign.connect_instance_id == connect_instance_id && campaign.name == name
            })
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

    /// The campaign whose ARN is `arn`, or `None` when no stored campaign carries
    /// it.
    ///
    /// The whole ARN is compared, not just the id it ends with: an ARN names a
    /// partition, a region and an account as well as a resource, so one naming a
    /// different region or account names a different resource and resolves to
    /// nothing here.
    pub fn campaign_by_arn(&self, arn: &str) -> Option<&StoredCampaign> {
        let id = campaign_id_from_arn(arn)?;
        self.campaigns.get(id).filter(|stored| stored.arn == arn)
    }

    /// The campaign whose ARN is `arn`, for a handler that writes to it. Resolved
    /// exactly as [`Self::campaign_by_arn`] resolves it.
    pub fn campaign_by_arn_mut(&mut self, arn: &str) -> Option<&mut StoredCampaign> {
        let id = campaign_id_from_arn(arn)?.to_string();
        self.campaigns
            .get_mut(&id)
            .filter(|stored| stored.arn == arn)
    }

    /// The id the next outbound request this store takes will carry: a store-wide
    /// sequence number, so no two dial requests of any campaign share an id.
    ///
    /// Minted from the store's own counter, never from anything in the request. The
    /// text is within `DialRequestId`'s pattern `^[-_.a-zA-Z0-9]+$` and its 1..256
    /// length bound.
    fn next_dial_request_id(&mut self) -> String {
        let sequence = self.next_dial_sequence;
        self.next_dial_sequence = self.next_dial_sequence.saturating_add(1);
        format!("dial-{sequence:012}")
    }

    /// Take the outbound request submitted under `client_token` into the buffer of
    /// the campaign stored under `campaign_id`, and report what became of it (see
    /// [`OutboundRequestOutcome`]). `None` when no campaign is stored under that id.
    ///
    /// A `client_token` the campaign has already taken stores nothing and reports
    /// the id it was given then, which is what makes a repeated batch idempotent.
    pub fn take_outbound_request(
        &mut self,
        campaign_id: &str,
        client_token: &str,
        channel_subtype: &str,
        expires_at_epoch_seconds: i64,
    ) -> Option<OutboundRequestOutcome> {
        {
            let campaign = self.campaigns.get(campaign_id)?;
            if let Some(taken) = campaign.outbound_requests.get(client_token) {
                return Some(OutboundRequestOutcome::Taken(taken.id.clone()));
            }
            if campaign.outbound_requests.len() >= MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN {
                return Some(OutboundRequestOutcome::BufferFull);
            }
        }
        // Only now, past both refusals, is an id minted — so a refused request
        // consumes no id and leaves no gap in the sequence.
        let id = self.next_dial_request_id();
        let campaign = self.campaigns.get_mut(campaign_id)?;
        campaign.outbound_requests.insert(
            client_token.to_string(),
            StoredOutboundRequest {
                id: id.clone(),
                channel_subtype: channel_subtype.to_string(),
                expires_at_epoch_seconds,
            },
        );
        Some(OutboundRequestOutcome::Taken(id))
    }

    /// The onboarding record of `connect_instance_id` when that instance is onboarded
    /// to Campaigns at `now` — a record exists AND its job has reached `SUCCEEDED` —
    /// and `None` otherwise. An instance whose job is still `IN_PROGRESS` is not a
    /// campaign target yet.
    ///
    /// The one place "is this instance onboarded?" is decided, so `CreateCampaign`'s
    /// gate ([`Self::is_onboarded`]) and `GetConnectInstanceConfig`'s projection
    /// cannot disagree. A pure read of the stored due instant (see
    /// [`StoredInstanceOnboarding::status_at`]), so neither writes anything.
    pub fn onboarded_instance(
        &self,
        connect_instance_id: &str,
        now: SystemTime,
    ) -> Option<&StoredInstanceOnboarding> {
        self.instance_onboarding(connect_instance_id)
            .filter(|record| record.status_at(now) == ONBOARDING_STATUS_SUCCEEDED)
    }

    /// Whether `connect_instance_id` is onboarded to Campaigns at `now`, as
    /// [`Self::onboarded_instance`] decides it.
    pub fn is_onboarded(&self, connect_instance_id: &str, now: SystemTime) -> bool {
        self.onboarded_instance(connect_instance_id, now).is_some()
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

    /// Delete the onboarding job of `connect_instance_id`, and report what became of
    /// the request (see [`DeleteOnboardingOutcome`]).
    ///
    /// Only a job that has FAILED may be deleted: a job in any other status keeps
    /// its record, so the status a later `GetInstanceOnboardingJobStatus` reports
    /// and the config `GetConnectInstanceConfig` projects are untouched by a refused
    /// delete. The status is read off the stored due instant at `now`, the same way
    /// every other reader of the record reads it.
    pub fn delete_instance_onboarding(
        &mut self,
        connect_instance_id: &str,
        now: SystemTime,
    ) -> DeleteOnboardingOutcome {
        let Some(record) = self.instance_onboarding.get(connect_instance_id) else {
            return DeleteOnboardingOutcome::NoSuchJob;
        };
        if record.status_at(now) != ONBOARDING_STATUS_FAILED {
            return DeleteOnboardingOutcome::NotFailed;
        }
        self.instance_onboarding.remove(connect_instance_id);
        DeleteOnboardingOutcome::Deleted
    }

    /// Put `config` as an integration of `connect_instance_id`, and hand back the
    /// record now in the store — so a handler reports what the store holds rather
    /// than what the request carried.
    ///
    /// An integration already stored under the same identity (see
    /// [`integration_key`]) takes the new config and KEEPS its sequence number: that
    /// is what makes the call an update of that integration rather than a second one
    /// beside it. A config under an identity the instance does not hold is appended.
    ///
    /// `None` means the instance already holds [`MAX_INTEGRATIONS_PER_INSTANCE`]
    /// integrations and nothing was written — a refusal consumes no sequence number
    /// and leaves no empty entry behind.
    pub fn put_instance_integration(
        &mut self,
        connect_instance_id: &str,
        config: IntegrationConfig,
    ) -> Option<StoredIntegration> {
        let (kind, arn) = {
            let (kind, arn) = integration_key(&config);
            (kind, arn.to_string())
        };
        let held = self
            .instance_integrations
            .get(connect_instance_id)
            .map_or(0, Vec::len);
        if let Some(existing) = self
            .instance_integrations
            .get_mut(connect_instance_id)
            .and_then(|integrations| {
                integrations
                    .iter_mut()
                    .find(|stored| stored.key() == (kind, arn.as_str()))
            })
        {
            existing.config = config;
            return Some(existing.clone());
        }
        if held >= MAX_INTEGRATIONS_PER_INSTANCE {
            return None;
        }
        // Only now, past the refusal, is a sequence number taken — so the cursor
        // space has no gaps a refused put left behind.
        let record = StoredIntegration {
            sequence: self.next_integration_sequence,
            config,
        };
        self.next_integration_sequence = self.next_integration_sequence.saturating_add(1);
        self.instance_integrations
            .entry(connect_instance_id.to_string())
            .or_default()
            .push(record.clone());
        Some(record)
    }

    /// Every integration of `connect_instance_id`, in the order they were first put.
    ///
    /// An instance that holds none yields nothing, which is not the same as an
    /// instance that is not onboarded: whether the instance exists at all is
    /// [`Self::is_onboarded`]'s answer, not this one's.
    pub fn instance_integrations(
        &self,
        connect_instance_id: &str,
    ) -> impl Iterator<Item = &StoredIntegration> {
        self.instance_integrations
            .get(connect_instance_id)
            .into_iter()
            .flatten()
    }
}

/// What [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::delete_instance_onboarding`]
/// did with a delete request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOnboardingOutcome {
    /// The record was FAILED and is gone.
    Deleted,
    /// The instance has no onboarding record at all.
    NoSuchJob,
    /// The record is in a status other than FAILED and was left untouched.
    NotFailed,
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

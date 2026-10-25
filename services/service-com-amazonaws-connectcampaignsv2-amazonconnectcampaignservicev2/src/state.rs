// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2 service.
//!
//! Every record below belongs to ONE region: campaigns and Connect instance
//! configurations are regional resources, reachable only through the endpoint of the
//! region they were created in, so the store keeps one set of records per
//! credential-scope region ([`request_region`]) and every lookup, listing and write
//! takes the region it is serving and resolves inside that region alone.
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
//! is not a campaign target. The instance-wide communication limits live on that same
//! record: `PutInstanceCommunicationLimits` writes them and
//! `GetInstanceCommunicationLimits` reports them, so neither keeps a copy of its own.
//!
//! The integration store is the single source of truth for what an onboarded Connect
//! instance is integrated with: `PutConnectInstanceIntegration` writes one record per
//! integration the instance holds, keyed by the kind and resource ARN that identify
//! it, `ListConnectInstanceIntegrations` projects those records and
//! `DeleteConnectInstanceIntegration` removes the one its identifier names.
//!
//! `DeleteConnectInstanceConfig` is the other end of onboarding: it removes an
//! instance's record from the onboarding store together with everything held on that
//! configuration — its communication limits and its integrations — so the instance is
//! no longer onboarded and no reader reports a configuration for it. The campaigns of
//! the instance go with it only when the request's campaign deletion policy says so.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    Campaign, CampaignSummary, ChannelSubtypeConfig, CommunicationLimits,
    CommunicationLimitsConfig, CommunicationTimeConfig, CustomerProfilesIntegrationSummary,
    EncryptionConfig, EntryLimitsConfig, InstanceCommunicationLimitsConfig, InstanceConfig,
    InstanceOnboardingJobStatus, IntegrationConfig, IntegrationIdentifier, IntegrationSummary,
    LambdaIntegrationSummary,
    OpenHours, QConnectIntegrationSummary, Schedule, Source, TimeWindow, DAY_OF_WEEK_VALUES,
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

/// The shortest tag key this service accepts.
///
/// Source: the `@length` of `TagKey`, the shape every tag map's key and every tag-key
/// list's member targets, whose minimum is 1.
pub const TAG_KEY_MIN_LENGTH: usize = 1;

/// The longest tag key this service accepts, from the same `@length`, whose maximum
/// is 128.
pub const TAG_KEY_MAX_LENGTH: usize = 128;

/// `TagKey`'s modelled `@pattern`, quoted exactly as the model states it so a refusal
/// names the expression the key failed.
pub const TAG_KEY_PATTERN: &str = "^(?!aws:)[a-zA-Z+-=._:/]+$";

/// The key prefix [`TAG_KEY_PATTERN`]'s leading negative lookahead excludes: a
/// namespace the service reserves for keys of its own, so a caller cannot supply one.
const TAG_KEY_RESERVED_PREFIX: &str = "aws:";

/// Whether every character of `key` is one [`TAG_KEY_PATTERN`]'s character class
/// admits: an ASCII letter, `_`, or a character of the `+`..`=` range that class
/// spells (`+ , - . / 0-9 : ; < =`).
fn tag_key_characters_allowed(key: &str) -> bool {
    key.chars()
        .all(|c| matches!(c, 'a'..='z' | 'A'..='Z' | '_' | '+'..='='))
}

/// The longest caller-supplied text a refusal message echoes, pinned to the boundary
/// validator's own ceiling (`MAX_ECHOED_VALUE` in
/// `aws_smithy_schema_server::constraints`), so a huge request member cannot be
/// amplified into a huge response.
///
/// The framework's own eliding helper is private and lives under the agent-immutable
/// `vendor/` tree, so it cannot be called from here; the ceiling is pinned to the same
/// figure instead.
const MAX_ECHOED_VALUE: usize = 256;

/// The most clauses one refusal message carries, pinned to the boundary validator's
/// own ceiling (`MAX_VIOLATIONS`): a request may carry arbitrarily many tag keys, and
/// a message that named every bad one would grow with the request.
const MAX_REPORTED_VIOLATIONS: usize = 100;

/// `text` as a refusal message may carry it: itself when short, a
/// character-boundary-safe prefix with an elision when not.
fn elided(text: &str) -> String {
    if text.len() <= MAX_ECHOED_VALUE {
        return text.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &text[..end], text.len())
}

/// Why the tag keys at `member` are a bad request, or `None` when every one of them is
/// a key `TagKey` admits.
///
/// Tag keys reach a handler UNCHECKED, for two reasons that compound: the boundary
/// walk captures a map's key as the path segment of that entry's value rather than
/// validating it as a leaf of its own, and `TagKey`'s `@pattern` carries a leading
/// negative lookahead the pattern engine cannot compile, which it then treats as
/// matching. So this is the one place the key shape's `@length` and `@pattern` are
/// enforced, shared by every operation that takes tag keys, and the sentence is built
/// from the framework's own [`::smithy_server_runtime::Violation`] and rendered by
/// `aws_smithy_schema_server::constraints::envelope` — the one place that wording
/// lives — so a handler refusal and a boundary refusal read identically.
pub fn tag_keys_violation<'k>(
    member: &str,
    keys: impl IntoIterator<Item = &'k str>,
) -> Option<String> {
    // Sorted and deduplicated: a `HashMap` iterates in an arbitrary order, and the
    // same request must be refused with the same sentence every time.
    let mut keys: Vec<&str> = keys.into_iter().collect();
    keys.sort_unstable();
    keys.dedup();

    let violations: Vec<::smithy_server_runtime::Violation> = keys
        .into_iter()
        .filter_map(|key| {
            let length = key.chars().count();
            let kind = if length < TAG_KEY_MIN_LENGTH {
                ::smithy_server_runtime::ViolationKind::MinLength(TAG_KEY_MIN_LENGTH as i64)
            } else if length > TAG_KEY_MAX_LENGTH {
                ::smithy_server_runtime::ViolationKind::MaxLength(TAG_KEY_MAX_LENGTH as i64)
            } else if key.starts_with(TAG_KEY_RESERVED_PREFIX) || !tag_key_characters_allowed(key) {
                ::smithy_server_runtime::ViolationKind::Pattern(TAG_KEY_PATTERN.to_string())
            } else {
                return None;
            };
            Some(::smithy_server_runtime::Violation {
                // The path names the offending key, which is what identifies it in a
                // tag map as in a tag-key list. Elided like the value: it embeds
                // caller-supplied text, so it is the same amplification vector.
                path: elided(&format!(
                    "{member}.{}",
                    ::aws_smithy_schema_server::constraints::path::entry(key)
                )),
                kind,
                // `Violation::value` must arrive ALREADY elided: the message
                // interpolates it verbatim.
                value: Some(elided(key)),
            })
        })
        .take(MAX_REPORTED_VIOLATIONS)
        .collect();

    ::aws_smithy_schema_server::constraints::envelope(&violations)
}

/// Why leaving the resource at `member` holding `count` tags is a bad request, or
/// `None` when that many is within [`MAX_TAGS_PER_RESOURCE`].
///
/// The one place that ceiling is enforced, so every path that can leave a resource
/// holding tags — the create that is born with a tag map, the merge that adds to one —
/// refuses at the same count and with the same sentence, and no resource can come to
/// hold a map no tag call could have produced.
pub fn tag_count_violation(member: &str, count: usize) -> Option<String> {
    if count <= MAX_TAGS_PER_RESOURCE {
        return None;
    }
    let violation = ::smithy_server_runtime::Violation {
        path: member.to_string(),
        kind: ::smithy_server_runtime::ViolationKind::MaxLength(MAX_TAGS_PER_RESOURCE as i64),
        // The `<n> entries` form the framework displays a map's own length with.
        value: Some(format!("{count} entries")),
    };
    ::aws_smithy_schema_server::constraints::envelope(std::slice::from_ref(&violation))
}

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
/// (`CreateCampaign`, `UpdateCampaignCommunicationLimits`,
/// `PutInstanceCommunicationLimits`) refuses an over-long list the same way. `member`
/// is the request path the refusal names, so the message is built from the request
/// rather than fixed text.
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

/// Whether the instant `deadline` has passed at `now` — compared exactly, through
/// [`instant_parts`], so the comparison is not rounded to whole seconds.
///
/// The one place a deadline a caller sent is weighed against the clock, so every
/// reader of a stored deadline draws the line at the same instant.
fn deadline_passed(deadline: ::aws_smithy_types::DateTime, now: SystemTime) -> bool {
    instant_parts(::aws_smithy_types::DateTime::from(now)) >= instant_parts(deadline)
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
    /// The outbound requests this campaign has pending, keyed by the `clientToken`
    /// each was submitted under — the idempotency token `PutOutboundRequestBatch`
    /// ("This API is idempotent", per the model's own documentation) resolves a
    /// repeated submission through, so the same token is answered with the dial
    /// request id it was first given rather than a second one.
    ///
    /// A request leaves this buffer once its `expirationTime` has passed, which is
    /// owed to the clock alone ([`Self::expire_outbound_requests`]): an expired
    /// request is no longer dialable, so it neither occupies a buffer slot nor keeps
    /// answering for its token.
    pub outbound_requests: BTreeMap<String, StoredOutboundRequest>,
    /// The profile outbound requests this campaign has pending, keyed by the
    /// `clientToken` each was submitted under — the idempotency token
    /// `PutProfileOutboundRequestBatch` ("This API is idempotent", per the model's own
    /// documentation) resolves a repeated submission through, so the same token is
    /// answered with the `ProfileOutboundRequestId` it was first given rather than a
    /// second one.
    ///
    /// A buffer of its own beside [`Self::outbound_requests`], because the two carry
    /// different resources: a profile request names a Customer Profiles profile and its
    /// expiration is OPTIONAL, where a dial request names a channel and must carry one.
    /// A request whose `expirationTime` has passed leaves this buffer for the same
    /// reason ([`Self::expire_profile_outbound_requests`]): it can no longer be placed,
    /// so it neither occupies a buffer slot nor keeps answering for its token.
    pub profile_outbound_requests: BTreeMap<String, StoredProfileOutboundRequest>,
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
    /// The `expirationTime` the caller sent: the instant after which this request is
    /// no longer dialable, which is why `OutboundRequest$expirationTime` is required.
    /// A record past it is no longer pending (see [`Self::is_expired_at`]).
    pub expires_at: ::aws_smithy_types::DateTime,
}

impl StoredOutboundRequest {
    /// Whether this request's `expirationTime` has passed at `now`, so it is no
    /// longer dialable and no longer one of the campaign's pending requests.
    pub fn is_expired_at(&self, now: SystemTime) -> bool {
        deadline_passed(self.expires_at, now)
    }
}

/// A profile outbound request a campaign has taken, as stored.
#[derive(Debug, Clone)]
pub struct StoredProfileOutboundRequest {
    /// The `ProfileOutboundRequestId` this request was given. Minted from the store's
    /// own counter, never from anything in the request, and within
    /// `ProfileOutboundRequestId`'s pattern `^[a-zA-Z0-9_\-.]*$` and its 0..256 length
    /// bound.
    pub id: String,
    /// The `profileId` the request was submitted for: the Customer Profiles profile the
    /// campaign is to reach.
    pub profile_id: String,
    /// The `expirationTime` the caller sent, when it sent one: the instant after which
    /// this request can no longer be placed. `ProfileOutboundRequest$expirationTime` is
    /// OPTIONAL, so a request that carries none never expires and stays pending until
    /// the campaign goes (see [`Self::is_expired_at`]).
    pub expires_at: Option<::aws_smithy_types::DateTime>,
    /// The `eventTriggerContext$sourceEvent` the request carried, when it carried one:
    /// the event this request was triggered by. Part of the record, not of any response
    /// — no modelled member of this operation's output carries it.
    pub source_event: Option<String>,
}

impl StoredProfileOutboundRequest {
    /// Whether this request's `expirationTime` has passed at `now`, so it can no longer
    /// be placed and is no longer one of the campaign's pending profile requests. A
    /// request submitted without an expiration has no instant to pass, so it never
    /// expires.
    pub fn is_expired_at(&self, now: SystemTime) -> bool {
        self.expires_at
            .is_some_and(|expires_at| deadline_passed(expires_at, now))
    }
}

/// What a campaign's store did with one outbound request of a batch, as
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::take_outbound_request`]
/// and
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::take_profile_outbound_request`]
/// both report it: the two batch operations decide a request's fate the same way, so
/// they share one outcome rather than each carrying a copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundRequestOutcome {
    /// The request is in the campaign's buffer under this id (a `DialRequestId` or a
    /// `ProfileOutboundRequestId`) — either freshly minted, or the id an earlier
    /// submission under the same `clientToken` was given, in which case this repeat
    /// stored nothing new.
    Taken(String),
    /// The campaign's buffer already holds [`MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN`]
    /// pending requests, so nothing was stored.
    BufferFull,
    /// The request's `expirationTime` has already passed, so there is no call left to
    /// place for it: nothing was stored and no id was minted.
    Expired,
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

    /// Drop every request this campaign's buffer holds whose `expirationTime` has
    /// passed at `now`, and report how many went.
    ///
    /// The buffer is a buffer of PENDING requests: a request past its expiration is
    /// no longer dialable, so it is no longer pending — it holds neither one of the
    /// [`MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN`] slots nor its own `clientToken`. The
    /// removal is owed to the CLOCK, not to the call that materialises it: it was due
    /// at the expiration instant whether or not anyone called afterwards.
    pub fn expire_outbound_requests(&mut self, now: SystemTime) -> usize {
        let before = self.outbound_requests.len();
        self.outbound_requests
            .retain(|_, request| !request.is_expired_at(now));
        before - self.outbound_requests.len()
    }

    /// The requests this campaign still has pending at `now` — everything in its
    /// buffer that has not passed its `expirationTime`. A pure read, so a reader
    /// counting the buffer writes nothing.
    pub fn pending_outbound_requests(&self, now: SystemTime) -> usize {
        self.outbound_requests
            .values()
            .filter(|request| !request.is_expired_at(now))
            .count()
    }

    /// Drop every profile outbound request this campaign's buffer holds whose
    /// `expirationTime` has passed at `now`, and report how many went.
    ///
    /// The same rule as [`Self::expire_outbound_requests`], on the buffer of profile
    /// requests: a request past its expiration can no longer be placed, so it is no
    /// longer pending and holds neither one of the
    /// [`MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN`] slots nor its own `clientToken`. The
    /// removal is owed to the CLOCK, not to the call that materialises it. A request
    /// submitted without an expiration never becomes due.
    pub fn expire_profile_outbound_requests(&mut self, now: SystemTime) -> usize {
        let before = self.profile_outbound_requests.len();
        self.profile_outbound_requests
            .retain(|_, request| !request.is_expired_at(now));
        before - self.profile_outbound_requests.len()
    }

    /// The profile outbound requests this campaign still has pending at `now` —
    /// everything in that buffer which has not passed its `expirationTime`. A pure
    /// read, so a reader counting the buffer writes nothing.
    pub fn pending_profile_outbound_requests(&self, now: SystemTime) -> usize {
        self.profile_outbound_requests
            .values()
            .filter(|request| !request.is_expired_at(now))
            .count()
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
/// go in: never a stored encryption, integration or communication-limits config, a
/// key or resource ARN or any credential material.
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

/// The `CampaignDeletionPolicy` value that has `DeleteConnectInstanceConfig` remove
/// the instance's existing campaigns along with its configuration, as the model's
/// catalog declares it (see `CAMPAIGN_DELETION_POLICY_VALUES` in
/// [`crate::ops::types`]).
pub const CAMPAIGN_DELETION_POLICY_DELETE_ALL: &str = "DELETE_ALL";

/// The `CampaignDeletionPolicy` value that retains the instance's existing
/// campaigns, from the same catalog: the policy that enacts nothing on them, and so
/// also how a request that names no policy at all is served — the conservative
/// reading, since a caller that did not ask for the campaigns to go keeps them.
pub const CAMPAIGN_DELETION_POLICY_RETAIN_ALL: &str = "RETAIN_ALL";

/// What this service answers when a `connectInstanceId` names no Connect instance
/// onboarded to Campaigns: there is no instance configuration to report, nor any to
/// configure.
///
/// Composed from the request, so the refusal names the instance the caller asked
/// about. The one text every instance-scoped operation shares, so two of them cannot
/// disagree about an instance this service holds nothing for.
pub fn instance_config_not_found_message(connect_instance_id: &str) -> String {
    format!("ConnectInstanceConfig not found {connect_instance_id}")
}

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
/// `StartInstanceOnboardingJob` call persisted, the instant its job is due to
/// have finished, and the instance-wide configuration set on it since.
#[derive(Debug, Clone)]
pub struct StoredInstanceOnboarding {
    pub connect_instance_id: String,
    /// What the instance's campaign data is encrypted with, exactly as the call
    /// that started the job supplied it.
    pub encryption_config: EncryptionConfig,
    /// The instant this job reaches `SUCCEEDED`: the start instant plus
    /// [`ONBOARDING_JOB_DURATION`]. Before it the job reports `IN_PROGRESS`.
    pub succeeds_at: std::time::SystemTime,
    /// The instance-wide communication limits `PutInstanceCommunicationLimits`
    /// persisted for this instance, exactly as that call supplied them, and `None`
    /// while no call has set any. `GetInstanceCommunicationLimits` reports this
    /// member and nothing else, so the two cannot disagree. It lives on the
    /// instance's record, so an instance whose record goes takes its limits with it.
    pub communication_limits: Option<InstanceCommunicationLimitsConfig>,
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
            communication_limits: None,
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

/// The most outbound requests one campaign's store holds in a buffer, after which a
/// further request in a batch is reported as a per-request failure carrying the code
/// the model declares for a campaign that cannot take another request right now:
/// `FailureCode` `BufferLimitExceeded` for a dial request, and
/// `ProfileOutboundRequestFailureCode` `Conflict` for a profile request, whose catalog
/// carries no buffer code and whose nearest entry is the one for a refusal the
/// resource's own state causes.
///
/// A campaign's buffer grows one record per accepted request and shrinks only as
/// requests pass their `expirationTime`, so without a ceiling a caller could submit
/// batches of long-lived requests until the process runs out of memory. The figure is
/// the emulator's own, set far above any plausible workload. It counts the PENDING
/// requests: an expired one is not in the buffer any more (see
/// [`StoredCampaign::expire_outbound_requests`] and
/// [`StoredCampaign::expire_profile_outbound_requests`]). Each buffer is counted on
/// its own, since each holds its own resource.
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

/// The most regions this store holds records for at once, counted per region and not
/// per record: past it a call that would give a further region its first resource is
/// refused exactly as a full store is, and nothing is written.
///
/// The region of a request is its SigV4 credential scope, which the caller chooses,
/// so without a ceiling a caller could repeat a write under fresh region names until
/// the process runs out of memory, each one carrying its own set of the stores above.
/// A partition's regions are a fixed, published set; AWS publishes no limit of this
/// kind, so the figure is the emulator's own, set well above the largest partition.
pub const MAX_REGIONS: usize = 64;

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

/// The identity `identifier` names: the kind of integration, and the ARN of the
/// resource it integrates (see [`IntegrationKind`]).
///
/// The `IntegrationIdentifier` counterpart of [`integration_key`]: the model declares
/// the same kind-and-ARN pair in both the config a put carries and the identifier a
/// delete carries, and reading it in one place is what keeps the key a put stores
/// under and the key a delete resolves from disagreeing.
pub fn integration_identifier_key(
    identifier: &IntegrationIdentifier,
) -> (IntegrationKind, &str) {
    match identifier {
        IntegrationIdentifier::customerProfiles(identifier) => (
            IntegrationKind::CustomerProfiles,
            identifier.domain_arn.as_str(),
        ),
        IntegrationIdentifier::qConnect(identifier) => (
            IntegrationKind::QConnect,
            identifier.knowledge_base_arn.as_str(),
        ),
        IntegrationIdentifier::Lambda(identifier) => {
            (IntegrationKind::Lambda, identifier.function_arn.as_str())
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

/// Which campaign a call is about: the region whose endpoint holds it, and its id
/// within that region.
///
/// A campaign is identified by the PAIR — the same id in two regions names two
/// different resources — so the methods that take a campaign's id alongside several
/// values of its own take this instead of two loose strings.
#[derive(Debug, Clone, Copy)]
pub struct CampaignRef<'a> {
    pub region: &'a str,
    pub id: &'a str,
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

/// The records of ONE region: everything a client talking to that region's endpoint
/// can see or change.
///
/// `campaigns` answers a `{id}` lookup in O(1); `order` carries the creation order as
/// `(sequence number, id)` pairs, which the methods below keep in step with the
/// map, so `ListCampaigns` paginates deterministically and its `nextToken` can
/// name the sequence number the last page ended at. Growth is bounded:
/// [`MAX_CAMPAIGNS`] records, enforced by [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::insert_new_campaign`].
///
/// `instance_onboarding` is the second map: one onboarding record per Connect
/// instance, keyed by its instance id, bounded at [`MAX_ONBOARDED_INSTANCES`] by
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::start_instance_onboarding`].
/// It is the one place "is this instance a campaign target?" is answered, so
/// `CreateCampaign`'s gate and `GetConnectInstanceConfig` /
/// `GetInstanceOnboardingJobStatus` cannot disagree.
///
/// `instance_integrations` is the third: the integrations of each Connect instance,
/// which `PutConnectInstanceIntegration` writes and
/// `ListConnectInstanceIntegrations` projects, so the list reports what the puts
/// stored and nothing else.
#[derive(Debug, Default)]
struct RegionStore {
    campaigns: HashMap<String, StoredCampaign>,
    order: Vec<(u64, String)>,
    instance_onboarding: HashMap<String, StoredInstanceOnboarding>,
    instance_integrations: HashMap<String, Vec<StoredIntegration>>,
}

/// Per-service state for com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2. A handler reaches it through [`service_state`].
///
/// A campaign and a Connect instance configuration are REGIONAL resources, reachable
/// only through the endpoint of the region they were created in — which the ARN this
/// service mints states ([`campaign_arn`]) — so the records live in one
/// [`RegionStore`] per region, keyed by the credential-scope region of the request
/// that wrote them ([`request_region`]). Every method below takes that region and
/// resolves inside its store alone: a resource another region holds is absent from a
/// page here and its id resolves to nothing, so no caller sees or changes a resource
/// its endpoint does not serve. A region nothing has been written to yet has no entry,
/// which every reader answers exactly as it answers an empty one, so reading a region
/// never creates it.
///
/// The sequence counters stay store-wide: an id or cursor value is minted from one
/// counter whatever region takes it, so no two campaigns, dial requests, profile
/// requests or integrations anywhere in the store share one.
#[derive(Debug, Default)]
pub struct ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State {
    /// The records of each region, keyed by the credential-scope region they belong
    /// to. Bounded at [`MAX_REGIONS`] regions, enforced by [`Self::region_mut`] — the
    /// one path that gives a region its first record.
    regions: HashMap<String, RegionStore>,
    next_sequence: u64,
    next_dial_sequence: u64,
    /// The counter the `ProfileOutboundRequestId`s are minted from, store-wide so no
    /// two profile outbound requests share an id (see
    /// [`Self::take_profile_outbound_request`]).
    next_profile_request_sequence: u64,
    next_integration_sequence: u64,
}

impl ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State {
    /// The records `region` holds, or `None` when nothing has been written to that
    /// region yet.
    ///
    /// The read path: a region with no entry holds no resources, which every reader
    /// answers exactly as it answers an empty store, so a read never adds an entry and
    /// never changes what a later caller sees.
    fn region(&self, region: &str) -> Option<&RegionStore> {
        self.regions.get(region)
    }

    /// The records `region` holds, for a WRITER — created empty when that region has
    /// none yet, since the write is what gives the region its first resource.
    ///
    /// `None` when the region has no records and the store already holds
    /// [`MAX_REGIONS`] regions: the caller chooses the region it signs for, so a new
    /// region is taken only while there is room for one, and a refusal creates
    /// nothing. A region already in the store is always handed back.
    fn region_mut(&mut self, region: &str) -> Option<&mut RegionStore> {
        if !self.regions.contains_key(region) && self.regions.len() >= MAX_REGIONS {
            return None;
        }
        Some(self.regions.entry(region.to_string()).or_default())
    }

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
        region: &str,
        campaign: StoredCampaign,
    ) -> Result<StoredCampaign, InsertRejected> {
        // A campaign name identifies a campaign within its Connect instance, so a
        // record whose (instance, name) pair is already held is refused before any
        // capacity question: the resource it names exists, and taking a second one
        // would leave the instance with two campaigns answering to one name. This is
        // the one place that invariant is enforced, so every writer of a campaign
        // name upholds it.
        if self
            .campaign_named(region, &campaign.connect_instance_id, &campaign.name)
            .is_some()
        {
            return Err(InsertRejected::NameTaken);
        }
        let sequence = self.next_sequence;
        let outcome = {
            // Past the region ceiling there is no room for a region's first campaign,
            // which the caller answers exactly as it answers a full store.
            let Some(store) = self.region_mut(region) else {
                return Err(InsertRejected::StoreFull);
            };
            let full = store.campaigns.len() >= MAX_CAMPAIGNS;
            let id = campaign.id.clone();
            match store.campaigns.entry(id.clone()) {
                // Taking this campaign would discard the one already under that id.
                std::collections::hash_map::Entry::Occupied(_) => Err(InsertRejected::IdTaken),
                std::collections::hash_map::Entry::Vacant(_) if full => {
                    Err(InsertRejected::StoreFull)
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    let stored = slot.insert(campaign).clone();
                    // Only now, with the record actually in the map, does the order
                    // gain an entry and a sequence number — so "a refusal stores
                    // nothing" holds structurally on every arm above.
                    store.order.push((sequence, id));
                    Ok(stored)
                }
            }
        };
        if outcome.is_ok() {
            self.next_sequence = sequence.saturating_add(1);
        }
        outcome
    }

    /// The campaign `region` stores under `id`, or `None` when that region stores
    /// none — including when another region stores one under that id, which is a
    /// different resource and not this endpoint's to answer for.
    pub fn campaign(&self, region: &str, id: &str) -> Option<&StoredCampaign> {
        self.region(region)?.campaigns.get(id)
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
        region: &str,
        connect_instance_id: &str,
        name: &str,
    ) -> Option<&StoredCampaign> {
        self.campaigns_in_order(region)
            .map(|(_, campaign)| campaign)
            .find(|campaign| {
                campaign.connect_instance_id == connect_instance_id && campaign.name == name
            })
    }

    /// The campaign `region` stores under `id`, for a handler that moves its state.
    /// Resolved exactly as [`Self::campaign`] resolves it, so a lifecycle write
    /// reaches only the region it was signed for.
    pub fn campaign_mut(&mut self, region: &str, id: &str) -> Option<&mut StoredCampaign> {
        self.regions.get_mut(region)?.campaigns.get_mut(id)
    }

    /// Remove the campaign `region` stores under `id`, returning it when it was there.
    pub fn remove_campaign(&mut self, region: &str, id: &str) -> Option<StoredCampaign> {
        let store = self.regions.get_mut(region)?;
        let removed = store.campaigns.remove(id)?;
        store.order.retain(|(_, stored)| stored != id);
        Some(removed)
    }

    /// The campaign of `region` whose ARN is `arn`, or `None` when that region stores
    /// no campaign carrying it.
    ///
    /// Resolved in the region the request was signed for, and then the whole ARN is
    /// compared, not just the id it ends with: an ARN names a partition, a region and
    /// an account as well as a resource, so one naming a different region or account
    /// names a different resource and resolves to nothing here.
    pub fn campaign_by_arn(&self, region: &str, arn: &str) -> Option<&StoredCampaign> {
        let id = campaign_id_from_arn(arn)?;
        self.campaign(region, id)
            .filter(|stored| stored.arn == arn)
    }

    /// The campaign whose ARN is `arn`, for a handler that writes to it. Resolved
    /// exactly as [`Self::campaign_by_arn`] resolves it.
    pub fn campaign_by_arn_mut(&mut self, region: &str, arn: &str) -> Option<&mut StoredCampaign> {
        let id = campaign_id_from_arn(arn)?.to_string();
        self.campaign_mut(region, &id)
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
    /// the campaign `campaign` names, and report what became of it (see
    /// [`OutboundRequestOutcome`]). `None` when that campaign's region stores no
    /// campaign under that id.
    ///
    /// A `client_token` the campaign has already taken stores nothing and reports
    /// the id it was given then, which is what makes a repeated batch idempotent.
    ///
    /// `expires_at` is weighed against `now` first: a request whose expiration has
    /// already passed can never be dialled, so it is not taken at all, mints no id
    /// and occupies no slot. The expiries the clock already owes the campaign's
    /// buffer are materialised at the same instant
    /// ([`StoredCampaign::expire_outbound_requests`]), so a slot and a `clientToken`
    /// are both held only while their request is still pending.
    pub fn take_outbound_request(
        &mut self,
        campaign: CampaignRef<'_>,
        client_token: &str,
        channel_subtype: &str,
        expires_at: ::aws_smithy_types::DateTime,
        now: SystemTime,
    ) -> Option<OutboundRequestOutcome> {
        {
            let stored = self.campaign_mut(campaign.region, campaign.id)?;
            stored.expire_outbound_requests(now);
            if deadline_passed(expires_at, now) {
                return Some(OutboundRequestOutcome::Expired);
            }
            if let Some(taken) = stored.outbound_requests.get(client_token) {
                return Some(OutboundRequestOutcome::Taken(taken.id.clone()));
            }
            if stored.outbound_requests.len() >= MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN {
                return Some(OutboundRequestOutcome::BufferFull);
            }
        }
        // Only now, past every refusal, is an id minted — so a refused request
        // consumes no id and leaves no gap in the sequence.
        let id = self.next_dial_request_id();
        let stored = self.campaign_mut(campaign.region, campaign.id)?;
        stored.outbound_requests.insert(
            client_token.to_string(),
            StoredOutboundRequest {
                id: id.clone(),
                channel_subtype: channel_subtype.to_string(),
                expires_at,
            },
        );
        Some(OutboundRequestOutcome::Taken(id))
    }

    /// The id the next profile outbound request this store takes will carry: a
    /// store-wide sequence number, so no two profile requests of any campaign share an
    /// id, and none shares one with a dial request.
    ///
    /// Minted from the store's own counter, never from anything in the request. The
    /// text is within `ProfileOutboundRequestId`'s pattern `^[a-zA-Z0-9_\-.]*$` and its
    /// 0..256 length bound.
    fn next_profile_outbound_request_id(&mut self) -> String {
        let sequence = self.next_profile_request_sequence;
        self.next_profile_request_sequence = self.next_profile_request_sequence.saturating_add(1);
        format!("profile-{sequence:012}")
    }

    /// Take the profile outbound request submitted under `client_token` into the
    /// profile buffer of the campaign `campaign` names, and report what became of it
    /// (see [`OutboundRequestOutcome`]). `None` when that campaign's region stores no
    /// campaign under that id.
    ///
    /// Decided exactly as [`Self::take_outbound_request`] decides a dial request, on
    /// the buffer of profile requests: a `client_token` the campaign has already taken
    /// stores nothing and reports the id it was given then (what makes a repeated batch
    /// idempotent), an `expires_at` the clock has passed is not taken at all, mints no
    /// id and occupies no slot, and the expiries the clock already owes the buffer are
    /// materialised at the same instant
    /// ([`StoredCampaign::expire_profile_outbound_requests`]).
    ///
    /// `expires_at` is optional, as `ProfileOutboundRequest$expirationTime` is: a
    /// request that carries none has no instant to have passed, so it is always taken.
    pub fn take_profile_outbound_request(
        &mut self,
        campaign: CampaignRef<'_>,
        client_token: &str,
        profile_id: &str,
        expires_at: Option<::aws_smithy_types::DateTime>,
        source_event: Option<&str>,
        now: SystemTime,
    ) -> Option<OutboundRequestOutcome> {
        {
            let stored = self.campaign_mut(campaign.region, campaign.id)?;
            stored.expire_profile_outbound_requests(now);
            if expires_at.is_some_and(|expires_at| deadline_passed(expires_at, now)) {
                return Some(OutboundRequestOutcome::Expired);
            }
            if let Some(taken) = stored.profile_outbound_requests.get(client_token) {
                return Some(OutboundRequestOutcome::Taken(taken.id.clone()));
            }
            if stored.profile_outbound_requests.len() >= MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN {
                return Some(OutboundRequestOutcome::BufferFull);
            }
        }
        // Only now, past every refusal, is an id minted — so a refused request
        // consumes no id and leaves no gap in the sequence.
        let id = self.next_profile_outbound_request_id();
        let stored = self.campaign_mut(campaign.region, campaign.id)?;
        stored.profile_outbound_requests.insert(
            client_token.to_string(),
            StoredProfileOutboundRequest {
                id: id.clone(),
                profile_id: profile_id.to_string(),
                expires_at,
                source_event: source_event.map(ToString::to_string),
            },
        );
        Some(OutboundRequestOutcome::Taken(id))
    }

    /// The onboarding record `region` holds for `connect_instance_id` when that
    /// instance is onboarded to Campaigns there at `now` — a record exists in that
    /// region AND its job has reached `SUCCEEDED` — and `None` otherwise. An instance
    /// whose job is still `IN_PROGRESS` is not a campaign target yet, and neither is
    /// one onboarded only in another region.
    ///
    /// The one place "is this instance onboarded?" is decided, so `CreateCampaign`'s
    /// gate ([`Self::is_onboarded`]) and `GetConnectInstanceConfig`'s projection
    /// cannot disagree. A pure read of the stored due instant (see
    /// [`StoredInstanceOnboarding::status_at`]), so neither writes anything.
    pub fn onboarded_instance(
        &self,
        region: &str,
        connect_instance_id: &str,
        now: SystemTime,
    ) -> Option<&StoredInstanceOnboarding> {
        self.instance_onboarding(region, connect_instance_id)
            .filter(|record| record.status_at(now) == ONBOARDING_STATUS_SUCCEEDED)
    }

    /// Whether `region` has `connect_instance_id` onboarded to Campaigns at `now`, as
    /// [`Self::onboarded_instance`] decides it.
    pub fn is_onboarded(&self, region: &str, connect_instance_id: &str, now: SystemTime) -> bool {
        self.onboarded_instance(region, connect_instance_id, now)
            .is_some()
    }

    /// The record [`Self::onboarded_instance`] resolves, mutably — for a caller that
    /// configures the onboarded instance. The same predicate decides it, so an
    /// instance that is not a campaign target yet is not configurable either.
    fn onboarded_instance_mut(
        &mut self,
        region: &str,
        connect_instance_id: &str,
        now: SystemTime,
    ) -> Option<&mut StoredInstanceOnboarding> {
        self.regions
            .get_mut(region)?
            .instance_onboarding
            .get_mut(connect_instance_id)
            .filter(|record| record.status_at(now) == ONBOARDING_STATUS_SUCCEEDED)
    }

    /// Replace the instance-wide communication limits `region` holds for
    /// `connect_instance_id` with `config`, and report whether the store took it.
    ///
    /// `false` means the instance is not onboarded in that region at `now`, so there
    /// is no instance configuration to put the limits on and nothing was written.
    /// Writing the whole config rather than merging into it is what makes the put
    /// idempotent: the same call twice leaves the same config stored.
    pub fn put_instance_communication_limits(
        &mut self,
        region: &str,
        connect_instance_id: &str,
        config: InstanceCommunicationLimitsConfig,
        now: SystemTime,
    ) -> bool {
        match self.onboarded_instance_mut(region, connect_instance_id, now) {
            Some(record) => {
                record.communication_limits = Some(config);
                true
            }
            None => false,
        }
    }

    /// Every campaign `region` stores, in creation order, each with its creation
    /// sequence number — the value a `ListCampaigns` cursor is built from. A region
    /// that stores none yields nothing.
    pub fn campaigns_in_order<'a>(
        &'a self,
        region: &str,
    ) -> impl Iterator<Item = (u64, &'a StoredCampaign)> + 'a {
        self.region(region).into_iter().flat_map(|store| {
            store
                .order
                .iter()
                .filter_map(|(sequence, id)| store.campaigns.get(id).map(|c| (*sequence, c)))
        })
    }

    /// Start the onboarding job `region` holds for `connect_instance_id` with
    /// `encryption_config`, and hand back the record now in the store.
    ///
    /// An instance with no record in that region gets a job that starts at `now` —
    /// including one already onboarded in another region, whose record is another
    /// region's resource. An instance that already has one there keeps that job —
    /// repeating the call does not re-onboard an instance, which is what makes the
    /// operation idempotent — and takes the request's encryption config, since the
    /// call carries the instance's config.
    ///
    /// `None` means the region already holds [`MAX_ONBOARDED_INSTANCES`] instances
    /// and nothing was written.
    pub fn start_instance_onboarding(
        &mut self,
        region: &str,
        connect_instance_id: &str,
        encryption_config: EncryptionConfig,
        now: std::time::SystemTime,
    ) -> Option<StoredInstanceOnboarding> {
        // Past the region ceiling there is no room for a region's first onboarding
        // record, which the caller answers exactly as it answers a full store.
        let store = self.region_mut(region)?;
        if let Some(existing) = store.instance_onboarding.get_mut(connect_instance_id) {
            existing.encryption_config = encryption_config;
            return Some(existing.clone());
        }
        if store.instance_onboarding.len() >= MAX_ONBOARDED_INSTANCES {
            return None;
        }
        let record = StoredInstanceOnboarding::starting(
            connect_instance_id.to_string(),
            encryption_config,
            now,
        );
        store
            .instance_onboarding
            .insert(connect_instance_id.to_string(), record.clone());
        Some(record)
    }

    /// The onboarding record `region` holds for `connect_instance_id`, or `None` when
    /// the instance has never been onboarded there.
    pub fn instance_onboarding(
        &self,
        region: &str,
        connect_instance_id: &str,
    ) -> Option<&StoredInstanceOnboarding> {
        self.region(region)?
            .instance_onboarding
            .get(connect_instance_id)
    }

    /// Delete the onboarding job `region` holds for `connect_instance_id`, and report
    /// what became of the request (see [`DeleteOnboardingOutcome`]).
    ///
    /// Only a job that has FAILED may be deleted: a job in any other status keeps
    /// its record, so the status a later `GetInstanceOnboardingJobStatus` reports
    /// and the config `GetConnectInstanceConfig` projects are untouched by a refused
    /// delete. The status is read off the stored due instant at `now`, the same way
    /// every other reader of the record reads it.
    pub fn delete_instance_onboarding(
        &mut self,
        region: &str,
        connect_instance_id: &str,
        now: SystemTime,
    ) -> DeleteOnboardingOutcome {
        let Some(record) = self.instance_onboarding(region, connect_instance_id) else {
            return DeleteOnboardingOutcome::NoSuchJob;
        };
        if record.status_at(now) != ONBOARDING_STATUS_FAILED {
            return DeleteOnboardingOutcome::NotFailed;
        }
        if let Some(store) = self.regions.get_mut(region) {
            store.instance_onboarding.remove(connect_instance_id);
        }
        DeleteOnboardingOutcome::Deleted
    }

    /// Remove every campaign `region` stores for `connect_instance_id`, in creation
    /// order, leaving every other instance's campaigns — and every other region's —
    /// where they are.
    ///
    /// Each record goes through [`Self::remove_campaign`], so the creation order
    /// loses exactly the entries the map does and a later page walks what is left.
    fn remove_instance_campaigns(&mut self, region: &str, connect_instance_id: &str) {
        let ids: Vec<String> = self
            .campaigns_in_order(region)
            .filter(|(_, campaign)| campaign.connect_instance_id == connect_instance_id)
            .map(|(_, campaign)| campaign.id.clone())
            .collect();
        for id in &ids {
            self.remove_campaign(region, id);
        }
    }

    /// Delete the instance configuration `region` holds for `connect_instance_id`,
    /// and report whether there was one to delete.
    ///
    /// `false` means the instance holds no configuration in that region at `now` — it
    /// was never onboarded there, its onboarding job has not finished, or its
    /// configuration is already gone — so nothing was written, and the caller answers
    /// as not found.
    ///
    /// Deleting the configuration takes with it everything held ON that
    /// configuration: the instance's onboarding record (so the instance is no longer
    /// a campaign target and no reader reports a config for it), the instance-wide
    /// communication limits that record carries, and the instance's integrations —
    /// which belong to the configured instance, so an instance onboarded again later
    /// starts out holding none.
    ///
    /// `delete_campaigns` is what the request's campaign deletion policy decided for
    /// the campaigns of the instance: `true` removes them with the configuration,
    /// `false` retains them exactly as they are.
    pub fn delete_instance_config(
        &mut self,
        region: &str,
        connect_instance_id: &str,
        delete_campaigns: bool,
        now: SystemTime,
    ) -> bool {
        if self
            .onboarded_instance(region, connect_instance_id, now)
            .is_none()
        {
            return false;
        }
        if delete_campaigns {
            self.remove_instance_campaigns(region, connect_instance_id);
        }
        // The region holds the record the check above resolved, so it is in the map.
        let Some(store) = self.regions.get_mut(region) else {
            return false;
        };
        store.instance_onboarding.remove(connect_instance_id);
        store.instance_integrations.remove(connect_instance_id);
        true
    }

    /// Put `config` as an integration `region` holds for `connect_instance_id`, and
    /// hand back the record now in the store — so a handler reports what the store
    /// holds rather than what the request carried.
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
        region: &str,
        connect_instance_id: &str,
        config: IntegrationConfig,
    ) -> Option<StoredIntegration> {
        let (kind, arn) = {
            let (kind, arn) = integration_key(&config);
            (kind, arn.to_string())
        };
        let sequence = self.next_integration_sequence;
        // Past the region ceiling there is no room for a region's first integration,
        // which the caller answers exactly as it answers a full store.
        let store = self.region_mut(region)?;
        let held = store
            .instance_integrations
            .get(connect_instance_id)
            .map_or(0, Vec::len);
        if let Some(existing) = store
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
        let record = StoredIntegration { sequence, config };
        store
            .instance_integrations
            .entry(connect_instance_id.to_string())
            .or_default()
            .push(record.clone());
        self.next_integration_sequence = sequence.saturating_add(1);
        Some(record)
    }

    /// Every integration `region` holds for `connect_instance_id`, in the order they
    /// were first put.
    ///
    /// An instance that holds none yields nothing, which is not the same as an
    /// instance that is not onboarded: whether the instance exists at all is
    /// [`Self::is_onboarded`]'s answer, not this one's.
    pub fn instance_integrations<'a>(
        &'a self,
        region: &str,
        connect_instance_id: &str,
    ) -> impl Iterator<Item = &'a StoredIntegration> + 'a {
        self.region(region)
            .and_then(|store| store.instance_integrations.get(connect_instance_id))
            .into_iter()
            .flatten()
    }

    /// Remove the integration `region` holds for `connect_instance_id` that
    /// `identifier` names (see [`integration_identifier_key`]), and report whether one
    /// was removed.
    ///
    /// `false` means the instance holds no integration under that identity and nothing
    /// was written, which is what the caller answers as not found. The integrations the
    /// instance keeps hold the sequence numbers they were first put at, so removing one
    /// does not renumber the rest or reorder a page a caller is part way through.
    pub fn delete_instance_integration(
        &mut self,
        region: &str,
        connect_instance_id: &str,
        identifier: &IntegrationIdentifier,
    ) -> bool {
        let key = integration_identifier_key(identifier);
        let Some(integrations) = self
            .regions
            .get_mut(region)
            .and_then(|store| store.instance_integrations.get_mut(connect_instance_id))
        else {
            return false;
        };
        let Some(at) = integrations.iter().position(|stored| stored.key() == key) else {
            return false;
        };
        integrations.remove(at);
        true
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

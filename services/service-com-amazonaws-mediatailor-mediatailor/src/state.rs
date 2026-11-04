// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediatailor-mediatailor service.

use std::collections::{BTreeMap, HashMap};

use crate::ops::types::*;

/// The number of playback configurations this emulator will hold at once.
///
/// A real account's playback configurations are limited per Region through
/// Service Quotas. This emulator does not reproduce that figure, so the number
/// here is CHOSEN: its purpose is that the store cannot grow without bound on
/// request-chosen names, and that the ceiling is reached through the service's
/// modelled client error rather than by exhausting memory. Capacity is released
/// by `DeletePlaybackConfiguration`, as on the real service, and replacing an
/// existing configuration never consumes more.
pub const MAX_PLAYBACK_CONFIGURATIONS: usize = 500;

/// The number of channels this emulator will hold at once.
///
/// As with [`MAX_PLAYBACK_CONFIGURATIONS`] this figure is CHOSEN, not AWS's:
/// its purpose is that the store cannot grow without bound on request-chosen
/// channel names, and that the ceiling is reached through the service's
/// modelled client error rather than by exhausting memory. `DeleteChannel`
/// releases capacity.
pub const MAX_CHANNELS: usize = 500;

/// The number of outputs one channel carries, and the number of audiences.
///
/// `RequestOutputs` and `Audiences` are caller-sized lists and the model
/// declares no `length` constraint on either, so both figures are CHOSEN: their
/// purpose is that one stored channel cannot grow without bound. They are far
/// above any real channel's output or audience set.
pub const MAX_OUTPUTS_PER_CHANNEL: usize = 20;
/// See [`MAX_OUTPUTS_PER_CHANNEL`].
pub const MAX_AUDIENCES_PER_CHANNEL: usize = 100;

/// `TimeShiftConfiguration$MaxTimeDelaySeconds`' documented bounds: "The
/// minimum allowed maximum time delay is 0 seconds, and the maximum allowed
/// maximum time delay is 21600 seconds (6 hours)."
pub const MIN_TIME_SHIFT_DELAY_SECONDS: i32 = 0;
/// See [`MIN_TIME_SHIFT_DELAY_SECONDS`].
pub const MAX_TIME_SHIFT_DELAY_SECONDS: i32 = 21_600;

/// `Tier`'s value for a channel whose request names no tier: BASIC is the
/// channel tier MediaTailor assigns unless STANDARD is asked for.
pub const DEFAULT_CHANNEL_TIER: &str = "BASIC";

/// The two states `ChannelState` models, named so the operations that move a
/// channel between them spell neither value twice: a channel is either playing
/// or it is not, with no transient value in between — the enum's whole value
/// set, as the generated `CHANNEL_STATE_VALUES` in `crate::ops::types`
/// enumerates it — so the state a start or a stop leaves the channel in is
/// reached by the call that asked for it.
pub const RUNNING_CHANNEL_STATE: &str = "RUNNING";
/// See [`RUNNING_CHANNEL_STATE`].
pub const STOPPED_CHANNEL_STATE: &str = "STOPPED";

/// The state a channel is in once it has been created: a new channel is not
/// playing until `StartChannel` starts it, so this is the one a channel that
/// has never been started is in.
pub const INITIAL_CHANNEL_STATE: &str = STOPPED_CHANNEL_STATE;

/// The form of SCTE-35 ad markup an HLS channel output advertises when the
/// request that created it named none.
///
/// `HlsPlaylistSettings$AdMarkupType` "[d]etermines the type of SCTE 35 tags to
/// use in ad markup", and its own documentation ties each value to a tag form:
/// `DATERANGE` uses `DATERANGE` tags "(for live or VOD content)" while
/// `SCTE35_ENHANCED` uses `EXT-X-CUE-OUT`/`EXT-X-CUE-IN` tags "(for VOD content
/// only)". An HLS manifest carries one of those forms whether or not the caller
/// chose it, so a channel output always advertises a markup type, and the one it
/// advertises when no type was chosen is the form that serves either content
/// kind. The model declares no `@default` for the member, so the value lives
/// here: should a later revision declare one, that default replaces this
/// constant.
pub const DEFAULT_AD_MARKUP_TYPE: &str = "DATERANGE";

/// The HLS playlist settings one channel output is STORED with, given the
/// settings its request carried.
///
/// An output that asked for no HLS playlist has none — a DASH-only output is
/// reported without the member — but one that did advertises an ad-markup type
/// even when the request named no member, so the stored settings carry
/// [`DEFAULT_AD_MARKUP_TYPE`] in that case and every read of the channel reports
/// it. A request that names the member is stored as it sent it: the model
/// constrains the list's values, not their order or their number, so the types
/// an output advertises are the caller's whenever the caller chose them.
pub fn stored_hls_playlist_settings(
    requested: Option<&HlsPlaylistSettings>,
) -> Option<HlsPlaylistSettings> {
    requested.map(|settings| HlsPlaylistSettings {
        ad_markup_type: Some(
            settings
                .ad_markup_type
                .clone()
                .unwrap_or_else(|| vec![DEFAULT_AD_MARKUP_TYPE.to_string()]),
        ),
        ..settings.clone()
    })
}

/// The number of ad-markup types one HLS playlist names.
///
/// `AdMarkupType` is a caller-sized list and the model declares no `length`
/// constraint on it, so this figure is CHOSEN, as the other list ceilings in
/// this module are: its purpose is that one stored channel output cannot grow
/// without bound. It is far above the member's whole modelled value set (the
/// generated `AD_MARKUP_TYPE_VALUES` in `crate::ops::types`), so a request that
/// names every type it can name never reaches it.
pub const MAX_AD_MARKUP_TYPES_PER_PLAYLIST: usize = 20;

/// The number of tags one resource holds: AWS's documented tagging limit of 50
/// tags per resource. `PutPlaybackConfiguration` and `TagResource` refuse a
/// request that would leave a resource above it.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The length of a tag key: AWS's documented tagging limit of 128 characters.
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The length of a tag value: AWS's documented tagging limit of 256 characters.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The number of dynamic variables one configuration carries aliases for, and
/// the number of aliases under one variable.
///
/// `ConfigurationAliasesRequest` is a caller-sized map of caller-sized maps and
/// the model declares no `length` constraint on either, so both figures are
/// CHOSEN: their purpose is that one stored configuration cannot grow without
/// bound. They are far above any real configuration's alias set.
pub const MAX_CONFIGURATION_ALIAS_VARIABLES: usize = 50;
/// See [`MAX_CONFIGURATION_ALIAS_VARIABLES`].
pub const MAX_ALIASES_PER_VARIABLE: usize = 100;

/// `AdDecisionServerUrl`'s documented maximum: "The maximum length is 25,000
/// characters."
pub const MAX_AD_DECISION_SERVER_URL_LENGTH: usize = 25_000;

/// `VideoContentSourceUrl`'s documented maximum: "The maximum length is 512
/// characters."
pub const MAX_VIDEO_CONTENT_SOURCE_URL_LENGTH: usize = 512;

/// The ad-buffet mode a playback configuration reports for live and for VOD
/// streams.
///
/// `LiveAdBuffetMode` and `VodAdBuffetMode` are projected on every read of a
/// playback configuration, and no operation this service models carries them in
/// a request, so every configuration this emulator holds reports the service's
/// own default for both and there is no per-configuration value to persist.
///
/// The value is the one the service reports for a configuration that never set
/// a mode. It lives here rather than in the model because the model this crate
/// is generated from declares no such member and so no default for it either:
/// should a later revision of that model carry the members, their declared
/// default replaces this constant, and an operation that lets a request choose
/// a mode makes it a field of [`PlaybackConfigurationRecord`] instead.
pub const DEFAULT_AD_BUFFET_MODE: &str = "INSERT_ALL";

/// The number of event types one log-configuration list names.
///
/// `EnabledLoggingStrategies`, the interaction-log event-type lists and a
/// channel's `LogTypes` are caller-sized and the model declares no `length`
/// constraint on any of them,
/// so this figure is CHOSEN: its purpose is that one stored log configuration
/// cannot grow without bound however many times logs are configured. It is far
/// above every one of those lists' modelled value sets, so a request that names
/// each event type it has to name never reaches it.
pub const MAX_LOG_EVENT_TYPES_PER_LIST: usize = 100;

/// The largest page `ListPlaybackConfigurations` serves, and the page size it
/// applies when the request names none: `MaxResults`' modelled `range 1..100`,
/// whose maximum is also the documented default ("The default value is 100.").
/// The handler clamps to it itself rather than relying on boundary validation.
pub const MAX_PAGE_SIZE: usize = 100;

/// Per-service state for com-amazonaws-mediatailor-mediatailor. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMediatailorMediatailorState {
    /// Every playback configuration this emulator holds, keyed by its `Name` —
    /// the identifier `PUT /playbackConfiguration` creates it under and every
    /// other playback-configuration route addresses it by. A `BTreeMap` so
    /// `ListPlaybackConfigurations` walks the configurations in one stable order
    /// and a continuation token can name the page boundary.
    pub playback_configurations: BTreeMap<String, PlaybackConfigurationRecord>,
    /// Every channel this emulator holds, keyed by its `ChannelName` — the
    /// identifier `POST /channel/{ChannelName}` creates it under and every other
    /// channel route addresses it by. A `BTreeMap` so `ListChannels` walks the
    /// channels in one stable order and a continuation token can name the page
    /// boundary.
    pub channels: BTreeMap<String, ChannelRecord>,
    /// The tags of every taggable resource, keyed by resource ARN. This is the
    /// SINGLE source of truth for tags: `TagResource`/`UntagResource`/
    /// `ListTagsForResource` read and write it, `PutPlaybackConfiguration` seeds
    /// it from the request's `Tags`, and the playback-configuration reads render
    /// their `Tags` member from it — so a tag written through either surface is
    /// visible through both.
    ///
    /// Every key is a stored resource's OWN ARN — the tagging operations refuse
    /// any other ARN (see [`Self::taggable_resource_exists`]), a replacement
    /// that re-mints the ARN moves the entry, and a delete removes it — so this
    /// map holds at most one entry per stored resource.
    pub tags_by_arn: BTreeMap<String, BTreeMap<String, String>>,
}

impl ComAmazonawsMediatailorMediatailorState {
    /// The tags of `arn` as the wire shape renders them: the empty map when the
    /// resource carries none.
    ///
    /// A read of a resource answers what that resource's tags ARE, and for an
    /// untagged resource the answer is "none" — an empty map, not an absent
    /// member — so the caller dereferences a map either way.
    pub fn tags_of(&self, arn: &str) -> HashMap<String, String> {
        self.tags_by_arn
            .get(arn)
            .map(|tags| {
                tags.iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether a configuration named `name` can be stored: either it is already
    /// stored (a replacement, which consumes no further capacity) or the store
    /// is below [`MAX_PLAYBACK_CONFIGURATIONS`].
    pub fn can_hold_configuration(&self, name: &str) -> bool {
        self.playback_configurations.contains_key(name)
            || self.playback_configurations.len() < MAX_PLAYBACK_CONFIGURATIONS
    }

    /// Whether one more channel can be stored: the store holds at most
    /// [`MAX_CHANNELS`] of them, and `DeleteChannel` releases capacity.
    pub fn can_hold_channel(&self) -> bool {
        self.channels.len() < MAX_CHANNELS
    }

    /// Whether `arn` names a resource this emulator currently holds, so the
    /// tagging operations can refuse an ARN that addresses nothing.
    ///
    /// The whole ARN has to match the one the resource itself reports — its
    /// partition, region and account as well as its type and name — so an ARN
    /// that merely ends in a stored resource's name addresses nothing. That is
    /// also what bounds [`Self::tags_by_arn`]: every key it can ever hold is a
    /// stored resource's own ARN, released when that resource is deleted, so
    /// the tag store is bounded by the same ceilings the resource stores are.
    ///
    /// Every MediaTailor resource type this emulator persists is listed here:
    /// an ARN naming a type it does not persist yet addresses nothing, and an
    /// ARN this service does not own is not a resource of this service at all.
    pub fn taggable_resource_exists(&self, arn: &str) -> bool {
        let stored_arn = match resource_from_arn(arn) {
            Some(("playbackConfiguration", name)) => self
                .playback_configurations
                .get(name)
                .map(|record| record.arn.as_str()),
            Some(("channel", name)) => self.channels.get(name).map(|record| record.arn.as_str()),
            _ => None,
        };
        stored_arn == Some(arn)
    }
}

/// How this service states that `name` addresses no channel this account holds.
///
/// Every channel route that addresses a stored channel by name answers a miss
/// the same way — the name the request carried is the whole reason it cannot be
/// served, so the refusal names it — and the wording lives here once so the
/// channel read and the start/stop writes cannot drift into two accounts of the
/// same miss. The caller lifts it into its own modelled `BadRequestException`.
pub fn channel_not_found_message(name: &str) -> String {
    format!("Channel {name} not found.")
}

/// Why a tag set cannot be stored, or `None` when it can.
///
/// Checked against the set that WOULD be stored — the replacement set on the
/// Put path, the merge result on the `TagResource` path — so the stored set is
/// bounded in both the number of tags and the size of each one however it was
/// written. The caller words the refusal with its own modelled error.
pub fn tag_set_violation(tags: &BTreeMap<String, String>) -> Option<String> {
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Some(format!(
            "a resource holds at most {MAX_TAGS_PER_RESOURCE} tags, and this request would leave it with {}",
            tags.len()
        ));
    }
    for (key, value) in tags {
        if key.chars().count() > MAX_TAG_KEY_LENGTH {
            return Some(format!(
                "a tag key is at most {MAX_TAG_KEY_LENGTH} characters: {key}"
            ));
        }
        if value.chars().count() > MAX_TAG_VALUE_LENGTH {
            return Some(format!(
                "the value of tag {key} is at most {MAX_TAG_VALUE_LENGTH} characters"
            ));
        }
    }
    None
}

/// The resource type and name a MediaTailor ARN addresses, or `None` when `arn`
/// is not an ARN of this service's resources.
///
/// The ARN form is the one this service mints:
/// `arn:aws:mediatailor:<region>:<account>:<type>/<name>`, with `mediatailor`
/// the service's modelled `arnNamespace`.
pub fn resource_from_arn(arn: &str) -> Option<(&str, &str)> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some("mediatailor"), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    let (kind, name) = resource.split_once('/')?;
    (!kind.is_empty() && !name.is_empty()).then_some((kind, name))
}

/// The region an ARN a request mints is scoped to: the region the caller signed
/// for, which is the region whose endpoint the request addressed. Emulators
/// default to `us-east-1` when the request carries no SigV4 scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// One persisted playback configuration: the members the request carried, plus
/// the fields the service derives when it mints the configuration (its ARN and
/// its playback/session/manifest endpoint prefixes) and the log settings the
/// logging operations own.
///
/// `Tags` is deliberately NOT here: tags live once, in
/// [`ComAmazonawsMediatailorMediatailorState::tags_by_arn`], keyed by this
/// record's `arn`.
#[derive(Debug, Clone, Default)]
pub struct PlaybackConfigurationRecord {
    pub name: String,
    pub arn: String,
    pub ad_decision_server_url: Option<String>,
    pub video_content_source_url: Option<String>,
    pub slate_ad_url: Option<String>,
    pub transcode_profile_name: Option<String>,
    pub insertion_mode: String,
    pub personalization_threshold_seconds: Option<i32>,
    pub cdn_configuration: Option<CdnConfiguration>,
    pub dash_configuration: Option<DashConfiguration>,
    pub hls_configuration: Option<HlsConfiguration>,
    pub manifest_processing_rules: Option<ManifestProcessingRules>,
    pub avail_suppression: Option<AvailSuppression>,
    pub bumper: Option<Bumper>,
    pub configuration_aliases: Option<HashMap<String, HashMap<String, String>>>,
    pub live_pre_roll_configuration: Option<LivePreRollConfiguration>,
    pub ad_conditioning_configuration: Option<AdConditioningConfiguration>,
    pub ad_decision_server_configuration: Option<AdDecisionServerConfiguration>,
    pub yield_optimization_configuration: Option<YieldOptimizationConfiguration>,
    pub function_mapping: Option<HashMap<String, String>>,
    pub ads_personalization_timeouts: Option<AdsPersonalizationTimeouts>,
    pub ads_personalization_concurrency: Option<AdsPersonalizationConcurrency>,
    pub beaconing_configuration: Option<BeaconingConfiguration>,
    /// Owned by the logging operations, not by `PutPlaybackConfiguration`: a
    /// later Put over the same name carries it forward unchanged.
    pub log_configuration: LogConfiguration,
    pub playback_endpoint_prefix: String,
    pub dual_stack_playback_endpoint_prefix: String,
    pub session_initialization_endpoint_prefix: String,
    pub dual_stack_session_initialization_endpoint_prefix: String,
}

/// Renders a stored configuration into one of the three identical wire shapes
/// the model declares for it.
///
/// `PutPlaybackConfigurationResponse`, `GetPlaybackConfigurationResponse` and
/// the `PlaybackConfiguration` list item carry the SAME member set, so every
/// one of them is the stored record viewed through a different generated
/// struct. Implementing one rendering for all three is what keeps the Put
/// response, the Get response and the list item from drifting apart.
pub trait PlaybackConfigurationView: Sized {
    fn from_record(record: &PlaybackConfigurationRecord, tags: HashMap<String, String>) -> Self;
}

macro_rules! impl_playback_configuration_view {
    ($($shape:ty),+ $(,)?) => {
        $(impl PlaybackConfigurationView for $shape {
            fn from_record(
                record: &PlaybackConfigurationRecord,
                tags: HashMap<String, String>,
            ) -> Self {
                Self {
                    name: Some(record.name.clone()),
                    playback_configuration_arn: Some(record.arn.clone()),
                    ad_decision_server_url: record.ad_decision_server_url.clone(),
                    video_content_source_url: record.video_content_source_url.clone(),
                    slate_ad_url: record.slate_ad_url.clone(),
                    transcode_profile_name: record.transcode_profile_name.clone(),
                    insertion_mode: record.insertion_mode.clone(),
                    personalization_threshold_seconds: record.personalization_threshold_seconds,
                    cdn_configuration: record.cdn_configuration.clone(),
                    dash_configuration: record.dash_configuration.clone(),
                    hls_configuration: record.hls_configuration.clone(),
                    manifest_processing_rules: record.manifest_processing_rules.clone(),
                    avail_suppression: record.avail_suppression.clone(),
                    bumper: record.bumper.clone(),
                    configuration_aliases: record.configuration_aliases.clone(),
                    live_pre_roll_configuration: record.live_pre_roll_configuration.clone(),
                    ad_conditioning_configuration: record.ad_conditioning_configuration.clone(),
                    ad_decision_server_configuration: record
                        .ad_decision_server_configuration
                        .clone(),
                    yield_optimization_configuration: record
                        .yield_optimization_configuration
                        .clone(),
                    function_mapping: record.function_mapping.clone(),
                    ads_personalization_timeouts: record.ads_personalization_timeouts.clone(),
                    ads_personalization_concurrency: record.ads_personalization_concurrency.clone(),
                    beaconing_configuration: record.beaconing_configuration.clone(),
                    log_configuration: Some(record.log_configuration.clone()),
                    playback_endpoint_prefix: Some(record.playback_endpoint_prefix.clone()),
                    dual_stack_playback_endpoint_prefix: Some(
                        record.dual_stack_playback_endpoint_prefix.clone(),
                    ),
                    session_initialization_endpoint_prefix: Some(
                        record.session_initialization_endpoint_prefix.clone(),
                    ),
                    dual_stack_session_initialization_endpoint_prefix: Some(
                        record.dual_stack_session_initialization_endpoint_prefix.clone(),
                    ),
                    // Always present: the tag map (empty for an untagged
                    // configuration) and the two ad-buffet modes, which every
                    // read of a playback configuration reports.
                    tags: Some(tags),
                    live_ad_buffet_mode: Some(DEFAULT_AD_BUFFET_MODE.to_string()),
                    vod_ad_buffet_mode: Some(DEFAULT_AD_BUFFET_MODE.to_string()),
                }
            }
        })+
    };
}

impl_playback_configuration_view!(
    PutPlaybackConfigurationOutput,
    GetPlaybackConfigurationOutput,
    PlaybackConfiguration,
);

/// One persisted channel: the members the request carried, the fields the
/// service derives when it mints the channel (its ARN, each output's playback
/// URLs, its timestamps and its initial state) and the log settings the logging
/// operation owns.
///
/// `Tags` is deliberately NOT here: tags live once, in
/// [`ComAmazonawsMediatailorMediatailorState::tags_by_arn`], keyed by this
/// record's `arn`.
#[derive(Debug, Clone)]
pub struct ChannelRecord {
    pub name: String,
    pub arn: String,
    /// `RUNNING` or `STOPPED`: a created channel starts [`INITIAL_CHANNEL_STATE`]
    /// and the start/stop operations move it.
    pub channel_state: String,
    pub creation_time: ::aws_smithy_types::DateTime,
    pub last_modified_time: ::aws_smithy_types::DateTime,
    /// The channel's outputs as they are REPORTED: each request output plus the
    /// playback URLs the service derives for it.
    pub outputs: Vec<ResponseOutputItem>,
    pub playback_mode: String,
    pub tier: String,
    pub filler_slate: Option<SlateSource>,
    pub time_shift_configuration: Option<TimeShiftConfiguration>,
    /// The audiences defined in this channel, none when the request that created
    /// it named none: the member says which audiences the channel has, so the
    /// answer for a channel with none is an empty list rather than nothing.
    pub audiences: Vec<String>,
    /// Owned by `ConfigureLogsForChannel`, not by the create path: the log types
    /// enabled on this channel, none until that operation enables some.
    pub log_types: Vec<String>,
}

impl ChannelRecord {
    /// Moves this channel into `state`, which is one of
    /// [`RUNNING_CHANNEL_STATE`] / [`STOPPED_CHANNEL_STATE`].
    ///
    /// A start or a stop names the state the channel is to be IN rather than a
    /// change to apply, so the call is idempotent: one made against a channel
    /// already in that state leaves the stored record exactly as it was,
    /// `LastModifiedTime` included — that instant says when the channel was
    /// last modified, so it moves only when the channel itself did.
    pub fn set_channel_state(&mut self, state: &str, at: ::aws_smithy_types::DateTime) {
        if self.channel_state != state {
            self.channel_state = state.to_string();
            self.last_modified_time = at;
        }
    }

    /// The stored channel as the create path reports it.
    ///
    /// `Tags` and `Audiences` are ALWAYS reported: each member says what the
    /// channel HAS — the tags it carries, the audiences defined in it — and a
    /// channel with none of either has an empty set rather than nothing, so a
    /// caller dereferences a map and a list whatever the channel was created
    /// with. Each of this channel's outputs is reported with the playlist
    /// settings it was stored with, which carry their own always-present members
    /// (see [`stored_hls_playlist_settings`]).
    ///
    /// Every channel-shaped response is this one rendering of the stored
    /// record: [`ChannelRecord::describe_output`] re-uses it member for member,
    /// so the create response and the read cannot drift apart.
    pub fn create_output(&self, tags: HashMap<String, String>) -> CreateChannelOutput {
        CreateChannelOutput {
            arn: Some(self.arn.clone()),
            channel_name: Some(self.name.clone()),
            channel_state: Some(self.channel_state.clone()),
            creation_time: Some(self.creation_time),
            filler_slate: self.filler_slate.clone(),
            last_modified_time: Some(self.last_modified_time),
            outputs: Some(self.outputs.clone()),
            playback_mode: Some(self.playback_mode.clone()),
            tags: Some(tags),
            tier: Some(self.tier.clone()),
            time_shift_configuration: self.time_shift_configuration.clone(),
            audiences: Some(self.audiences.clone()),
        }
    }

    /// The stored channel as the read reports it: the create view's members,
    /// plus the log settings `ConfigureLogsForChannel` owns — the one member
    /// `DescribeChannelResponse` adds, and a required one, so a channel with no
    /// log types enabled reports an empty list rather than nothing.
    pub fn describe_output(&self, tags: HashMap<String, String>) -> DescribeChannelOutput {
        let CreateChannelOutput {
            arn,
            channel_name,
            channel_state,
            creation_time,
            filler_slate,
            last_modified_time,
            outputs,
            playback_mode,
            tags,
            tier,
            time_shift_configuration,
            audiences,
        } = self.create_output(tags);
        DescribeChannelOutput {
            arn,
            channel_name,
            channel_state,
            creation_time,
            filler_slate,
            last_modified_time,
            outputs,
            playback_mode,
            tags,
            tier,
            log_configuration: LogConfigurationForChannel {
                log_types: Some(self.log_types.clone()),
            },
            time_shift_configuration,
            audiences,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMediatailorMediatailorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMediatailorMediatailorState>)
}

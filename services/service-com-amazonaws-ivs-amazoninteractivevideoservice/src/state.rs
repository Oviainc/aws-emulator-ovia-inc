// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ivs-amazoninteractivevideoservice service,
//! plus the few rules more than one operation in the channel lifecycle applies
//! (ARN minting, id minting, pagination tokens, and the two type-dependent
//! `Channel` rules the model documents).

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AdConfiguration, Channel, MultitrackInputConfiguration, RecordingConfiguration,
    RenditionConfiguration, Stream, StreamKey, ThumbnailConfiguration,
};

/// The region every minted ARN falls back to when the request carries no SigV4
/// credential scope to read one from (an unsigned, anonymous call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// Per-service state for com-amazonaws-ivs-amazoninteractivevideoservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIvsAmazoninteractivevideoserviceState {
    /// Every persisted channel, keyed by its ARN. A `BTreeMap` rather than a hash
    /// map so `ListChannels` has ONE stable order, which is what makes a
    /// `nextToken` able to resume exactly where the previous page stopped.
    pub channels: BTreeMap<String, Channel>,
    /// Every persisted stream key, keyed by its ARN. `StreamKey::channel_arn` binds
    /// a key to its channel: `ListStreamKeys` filters on it and `DeleteChannel`
    /// drops every key that names the channel it removed.
    pub stream_keys: BTreeMap<String, StreamKey>,
    /// The stream each channel carries, keyed by the ARN of that channel: a
    /// channel ingests at most one stream at a time, so its ARN identifies the
    /// stream. A channel with no entry here — or one whose entry has left the
    /// `LIVE` state — is not broadcasting, which is the whole distinction
    /// `GetStream`'s `ChannelNotBroadcasting` answer carries. An entry is only
    /// ever reported through its channel, and `DeleteChannel` drops it with the
    /// channel it belongs to.
    pub streams: BTreeMap<String, Stream>,
    /// Every viewer-session revocation this store has started, keyed by the
    /// `(channelArn, viewerId)` pair it names and holding the HIGHEST viewer
    /// session version revoked for that pair so far. A `BTreeMap` for the same
    /// reason as the other two: one stable order over the pairs.
    ///
    /// `viewerId` comes from the request, so the live channel set does NOT bound
    /// this map's key space: one channel admits as many pairs as a caller invents
    /// viewer ids. [`MAX_VIEWER_SESSION_REVOCATIONS`] is what bounds it, enforced
    /// by [`start_viewer_session_revocation`] — the only writer.
    pub viewer_session_revocations: BTreeMap<(String, String), i32>,
    /// Every persisted ad configuration, keyed by its ARN. A `BTreeMap` for the
    /// same reason as `channels`: `ListAdConfigurations` is paginated, so the one
    /// stable order is what lets a `nextToken` resume where a page stopped.
    pub ad_configurations: BTreeMap<String, AdConfiguration>,
    /// Every persisted recording configuration, keyed by its ARN, each with the
    /// instant its `CREATING` → `ACTIVE` transition falls due. A `BTreeMap` for the
    /// same reason as [`Self::channels`]: one stable order for
    /// `ListRecordingConfigurations` to page over.
    pub recording_configurations: BTreeMap<String, RecordingConfigurationRecord>,
    /// How many ids this store has minted. Part of every id's seed, so no two
    /// minted ids collide even when two calls land in the same nanosecond.
    pub minted: u64,
}

/// A persisted recording configuration together with the instant at which the
/// `CREATING` → `ACTIVE` transition the clock owes it becomes due.
#[derive(Debug, Clone)]
pub struct RecordingConfigurationRecord {
    /// What every read of this resource answers.
    pub configuration: RecordingConfiguration,
    /// When the owed transition falls due; see [`RECORDING_CONFIGURATION_ACTIVATION`].
    pub active_at: SystemTime,
}

impl ComAmazonawsIvsAmazoninteractivevideoserviceState {
    /// A fresh lowercase-hex id of `bytes` bytes (so `2 * bytes` characters).
    ///
    /// Hex is a subset of the alphabet every IVS id constraint allows — an ARN's
    /// resource segment is `[a-zA-Z0-9-]+` — so a minted id always satisfies the
    /// modelled pattern. `minted` advances on every call and is hashed into the
    /// seed together with the wall clock and the process id, so repeated calls
    /// never return the same id.
    pub fn mint_hex(&mut self, label: &str, bytes: usize) -> String {
        self.minted = self.minted.wrapping_add(1);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let seed = format!("{label}|{}|{}|{nanos}", self.minted, std::process::id());
        let digest = Sha256::digest(seed.as_bytes());
        hex::encode(&digest[..bytes.min(digest.len())])
    }

    /// Materialise every `CREATING` → `ACTIVE` transition the clock already owes.
    ///
    /// A recording configuration is minted in `CREATING` and AWS turns it `ACTIVE`
    /// on its own, so the move is owed by the clock at
    /// [`RecordingConfigurationRecord::active_at`] whether or not anybody calls.
    /// Every operation that reports a recording configuration runs this first, so a
    /// read materialises only what was already due and never advances a resource
    /// because it was looked at.
    pub fn settle_recording_configurations(&mut self) {
        let now = SystemTime::now();
        for record in self.recording_configurations.values_mut() {
            if record.configuration.state == RECORDING_CONFIGURATION_CREATING
                && record.active_at <= now
            {
                record.configuration.state = RECORDING_CONFIGURATION_ACTIVE.to_string();
            }
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIvsAmazoninteractivevideoserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIvsAmazoninteractivevideoserviceState>)
}

/// The region to mint this request's ARNs in: the region of its SigV4 credential
/// scope, or [`DEFAULT_REGION`] when the request is unsigned.
///
/// A scope region is honoured only when it can appear in an IVS ARN at all — the
/// modelled `ChannelArn` pattern pins that segment to `[a-z0-9-]+` — so a hostile
/// or nonsense scope can never make this service mint an ARN its own model
/// rejects.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region segment of an ARN (`arn:aws:ivs:<region>:<account>:<resource>`), so a
/// resource minted for an existing one lands in the same region as its parent.
pub fn arn_region(arn: &str) -> Option<&str> {
    arn.split(':').nth(3).filter(|region| !region.is_empty())
}

/// A `ChannelArn`: `arn:aws:ivs:<region>:<account>:channel/<id>`.
pub fn channel_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("channel/{id}"))
}

/// A `StreamKeyArn`: `arn:aws:ivs:<region>:<account>:stream-key/<id>`.
pub fn stream_key_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("stream-key/{id}"))
}

/// An `AdConfigurationArn`: `arn:aws:ivs:<region>:<account>:ad-configuration/<id>`,
/// the form the modelled `AdConfigurationArn` pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:ad-configuration/[a-zA-Z0-9-]+$` pins.
pub fn ad_configuration_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("ad-configuration/{id}"))
}

/// A `RecordingConfigurationArn`:
/// `arn:aws:ivs:<region>:<account>:recording-configuration/<id>`.
pub fn recording_configuration_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("recording-configuration/{id}"))
}

/// The state a recording configuration is minted in, from the modelled
/// `RecordingConfigurationState` values `CREATING | CREATE_FAILED | ACTIVE`.
pub const RECORDING_CONFIGURATION_CREATING: &str = "CREATING";

/// The state a recording configuration settles in once the clock owes the move:
/// "When the state is `ACTIVE`, the configuration is ready for recording a channel
/// stream" (model doc on `RecordingConfiguration$state`).
pub const RECORDING_CONFIGURATION_ACTIVE: &str = "ACTIVE";

/// How long after `CreateRecordingConfiguration` the `CREATING` → `ACTIVE`
/// transition falls due.
///
/// AWS publishes no duration for it, so this is the emulator's own choice inside
/// the project's 250-750 ms band for a transition the clock owes. It is
/// deliberately non-zero: a caller that reads straight after the create still
/// observes `CREATING`, exactly as it would against AWS.
pub const RECORDING_CONFIGURATION_ACTIVATION: Duration = Duration::from_millis(500);

/// The ceiling on how many recording configurations this store holds, for the same
/// reason as [`MAX_CHANNELS`]: a create is the only thing that adds one and the
/// store lives for the life of the process, so caller-driven growth needs a bound.
/// Past it, `CreateRecordingConfiguration` answers the
/// `ServiceQuotaExceededException` (HTTP 402) it models. The number is the
/// emulator's own bound, not a published AWS default (AWS publishes the quota as
/// adjustable), and is set high enough that no ordinary workflow meets it.
pub const MAX_RECORDING_CONFIGURATIONS: usize = 1_000;

/// Refuse a rendition configuration whose members cannot hold together, returning
/// the message to put on a `ValidationException`.
///
/// From the model doc on `RenditionConfiguration$renditionSelection`: "If `CUSTOM`
/// is specified, a set of renditions must be specified in the `renditions` field."
/// An absent or empty list names no rendition, so neither is a set.
pub fn check_rendition_configuration(rendition: &RenditionConfiguration) -> Result<(), String> {
    let selection = rendition.rendition_selection.as_deref().unwrap_or("");
    let renditions = rendition.renditions.as_deref().unwrap_or(&[]);
    if selection == "CUSTOM" && renditions.is_empty() {
        return Err(
            "renditionConfiguration.renditions is required to name at least one rendition \
             when renditionConfiguration.renditionSelection is CUSTOM"
                .to_string(),
        );
    }
    Ok(())
}

/// The rendition configuration to persist for what the caller sent: every member it
/// omitted filled from the model's documented default.
///
/// From the model doc on `RenditionConfiguration$renditionSelection`: "Indicates
/// which set of renditions are recorded for a stream. … Default: `ALL`." A caller
/// that sent no `renditionConfiguration` at all chose nothing, and the model
/// documents no default for the structure itself, so that stays absent.
pub fn effective_rendition_configuration(
    requested: Option<RenditionConfiguration>,
) -> Option<RenditionConfiguration> {
    requested.map(|mut rendition| {
        if rendition
            .rendition_selection
            .as_deref()
            .is_none_or(str::is_empty)
        {
            rendition.rendition_selection = Some("ALL".to_string());
        }
        rendition
    })
}

/// The thumbnail configuration to persist for what the caller sent: every member it
/// omitted filled from the model's documented default.
///
/// From the model docs on `ThumbnailConfiguration`: `recordingMode` "Default:
/// `INTERVAL`"; `targetIntervalSeconds` "is configurable (and required) only if
/// `recordingMode` is `INTERVAL`. Default: 60"; `storage` "Default: `SEQUENTIAL`";
/// `resolution` "Default: Null (source resolution is returned)", so an omitted
/// resolution stays absent. As with [`effective_rendition_configuration`], an
/// omitted structure carries no choice and stays absent.
pub fn effective_thumbnail_configuration(
    requested: Option<ThumbnailConfiguration>,
) -> Option<ThumbnailConfiguration> {
    requested.map(|mut thumbnail| {
        if thumbnail.recording_mode.as_deref().is_none_or(str::is_empty) {
            thumbnail.recording_mode = Some("INTERVAL".to_string());
        }
        if thumbnail.recording_mode.as_deref() == Some("INTERVAL")
            && thumbnail.target_interval_seconds.is_none()
        {
            thumbnail.target_interval_seconds = Some(60);
        }
        if thumbnail.storage.is_none() {
            thumbnail.storage = Some(vec!["SEQUENTIAL".to_string()]);
        }
        thumbnail
    })
}

/// The ceiling on how many channels this store holds, and on how many stream keys
/// one channel holds: the two caps that bound caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process
/// and a create is the only thing that adds one, so the store needs a maximum entry
/// count — neither a TTL nor an eviction policy is available to it, because
/// `GetChannel` must still answer what `CreateChannel` created. The two operations
/// that grow the store refuse past these caps with the service's own modelled
/// `ServiceQuotaExceededException` (HTTP 402), which IVS declares on exactly those
/// two operations and on no read. The numbers are the emulator's own bound, not a
/// published AWS default: AWS publishes both quotas as adjustable, so no single
/// value is the contract, and the caps are set high enough that no ordinary workflow
/// meets one.
pub const MAX_CHANNELS: usize = 5_000;

/// See [`MAX_CHANNELS`].
pub const MAX_STREAM_KEYS_PER_CHANNEL: usize = 100;

/// The ceiling on how many `(channelArn, viewerId)` pairs this store tracks a
/// viewer-session revocation for — a caller-driven growth path bounded for the same
/// reason as [`MAX_CHANNELS`]: the store has neither a TTL nor an eviction policy,
/// and a pair's `viewerId` comes from the request, so nothing else bounds how many
/// pairs a caller can name. IVS publishes no quota for revocations, so like the
/// others this number is the emulator's own bound, set high enough that no ordinary
/// workflow meets it.
///
/// [`start_viewer_session_revocation`] enforces it: past the ceiling a pair that is
/// not already tracked is REFUSED rather than recorded, and the batch form reports
/// that refusal as the per-item failure it is.
pub const MAX_VIEWER_SESSION_REVOCATIONS: usize = 10_000;

/// The ceiling on how many ad configurations this store holds. See [`MAX_CHANNELS`]
/// for why a cap exists at all: `CreateAdConfiguration` is the only thing that adds
/// one, it never expires, and past the cap the operation answers the
/// `ServiceQuotaExceededException` (HTTP 402) IVS models on it. The number is the
/// emulator's own bound, not a published AWS default.
pub const MAX_AD_CONFIGURATIONS: usize = 1_000;

/// How many stream keys are currently bound to `channel_arn`, which is what
/// [`MAX_STREAM_KEYS_PER_CHANNEL`] bounds.
pub fn stream_key_count(
    state: &ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
) -> usize {
    state
        .stream_keys
        .values()
        .filter(|key| key.channel_arn.as_deref() == Some(channel_arn))
        .count()
}

/// Mint a stream key bound to `channel_arn` and persist it.
///
/// Shared by `CreateChannel` (which mints a channel's first key) and
/// `CreateStreamKey` (which adds another), so both produce keys of one form and
/// both land in the one `stream_keys` map `ListStreamKeys` reads.
pub fn mint_stream_key(
    state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState,
    region: &str,
    channel_arn: &str,
    tags: Option<std::collections::HashMap<String, String>>,
) -> StreamKey {
    let id = state.mint_hex("stream-key", 6);
    let secret = state.mint_hex("stream-key-value", 16);
    let arn = stream_key_arn(region, &id);
    let key = StreamKey {
        arn: Some(arn.clone()),
        value: Some(format!("sk_{region}_{id}_{secret}")),
        channel_arn: Some(channel_arn.to_string()),
        tags: Some(tags.unwrap_or_default()),
    };
    state.stream_keys.insert(arn, key.clone());
    key
}

/// The `StreamState` of a channel that is ingesting, from the modelled
/// `StreamState` enum (`LIVE | OFFLINE`, see
/// [`crate::ops::types::STREAM_STATE_VALUES`]).
pub const STREAM_STATE_LIVE: &str = "LIVE";

/// The `StreamHealth` of a stream whose ingest session is connected and
/// delivering, from the modelled `StreamHealth` enum (`HEALTHY | STARVING |
/// UNKNOWN`, see [`crate::ops::types::STREAM_HEALTH_VALUES`]).
pub const STREAM_HEALTH_HEALTHY: &str = "HEALTHY";

/// A fresh `StreamId`.
///
/// The modelled shape is exactly 26 characters and matches `^st-[a-zA-Z0-9]+$`,
/// so an id is the `st-` prefix plus 23 characters from that alphabet; hex is a
/// subset of it, and [`ComAmazonawsIvsAmazoninteractivevideoserviceState::mint_hex`]
/// never repeats a value.
fn mint_stream_id(state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState) -> String {
    let digest = state.mint_hex("stream", 12);
    format!("st-{}", &digest[..23])
}

/// The LIVE stream `channel_arn` is currently ingesting, or `None` when that
/// channel is not broadcasting.
///
/// Only a stream in the `LIVE` state is an active one: an entry that has left
/// that state is a channel whose broadcast has ended, which every operation
/// about the active stream must treat exactly like a channel that never had one.
pub fn live_stream(
    state: &ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
) -> Option<Stream> {
    state
        .streams
        .get(channel_arn)
        .filter(|stream| stream.state.as_deref() == Some(STREAM_STATE_LIVE))
        .cloned()
}

/// Start the stream an ingest session has brought up on `channel_arn`, persist it
/// and return it.
///
/// One place mints a stream, so every operation that reports one answers the same
/// representation: the stream is bound to the channel it is ingested on, is played
/// back from that channel's own `playbackUrl`, and starts at the instant the
/// ingest connected. `viewerCount` is 0 because no playback session is attached to
/// it yet, and `health` is `HEALTHY` because an ingest session is connected and
/// delivering — a measurement that would move it to `STARVING` belongs to whoever
/// is feeding the ingest. A channel that is ALREADY broadcasting keeps the stream
/// it has: IVS holds one stream per channel, so a second ingest cannot mint a
/// second stream id for the same channel.
pub fn start_stream(
    state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
) -> Stream {
    if let Some(live) = live_stream(state, channel_arn) {
        return live;
    }
    let playback_url = state
        .channels
        .get(channel_arn)
        .and_then(|channel| channel.playback_url.clone());
    let stream_id = mint_stream_id(state);
    let stream = Stream {
        channel_arn: Some(channel_arn.to_string()),
        stream_id: Some(stream_id),
        playback_url,
        start_time: Some(::aws_smithy_types::DateTime::from(
            std::time::SystemTime::now(),
        )),
        state: Some(STREAM_STATE_LIVE.to_string()),
        health: Some(STREAM_HEALTH_HEALTHY.to_string()),
        viewer_count: 0,
    };
    state.streams.insert(channel_arn.to_string(), stream.clone());
    stream
}

/// The one way [`start_viewer_session_revocation`] can fail: the store is at
/// [`MAX_VIEWER_SESSION_REVOCATIONS`] and the pair is not one it already tracks.
///
/// A type rather than a message, so each caller words the refusal the way its own
/// response shape needs and the store holds no wire text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewerSessionRevocationCapReached {
    /// The cap that was reached, so a caller can name it.
    pub cap: usize,
}

/// Start revoking `viewer_id`'s sessions on `channel_arn` up to and including
/// `versions_less_than_or_equal_to`, and answer the version threshold now in force
/// for that pair — or [`ViewerSessionRevocationCapReached`] when tracking a NEW pair
/// would take this store past [`MAX_VIEWER_SESSION_REVOCATIONS`], which is the only
/// way this can fail.
///
/// From the model doc on `viewerSessionVersionsLessThanOrEqualTo`: "An optional
/// filter on which versions of the viewer session to revoke. All versions less than
/// or equal to the specified version will be revoked." A later call naming a LOWER
/// version therefore cannot un-revoke what an earlier call already revoked, so the
/// stored threshold is the maximum over the calls — which also makes a repeated call
/// idempotent.
///
/// Raising an EXISTING pair's threshold adds no entry, so it is never refused: the
/// cap bounds how many pairs are tracked, not how often one is revoked.
///
/// Shared so that every operation that starts a revocation (the single-pair
/// `StartViewerSessionRevocation` and the batch form that the model documents as
/// "performs StartViewerSessionRevocation on multiple channel ARN and viewer ID
/// pairs") writes one record of one form and honours one cap.
pub fn start_viewer_session_revocation(
    state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
    viewer_id: &str,
    versions_less_than_or_equal_to: i32,
) -> Result<i32, ViewerSessionRevocationCapReached> {
    let key = (channel_arn.to_string(), viewer_id.to_string());
    if !state.viewer_session_revocations.contains_key(&key)
        && state.viewer_session_revocations.len() >= MAX_VIEWER_SESSION_REVOCATIONS
    {
        // A record whose channel is gone can never be revoked against again — this
        // service mints a fresh random id for every channel, so a deleted ARN never
        // comes back — so those dead records are dropped before the cap is weighed.
        let ComAmazonawsIvsAmazoninteractivevideoserviceState {
            channels,
            viewer_session_revocations,
            ..
        } = state;
        viewer_session_revocations.retain(|(arn, _), _| channels.contains_key(arn));
        if state.viewer_session_revocations.len() >= MAX_VIEWER_SESSION_REVOCATIONS {
            return Err(ViewerSessionRevocationCapReached {
                cap: MAX_VIEWER_SESSION_REVOCATIONS,
            });
        }
    }
    let threshold = state
        .viewer_session_revocations
        .entry(key)
        .or_insert(versions_less_than_or_equal_to);
    *threshold = (*threshold).max(versions_less_than_or_equal_to);
    Ok(*threshold)
}

/// The `preset` a channel of `channel_type` carries, given the `requested` value.
///
/// From the model doc on `Channel$preset`: "Optional transcode preset for the
/// channel. This is selectable only for `ADVANCED_HD` and `ADVANCED_SD` channel
/// types. For those channel types, the default `preset` is
/// `HIGHER_BANDWIDTH_DELIVERY`. For other channel types (`BASIC` and `STANDARD`),
/// `preset` is the empty string."
pub fn effective_preset(channel_type: &str, requested: Option<&str>) -> String {
    match channel_type {
        "ADVANCED_HD" | "ADVANCED_SD" => requested
            .filter(|preset| !preset.is_empty())
            .unwrap_or("HIGHER_BANDWIDTH_DELIVERY")
            .to_string(),
        _ => String::new(),
    }
}

/// Refuse a channel whose `containerFormat` and multitrack configuration cannot
/// hold together, returning the message to put on a `ValidationException`.
///
/// From the model doc on `Channel$containerFormat`: "If
/// `multitrackInputConfiguration` is specified and `enabled` is `true`, then
/// `containerFormat` is required and must be set to `FRAGMENTED_MP4`. Otherwise,
/// `containerFormat` may be set to `TS` or `FRAGMENTED_MP4`."
pub fn check_container_format(
    container_format: &str,
    multitrack: Option<&MultitrackInputConfiguration>,
) -> Result<(), String> {
    if multitrack.is_some_and(|config| config.enabled) && container_format != "FRAGMENTED_MP4" {
        return Err(format!(
            "containerFormat must be FRAGMENTED_MP4 when multitrackInputConfiguration is enabled, got {container_format}"
        ));
    }
    Ok(())
}

/// The stamp a `ListChannels` page token carries, so a token issued for one list
/// cannot be replayed against another.
pub const CHANNEL_PAGE: &str = "channel";

/// The stamp a `ListStreamKeys` page token carries. See [`CHANNEL_PAGE`].
pub const STREAM_KEY_PAGE: &str = "stream-key";

/// The stamp a `ListRecordingConfigurations` page token carries. See
/// [`CHANNEL_PAGE`].
pub const RECORDING_CONFIGURATION_PAGE: &str = "recording-configuration";

/// The prefix of every ARN this service mints, and so of every page boundary a
/// token may legitimately name.
const ARN_PREFIX: &str = "arn:aws:ivs:";

/// Encode a page boundary as an opaque `PaginationToken`.
///
/// The token carries the sort key of the LAST item the page returned, so the next
/// page is everything ordered strictly after it — a resume point that stays correct
/// when items are added or removed between pages, which a positional offset would
/// not. It is STAMPED with `kind` and a format version so a forged or replayed value
/// is rejected rather than honoured as some other list's boundary. Lowercase hex
/// keeps the token inside the modelled `PaginationToken` pattern
/// `^[a-zA-Z0-9+/=_-]*$` for any key, and the stamp plus a 128-byte ARN encodes to
/// well under the modelled 1024-character bound.
pub fn encode_page_token(kind: &str, last_key: &str) -> String {
    hex::encode(format!("ivs:1:{kind}:{last_key}").as_bytes())
}

/// The page boundary a `kind` token names, or `None` when it is not a token this
/// service issued for that list — which the caller is told as a
/// `ValidationException`, never as a silently truncated page.
pub fn decode_page_token(kind: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let key = decoded.strip_prefix(&format!("ivs:1:{kind}:"))?;
    // Every key this service pages on is an ARN it minted, so anything else is a
    // boundary it could not have issued.
    key.starts_with(ARN_PREFIX).then(|| key.to_string())
}

/// The page size a list serves for the `maxResults` the caller sent, bounded by the
/// `maximum` that list's own modelled `max*Results` shape declares.
///
/// Shared by every list operation, so the three cannot drift apart: an absent
/// `maxResults` means the whole page, and a value the modelled range would refuse —
/// which the framework has already rejected before any handler runs — can never
/// reach [`page`] as a zero or an unbounded size.
pub fn page_size(requested: Option<i32>, maximum: i32) -> usize {
    let maximum = usize::try_from(maximum).unwrap_or(1).max(1);
    usize::try_from(requested.unwrap_or(i32::MAX))
        .unwrap_or(maximum)
        .clamp(1, maximum)
}

/// Cut one page out of an ordered, already-filtered `(key, item)` sequence, and
/// issue the token that resumes after it.
///
/// Shared by every list operation so they cannot drift apart: one more than the page
/// is read, so a token is issued only when a further page really exists, and the
/// token names the last key this page returned. A page holds at least one item —
/// every modelled `max*Results` has a minimum of 1, and a zero-sized page would
/// answer an empty list with no token, which reads as "nothing left" while items
/// remain.
pub fn page<'a, T>(
    kind: &str,
    ordered: impl Iterator<Item = (&'a String, &'a T)>,
    after: Option<&str>,
    page_size: usize,
) -> (Vec<&'a T>, Option<String>) {
    let page_size = page_size.max(1);
    let mut window: Vec<(&'a String, &'a T)> = ordered
        .filter(|(key, _)| after.is_none_or(|boundary| key.as_str() > boundary))
        .take(page_size.saturating_add(1))
        .collect();
    let next_token = if window.len() > page_size {
        window.truncate(page_size);
        window
            .last()
            .map(|(key, _)| encode_page_token(kind, key.as_str()))
    } else {
        None
    };
    (
        window.into_iter().map(|(_, item)| item).collect(),
        next_token,
    )
}

// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime service.
//!
//! The stage lifecycle (`CreateStage`, `GetStage`, `ListStages`, `UpdateStage`,
//! `DeleteStage`) and the token issuance that makes a stage usable
//! (`CreateParticipantToken`) all read and write the records below, so the records,
//! the id minting they share and the modelled defaults they apply live here rather
//! than in any one operation's file.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{validate, LengthTrait, PatternTrait};
use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    AutoParticipantRecordingConfiguration, InternalServerException,
    ParticipantRecordingHlsConfiguration, ParticipantThumbnailConfiguration, ParticipantToken,
    ResourceNotFoundException, ServiceQuotaExceededException, Stage, StageEndpoints, StageSummary,
    ValidationException,
};
use crate::scaffolded::Scaffolded;

/// Lifetime a participant token gets when the request omits `duration`.
///
/// The model documents it on `CreateParticipantTokenRequest$duration` and on
/// `ParticipantToken$duration`: "Duration (in minutes), after which the token
/// expires. Default: 720 (12 hours)."
pub const DEFAULT_TOKEN_DURATION_MINUTES: i32 = 720;

/// Capabilities a participant token gets when the request omits `capabilities`.
///
/// The model documents it on `CreateParticipantTokenRequest$capabilities`: "Set of
/// capabilities that the user is allowed to perform in the stage. Default: `PUBLISH,
/// SUBSCRIBE`."
pub const DEFAULT_TOKEN_CAPABILITIES: [&str; 2] = ["PUBLISH", "SUBSCRIBE"];

/// [`DEFAULT_TOKEN_CAPABILITIES`] as the modelled `ParticipantTokenCapabilities` list,
/// so both operations that issue a token apply the documented default from one place.
pub fn default_token_capabilities() -> Vec<String> {
    DEFAULT_TOKEN_CAPABILITIES
        .iter()
        .map(|capability| (*capability).to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// The modelled defaults of a stage's individual-participant-recording
// configuration
//
// `AutoParticipantRecordingConfiguration` is reported for every stage, so each member
// the caller left out is reported with the default the model documents for it. Every
// constant below cites the member doc it comes from; none is this emulator's choice.
// ---------------------------------------------------------------------------

/// `storageConfigurationArn` of a stage with no storage configuration attached.
///
/// Documented on `AutoParticipantRecordingConfiguration$storageConfigurationArn`:
/// "Default: `\"\"` (empty string, no storage configuration is specified)."
pub const NO_STORAGE_CONFIGURATION_ARN: &str = "";

/// Media types recorded when the configuration names none.
///
/// Documented on `AutoParticipantRecordingConfiguration$mediaTypes`: "Types of media to
/// be recorded. Default: `AUDIO_VIDEO`."
pub const DEFAULT_RECORDING_MEDIA_TYPE: &str = "AUDIO_VIDEO";

/// Window in which a publisher's reconnect is merged into one recording, when the
/// configuration does not size it.
///
/// Documented on `AutoParticipantRecordingConfiguration$recordingReconnectWindowSeconds`:
/// "The default value is 0, which disables merging."
pub const DEFAULT_RECORDING_RECONNECT_WINDOW_SECONDS: i32 = 0;

/// Whether replica participants are recorded, when the configuration does not say.
///
/// Documented on `AutoParticipantRecordingConfiguration$recordParticipantReplicas`:
/// "Optional field to disable replica participant recording. … Default: `true`."
pub const DEFAULT_RECORD_PARTICIPANT_REPLICAS: bool = true;

/// Thumbnail-generation interval, when the thumbnail configuration does not set one.
///
/// Documented on `ParticipantThumbnailConfiguration$targetIntervalSeconds`: "The targeted
/// thumbnail-generation interval in seconds. … Default: 60."
pub const DEFAULT_THUMBNAIL_INTERVAL_SECONDS: i32 = 60;

/// Thumbnail storage format, when the thumbnail configuration names none.
///
/// Documented on `ParticipantThumbnailConfiguration$storage`: "Indicates the format in
/// which thumbnails are recorded. … Default: `SEQUENTIAL`."
pub const DEFAULT_THUMBNAIL_STORAGE: &str = "SEQUENTIAL";

/// Thumbnail recording mode, when the thumbnail configuration names none.
///
/// Documented on `ParticipantThumbnailConfiguration$recordingMode`: "Thumbnail recording
/// mode. Default: `DISABLED`."
pub const DEFAULT_THUMBNAIL_RECORDING_MODE: &str = "DISABLED";

/// Target duration of a recorded HLS segment, when the HLS configuration does not set one.
///
/// Documented on `ParticipantRecordingHlsConfiguration$targetSegmentDurationSeconds`:
/// "Defines the target duration for recorded segments generated when recording a stage
/// participant. … Default: 6."
pub const DEFAULT_HLS_TARGET_SEGMENT_DURATION_SECONDS: i32 = 6;

/// [`DEFAULT_RECORDING_MEDIA_TYPE`] as the modelled `ParticipantRecordingMediaTypeList`.
pub fn default_recording_media_types() -> Vec<String> {
    vec![DEFAULT_RECORDING_MEDIA_TYPE.to_string()]
}

/// [`DEFAULT_THUMBNAIL_STORAGE`] as the modelled `ThumbnailStorageTypeList`.
pub fn default_thumbnail_storage() -> Vec<String> {
    vec![DEFAULT_THUMBNAIL_STORAGE.to_string()]
}

/// The thumbnail configuration a stage reports, with each member the caller left out
/// filled in from the model's documented default for it.
pub fn effective_thumbnail_configuration(
    requested: Option<ParticipantThumbnailConfiguration>,
) -> ParticipantThumbnailConfiguration {
    let requested = requested.unwrap_or_default();
    ParticipantThumbnailConfiguration {
        target_interval_seconds: Some(
            requested
                .target_interval_seconds
                .unwrap_or(DEFAULT_THUMBNAIL_INTERVAL_SECONDS),
        ),
        storage: Some(requested.storage.unwrap_or_else(default_thumbnail_storage)),
        recording_mode: Some(
            requested
                .recording_mode
                .unwrap_or_else(|| DEFAULT_THUMBNAIL_RECORDING_MODE.to_string()),
        ),
    }
}

/// The HLS configuration a stage reports, with the one member the caller left out filled
/// in from the model's documented default for it.
pub fn effective_hls_configuration(
    requested: Option<ParticipantRecordingHlsConfiguration>,
) -> ParticipantRecordingHlsConfiguration {
    ParticipantRecordingHlsConfiguration {
        target_segment_duration_seconds: Some(
            requested
                .and_then(|configuration| configuration.target_segment_duration_seconds)
                .unwrap_or(DEFAULT_HLS_TARGET_SEGMENT_DURATION_SECONDS),
        ),
    }
}

/// The recording configuration attached to a stage whose request supplied none, or
/// supplied one with no storage configuration: every member at the default the model
/// documents for it.
pub fn default_recording_configuration() -> AutoParticipantRecordingConfiguration {
    AutoParticipantRecordingConfiguration {
        storage_configuration_arn: NO_STORAGE_CONFIGURATION_ARN.to_string(),
        media_types: Some(default_recording_media_types()),
        thumbnail_configuration: Some(effective_thumbnail_configuration(None)),
        recording_reconnect_window_seconds: DEFAULT_RECORDING_RECONNECT_WINDOW_SECONDS,
        hls_configuration: Some(effective_hls_configuration(None)),
        record_participant_replicas: DEFAULT_RECORD_PARTICIPANT_REPLICAS,
    }
}

/// The recording configuration a stage holds, given what its create or update request
/// supplied: the ONE place the configuration's documented defaults are applied, so
/// `CreateStage` and `UpdateStage` attach the same configuration for the same request.
///
/// A request that supplied no configuration, and one that supplied a configuration with
/// an empty `storageConfigurationArn`, both get [`default_recording_configuration`]: the
/// model documents that reset on the member itself — "To disable individual participant
/// recording, set this to `\"\"`; other fields in this object will get reset to their
/// defaults when sending `\"\"`."
///
/// Otherwise each member the caller left out is filled in from its documented default.
/// Two members cannot be "left out" at this seam: `recordingReconnectWindowSeconds` and
/// `recordParticipantReplicas` carry Smithy `@default` traits, so the framework has
/// already materialised the MODEL's default for them by the time a handler sees the
/// request and an absent member is indistinguishable from one the caller sent — their
/// values are reported as they arrived, which is also what honours an explicit `false`
/// on `recordParticipantReplicas` ("Optional field to disable replica participant
/// recording").
pub fn effective_recording_configuration(
    requested: Option<AutoParticipantRecordingConfiguration>,
) -> AutoParticipantRecordingConfiguration {
    let requested = requested.unwrap_or_default();
    if requested.storage_configuration_arn.is_empty() {
        return default_recording_configuration();
    }
    AutoParticipantRecordingConfiguration {
        storage_configuration_arn: requested.storage_configuration_arn,
        media_types: Some(
            requested
                .media_types
                .unwrap_or_else(default_recording_media_types),
        ),
        thumbnail_configuration: Some(effective_thumbnail_configuration(
            requested.thumbnail_configuration,
        )),
        recording_reconnect_window_seconds: requested.recording_reconnect_window_seconds,
        hls_configuration: Some(effective_hls_configuration(requested.hls_configuration)),
        record_participant_replicas: requested.record_participant_replicas,
    }
}

// ---------------------------------------------------------------------------
// A stage's data-plane endpoints
//
// The model's `StageEndpoints` doc tells a caller to cache these values "at stage
// creation", so every stage carries all four from the moment it exists. The HOSTS are
// this emulator's, built from the stage's own id the way the sibling `ivs` emulator
// builds a channel's ingest endpoint and playback URL: derived from persisted state,
// identical on every read of one stage and distinct between stages. The scheme, port and
// path of each are the forms AWS was recorded reporting, which is what a client parsing
// an endpoint depends on.
// ---------------------------------------------------------------------------

/// The `events` endpoint of every stage: one global host, carrying no per-stage part,
/// as AWS was recorded reporting it.
pub const STAGE_EVENTS_ENDPOINT: &str = "wss://global.events.live-video.net";

/// Host suffix of a stage's WHIP endpoint.
const WHIP_HOST_SUFFIX: &str = "global-bm.whip.live-video.net";

/// Host suffix of a stage's RTMP and RTMPS endpoints — the same contribution host the
/// sibling `ivs` emulator names in a channel's ingest endpoint.
const CONTRIBUTE_HOST_SUFFIX: &str = "global-contribute.live-video.net";

/// The four endpoints of the stage `stage_arn` names.
///
/// Keyed off the ARN's own resource id, so the endpoints of a stage are a function of
/// the stage and nothing else.
pub fn stage_endpoints(stage_arn: &str) -> StageEndpoints {
    let stage_id = stage_arn.rsplit('/').next().unwrap_or(stage_arn);
    StageEndpoints {
        events: Some(STAGE_EVENTS_ENDPOINT.to_string()),
        whip: Some(format!("https://{stage_id}.{WHIP_HOST_SUFFIX}")),
        rtmp: Some(format!("rtmp://{stage_id}.{CONTRIBUTE_HOST_SUFFIX}/app/")),
        rtmps: Some(format!(
            "rtmps://{stage_id}.{CONTRIBUTE_HOST_SUFFIX}:443/app/"
        )),
    }
}

/// What `activeSessionId` reports for a stage no session is active in.
///
/// Recorded behaviour: AWS reports the member as the empty string rather than leaving it
/// out, so a caller that reads the field off a stage always finds it and compares it
/// against a session id. (The model's `StageSessionId` `@length 16..16` describes an
/// actual session id; the empty value is the "no session" reading, and output is not
/// constraint-checked.)
pub const NO_ACTIVE_SESSION_ID: &str = "";

/// Page size `ListStages` uses when the request omits `maxResults`.
///
/// The model documents it on `ListStagesRequest$maxResults`: "Maximum number of
/// results to return. Default: 50."
pub const DEFAULT_LIST_PAGE_SIZE: usize = 50;

/// Total size a participant token's `attributes` map may reach.
///
/// The model documents it on `CreateParticipantTokenRequest$attributes` and
/// `ParticipantTokenConfiguration$attributes`: "The maximum length of this field is
/// 1 KB total." It is a documented limit the model carries no constraint trait for,
/// so the handler enforces it and answers `ValidationException`.
///
/// The measure is the one [`attributes_size`] computes: the sum of the key and value
/// bytes the caller sent, with no allowance for the JSON punctuation around them. The
/// model states the limit without stating how it is counted, so the counted thing is
/// the caller's own data.
pub const MAX_TOKEN_ATTRIBUTES_BYTES: usize = 1024;

/// Region an ARN names when the request carries no usable SigV4 credential scope.
pub const DEFAULT_REGION: &str = "us-east-1";

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// `DeleteStage`, so each growth site carries a cap and nothing caller-sized reaches
// the store unchecked. IVS RealTime publishes no per-account stage or participant
// -token quota this emulator can quote, so both values are this emulator's own
// resource guard, set well above any realistic emulated use; a request past either
// is answered with the modelled `ServiceQuotaExceededException` (HTTP 402), which is
// what the real service answers when an account's quota is reached.
//
// The product is therefore finite: at most `MAX_STAGES` stages, each with at most
// `MAX_PARTICIPANT_TOKENS_PER_STAGE` token records, each record holding members the
// model already bounds (`userId` 0..128, `capabilities` 0..2, and `attributes`
// capped at [`MAX_TOKEN_ATTRIBUTES_BYTES`]).
// ---------------------------------------------------------------------------

/// Stages this emulator keeps for one engine. Not an AWS quota — this emulator's
/// own resource guard on the one store `CreateStage` grows.
pub const MAX_STAGES: usize = 1_000;

/// Participant-token records this emulator keeps per stage. Not an AWS quota — this
/// emulator's own resource guard on the store `CreateParticipantToken` grows.
pub const MAX_PARTICIPANT_TOKENS_PER_STAGE: usize = 1_000;

/// Marks a `ListStages` page token as this service's, so a token from elsewhere is
/// refused rather than silently read as a position.
const PAGE_TOKEN_PREFIX: &str = "ivs-realtime-stage-after:";

/// One stage, as `CreateStage` persisted it and `GetStage` / `ListStages` report it.
#[derive(Debug, Clone)]
pub struct StageRecord {
    /// The stage ARN, and the single key every lookup, update and delete uses.
    pub arn: String,
    pub name: Option<String>,
    /// `None` until a session is active in the stage; no operation in this slice
    /// starts one. Reported as [`NO_ACTIVE_SESSION_ID`] while it is `None`.
    pub active_session_id: Option<String>,
    pub tags: HashMap<String, String>,
    /// The stage's individual-participant-recording configuration. Not optional: every
    /// stage has one, with the modelled defaults where the creating request supplied
    /// none ([`effective_recording_configuration`]).
    pub auto_participant_recording_configuration: AutoParticipantRecordingConfiguration,
    /// The stage's data-plane endpoints, assigned at creation ([`stage_endpoints`]).
    /// Not optional: the model's `StageEndpoints` doc tells a caller to cache them "at
    /// stage creation", so a stage without them is not a stage a client can use.
    pub endpoints: StageEndpoints,
    /// When the stage was created. No member of the modelled `Stage` /
    /// `StageSummary` reports it and pagination orders by [`Self::sequence`], so
    /// nothing reads it yet; it is recorded because the stage-session and
    /// participant operations (each its own gap issue) report times relative to the
    /// stage's creation.
    pub created_at: SystemTime,
    /// Creation order, and the stable key `ListStages` pages over, so deleting a
    /// stage mid-pagination cannot shift a later page onto an already-returned one.
    pub sequence: u64,
}

impl StageRecord {
    /// The modelled `Stage` this record reports.
    pub fn to_stage(&self) -> Stage {
        Stage {
            arn: self.arn.clone(),
            name: self.name.clone(),
            // Always reported, the empty value included: see [`NO_ACTIVE_SESSION_ID`].
            active_session_id: Some(self.active_session_id.clone().unwrap_or_default()),
            // Always reported, empty map included: `tags` is how a caller reads the
            // tags back, and an absent member is not the same answer as "none".
            tags: Some(self.tags.clone()),
            auto_participant_recording_configuration: Some(
                self.auto_participant_recording_configuration.clone(),
            ),
            endpoints: Some(self.endpoints.clone()),
        }
    }

    /// The modelled `StageSummary` this record reports in a `ListStages` page.
    pub fn to_summary(&self) -> StageSummary {
        StageSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            // The same member of the same stage as [`Self::to_stage`] reports, so a
            // summary and a read answer alike.
            active_session_id: Some(self.active_session_id.clone().unwrap_or_default()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// One issued participant token, as `CreateStage` and `CreateParticipantToken`
/// persisted it.
#[derive(Debug, Clone)]
pub struct ParticipantTokenRecord {
    pub participant_id: String,
    pub token: String,
    pub user_id: Option<String>,
    pub attributes: HashMap<String, String>,
    pub duration_minutes: i32,
    pub capabilities: Vec<String>,
    pub expiration_time: SystemTime,
}

impl ParticipantTokenRecord {
    /// The modelled `ParticipantToken` this record reports.
    pub fn to_participant_token(&self) -> ParticipantToken {
        ParticipantToken {
            participant_id: Some(self.participant_id.clone()),
            token: Some(self.token.clone()),
            user_id: self.user_id.clone(),
            attributes: (!self.attributes.is_empty()).then(|| self.attributes.clone()),
            duration: Some(self.duration_minutes),
            // Reported only when the token grants something, the same way `attributes`
            // is: `capabilities` is `@length 0..2`, so a caller may legally ask for an
            // empty set, and a token that grants no capability carries no capabilities
            // member rather than an empty list.
            capabilities: (!self.capabilities.is_empty()).then(|| self.capabilities.clone()),
            expiration_time: Some(::aws_smithy_types::DateTime::from(self.expiration_time)),
        }
    }
}

/// Per-service state for com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime. A handler reaches it through [`service_state`].
///
/// ONE store per engine, not one per region: the minted ARN names the region the
/// creating request was signed for, but a stage is reachable from a request signed for
/// any region, which is how this tree stores every service's resources. The real service
/// is region-scoped; partitioning this store by region is a change for the slice that
/// implements the operations that would make the difference observable (a stage session
/// or participant in one region), not something to half-apply here.
#[derive(Debug, Default)]
pub struct ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState {
    /// Every live stage, keyed by its ARN.
    pub stages: HashMap<String, StageRecord>,
    /// The tokens issued against a stage, keyed by that stage's ARN. Removed with
    /// the stage.
    pub participant_tokens: HashMap<String, Vec<ParticipantTokenRecord>>,
    /// Monotonic counter behind every id this service mints, so two stages created
    /// in the same instant still get distinct ids and a distinct page position.
    next_id: u64,
}

impl ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState {
    /// The next value of the service's monotonic id counter.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        self.next_id
    }

    /// Every stage in creation order — the order `ListStages` pages over.
    pub fn stages_in_order(&self) -> Vec<&StageRecord> {
        let mut stages: Vec<&StageRecord> = self.stages.values().collect();
        stages.sort_by_key(|record| record.sequence);
        stages
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState>)
}

/// The modelled 500 a handler answers with when the shared engine cannot hand back
/// this service's store. Unreachable in this emulator (the router builds one
/// `InMemoryEngine` and hands it to every operation), but a handler must answer
/// rather than panic.
///
/// `variant` is the calling operation's OWN modelled error constructor, so the compiler
/// still refuses a failure the operation does not model and the code and status stay
/// model-derived; only the BODY is this service's rendering (see
/// [`crate::scaffolded::Scaffolded::stated`]).
pub fn store_unavailable<E>(variant: fn(InternalServerException) -> E) -> Scaffolded<E> {
    let message = STORE_UNAVAILABLE_MESSAGE.to_string();
    Scaffolded::stated(
        variant(InternalServerException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The text [`store_unavailable`] states. AWS documents no message for this (it is an
/// emulator fault, not a service behaviour), so it says plainly what failed.
pub const STORE_UNAVAILABLE_MESSAGE: &str =
    "the IVS RealTime store is not reachable from the engine serving this request";

/// The modelled 500 a handler answers with when the emulator's credential store minted
/// no secret for a participant token, so no token is issued rather than one this service
/// made up itself.
pub fn token_minting_failed<E>(variant: fn(InternalServerException) -> E) -> Scaffolded<E> {
    let message = TOKEN_MINTING_FAILED_MESSAGE.to_string();
    Scaffolded::stated(
        variant(InternalServerException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The text [`token_minting_failed`] states.
pub const TOKEN_MINTING_FAILED_MESSAGE: &str =
    "the participant token could not be minted, so no token was issued";

/// Characters in a minted stage id. The model's `StageArn` pattern allows
/// `[a-zA-Z0-9-]+` after `stage/`, and caps the whole ARN at 128 characters.
pub const STAGE_ID_LEN: usize = 12;

/// Characters in a minted participant id. The model's `ParticipantTokenId` allows
/// `[a-zA-Z0-9-_]{0,64}`.
pub const PARTICIPANT_ID_LEN: usize = 12;

/// A fresh id for a resource this service mints: `len` lowercase-hex characters, which
/// stays inside the `[a-zA-Z0-9-]` / `[a-zA-Z0-9-_]` id charsets the model's `StageArn`
/// and `ParticipantTokenId` patterns allow.
///
/// The leading digits are the store's monotonic `sequence`, which is distinct for every
/// resource this store mints, so two ids of one store run can never collide. The trailing
/// digits are the low bits of the creation instant, so a store that starts over after a
/// restart does not re-issue the ids of the run before it.
pub fn mint_id(sequence: u64, len: usize) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    // `% 0x1_0000_0000` keeps the four low hex bytes of the instant; the cast cannot
    // truncate after it.
    let instant = u64::try_from(nanos % 0x1_0000_0000).unwrap_or(0);
    let mut id = format!("{sequence:06x}{instant:08x}");
    id.truncate(len);
    id
}

/// The stage ARN for `stage_id`, in the form the model's `StageArn` pattern
/// requires: `arn:aws:ivs:<region>:<account>:stage/<id>`.
pub fn stage_arn(region: &str, stage_id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("stage/{stage_id}"))
}

/// When a token issued now for `duration_minutes` expires.
pub fn expiration_time(duration_minutes: i32) -> SystemTime {
    let minutes = u64::try_from(duration_minutes).unwrap_or(0);
    SystemTime::now() + Duration::from_secs(minutes.saturating_mul(60))
}

/// The region a minted ARN names: the one the request was signed for, so a caller
/// reads back the region it addressed.
///
/// Falls back to [`DEFAULT_REGION`] for an unsigned request, and for a credential scope
/// whose region is not a token the model's ARN patterns accept (`[a-z0-9-]+`). It lives
/// here, beside that constant, because it is the rule for every ARN this service mints,
/// not just the stage ARNs of this slice.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// What one request supplies for one participant token: the four members
/// `ParticipantTokenConfiguration` (a `CreateStage` entry) and
/// `CreateParticipantTokenRequest` have in common. Both operations hand this to
/// [`issue_token_record`], so neither can size, name or empower a token differently from
/// the other.
#[derive(Debug, Default)]
pub struct RequestedToken {
    pub user_id: Option<String>,
    pub attributes: Option<HashMap<String, String>>,
    pub duration: Option<i32>,
    pub capabilities: Option<Vec<String>>,
}

/// Mint one participant token for `stage_arn` from what the request supplied, applying
/// the model's documented defaults to what it left out.
///
/// `in_flight` holds the records this same request has already minted but not yet
/// stored, so the id check covers them too: `CreateStage` issues up to twelve tokens
/// before any of them reaches the store.
///
/// The ONE place a token record is built: `CreateStage` (once per
/// `participantTokenConfigurations` entry) and `CreateParticipantToken` both call it, so
/// the id length, the documented defaults and the minting cannot drift apart between
/// them. The record is returned rather than stored: each caller owns where it belongs in
/// the store, and a request refused after this point persists nothing.
///
/// `None` only when the credential store minted no secret for the token, which the caller
/// answers with [`token_minting_failed`] rather than by returning a token this service
/// made up.
pub fn issue_token_record(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    credentials: &::emulator_server::CredentialStore,
    stage_arn: &str,
    in_flight: &[ParticipantTokenRecord],
    requested: RequestedToken,
) -> Option<ParticipantTokenRecord> {
    // A fresh id per issuance, re-minted on the (vanishingly unlikely) chance it already
    // names a participant of this stage — stored or minted earlier in this same request —
    // the same uniqueness rule `CreateStage` applies to a minted stage ARN, so both ids
    // this service mints are unique by construction and not by probability.
    let mut participant_id = mint_id(state.next_sequence(), PARTICIPANT_ID_LEN);
    while in_flight
        .iter()
        .any(|record| record.participant_id == participant_id)
        || state
            .participant_tokens
            .get(stage_arn)
            .is_some_and(|records| {
                records
                    .iter()
                    .any(|record| record.participant_id == participant_id)
            })
    {
        participant_id = mint_id(state.next_sequence(), PARTICIPANT_ID_LEN);
    }

    let token = mint_participant_token(credentials)?;
    let duration_minutes = requested
        .duration
        .unwrap_or(DEFAULT_TOKEN_DURATION_MINUTES);
    Some(ParticipantTokenRecord {
        participant_id,
        token,
        user_id: requested.user_id,
        attributes: requested.attributes.unwrap_or_default(),
        duration_minutes,
        capabilities: requested
            .capabilities
            .unwrap_or_else(default_token_capabilities),
        expiration_time: expiration_time(duration_minutes),
    })
}

/// Total size of an `attributes` map, against [`MAX_TOKEN_ATTRIBUTES_BYTES`]: the sum of
/// the key and value bytes the caller sent.
pub fn attributes_size(attributes: &HashMap<String, String>) -> usize {
    attributes
        .iter()
        .map(|(key, value)| key.len() + value.len())
        .sum()
}

/// The client token a participant joins the stage with.
///
/// The token is a secret handed to the caller, and nothing in a service may construct a
/// secret itself, so it is minted by the one component of this emulator that generates
/// secrets — the credential store — and then removed from that store again: the caller
/// gets the secret, the emulator keeps no credential it would accept as SigV4 (this token
/// authenticates to a stage, not to the emulator's API), and nothing accumulates per
/// call. The same pattern the MWAA and Grafana emulators use for their caller-facing
/// tokens.
///
/// The model's own `ParticipantToken` doc fixes the contract a caller may rely on: "Treat
/// tokens as opaque; i.e., do not build functionality based on token contents. The format
/// of tokens could change in the future." So the only properties that matter are the ones
/// this gives: every token is distinct, and none is echoed back from the request.
///
/// `None` when the store minted no session-token secret, which leaves the operation to
/// answer [`token_minting_failed`] rather than report a token it made up.
pub fn mint_participant_token(credentials: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = credentials.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(str::to_string);
    // The participant token is not a credential of this emulator; see above.
    let _ = credentials.delete(&minted.id);
    token
}

/// The opaque `nextToken` a `ListStages` page ends with: the creation sequence the
/// next page resumes after.
///
/// The codec is `aws_smithy_types::base64`, the standard-alphabet encoder the wire
/// serializer already uses for every blob, and its output stays inside the model's
/// `PaginationToken` pattern `^[a-zA-Z0-9+/=_-]*$`.
pub fn encode_page_token(after_sequence: u64) -> String {
    ::aws_smithy_types::base64::encode(format!("{PAGE_TOKEN_PREFIX}{after_sequence}"))
}

/// The creation sequence a `nextToken` resumes after, or `None` when the token was
/// not issued by [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Option<u64> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_PREFIX)?.parse().ok()
}

// =============================================================================
// How this service WORDS the failures it answers
// =============================================================================
//
// Every message below is built from the REQUEST (the resource it named, the member it
// sent, the quota it met) by one rule, stated once here and used by every handler that
// answers that failure, so two operations cannot word the same refusal two ways. The
// error CODE and the HTTP status are NOT here: they stay model-derived, on the modelled
// error shape the operation declares (see `crate::scaffolded::Scaffolded::stated`).

/// How this service states a request that named a resource it does not hold: the
/// identifier the request named, under a fixed template.
///
/// One rule for every operation that takes an identifier. It is the same sentence the
/// sibling IVS endpoints were recorded answering (`Resource: <arn> not found` for the
/// low-latency `ivs` service's `GetChannel` / `UpdateChannel` / `CreateStreamKey`, and for
/// `ivschat`), and the recorded IVS RealTime `CreateParticipantToken` not-found carries it
/// too, which is what makes it a rule of the family rather than wording of one operation.
pub fn resource_not_found_message(identifier: &str) -> String {
    format!("Resource: {identifier} not found")
}

/// How this service states a quota refusal (`ServiceQuotaExceededException`, HTTP 402):
/// it names the resource the quota is on and the limit that was reached.
pub fn quota_exceeded_message(resource: &str, limit: usize) -> String {
    format!("Resource: {resource} quota exceeded limit: {limit}")
}

/// The `ResourceNotFoundException` (HTTP 404) a handler answers when a request names a
/// resource this service does not hold. See [`resource_not_found_message`].
///
/// `variant` is the calling operation's OWN modelled error constructor —
/// `GetStageErrorModeled::ResourceNotFoundException`, say — so the compiler still refuses
/// a failure the operation's model does not declare, and the error code and HTTP status
/// stay derived from that modelled shape. Only the BODY is this service's own rendering:
/// AWS puts the text under the key `message`, which the modelled `exceptionMessage` member
/// is not (see `crate::scaffolded::STATED_ERROR_SCHEMA`). The modelled struct is filled in
/// with the same text, so the error is complete as a Rust value too.
pub fn resource_not_found<E>(
    variant: fn(ResourceNotFoundException) -> E,
    identifier: &str,
) -> Scaffolded<E> {
    let message = resource_not_found_message(identifier);
    Scaffolded::stated(
        variant(ResourceNotFoundException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The `ValidationException` (HTTP 400) a handler answers for a request it refuses for a
/// reason the model cannot express. The caller supplies the sentence, built from the
/// request by one of the rules above this line or by [`stated_violations`].
pub fn validation_failed<E>(
    variant: fn(ValidationException) -> E,
    message: impl Into<String>,
) -> Scaffolded<E> {
    let message = message.into();
    Scaffolded::stated(
        variant(ValidationException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The `ServiceQuotaExceededException` (HTTP 402) a handler answers when a create would
/// exceed a quota. See [`quota_exceeded_message`].
pub fn quota_exceeded<E>(
    variant: fn(ServiceQuotaExceededException) -> E,
    resource: &str,
    limit: usize,
) -> Scaffolded<E> {
    let message = quota_exceeded_message(resource, limit);
    Scaffolded::stated(
        variant(ServiceQuotaExceededException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

// =============================================================================
// How this service states a CONSTRAINT refusal
// =============================================================================
//
// The framework's constraint walk runs between deserialize and `handle` and words a
// violation the same way for every service ("1 validation error detected: Value '…' at
// '…' failed to satisfy constraint: …"). IVS RealTime words it from the OPERATION and the
// member path instead ("CreateParticipantToken.duration is required to have a maximum
// value of 20160.0, found 20161.0" — recorded), and the operation is not reachable at the
// framework's refusal seam: `Service::override_refusal` is handed the violations alone.
// `crate::service`'s per-operation wrapper collects the framework's OWN violations and
// brings the operation with it; the sentences are here, beside the message rules above,
// and not one modelled constraint is restated — every bound and pattern a clause prints
// comes off the `Violation` the walk produced.

/// A numeric bound or submitted value as this service's validation layer renders it: the
/// decimal form, so an integral value carries a `.0` (`20160` reads as `20160.0`).
///
/// The bound arrives as the text the constraint trait displays with and the submitted
/// value as the text the request carried, so a value that already shows a fraction is
/// left exactly as it is.
fn as_decimal(value: &str) -> String {
    if value.contains('.') {
        value.to_string()
    } else {
        format!("{value}.0")
    }
}

/// One violation as this service states it: the operation, the dotted member path and
/// the constraint the request violated.
///
/// The `@range` form is the recorded one. The remaining kinds are derived from it by the
/// same construction — the operation and member path, then the requirement — because
/// AWS's exact wording for those is not recorded; the ERROR CODE and status they travel
/// with are the modelled ones either way.
pub fn violation_clause(operation: &str, violation: &Violation) -> String {
    let path = violation.path.as_str();
    let found = violation.value.as_deref().unwrap_or_default();
    match &violation.kind {
        ViolationKind::MinValue(minimum) => format!(
            "{operation}.{path} is required to have a minimum value of {}, found {}",
            as_decimal(minimum),
            as_decimal(found),
        ),
        ViolationKind::MaxValue(maximum) => format!(
            "{operation}.{path} is required to have a maximum value of {}, found {}",
            as_decimal(maximum),
            as_decimal(found),
        ),
        ViolationKind::Pattern(pattern) => {
            format!("{operation}.{path} does not match the required pattern {pattern}")
        }
        ViolationKind::MinLength(minimum) => format!(
            "{operation}.{path} is required to have a minimum length of {minimum}, found {}",
            found_length(violation),
        ),
        ViolationKind::MaxLength(maximum) => format!(
            "{operation}.{path} is required to have a maximum length of {maximum}, found {}",
            found_length(violation),
        ),
        ViolationKind::MissingRequiredMember => format!("{operation}.{path} is required"),
        ViolationKind::EnumValue(allowed) => format!(
            "{operation}.{path} is required to be one of [{}]",
            allowed.join(", ")
        ),
        ViolationKind::UniqueItems => {
            format!("{operation}.{path} is required to have unique values")
        }
        // `ViolationKind` is `#[non_exhaustive]`: a constraint class the framework gains
        // later is stated in the same construction rather than silently unworded.
        _ => format!("{operation}.{path} does not satisfy the constraint the model declares"),
    }
}

/// `violations` as this service states them for `operation`, or `None` when there is
/// nothing to refuse.
///
/// The recorded refusal carries ONE violation and states its sentence alone — no count
/// prefix, unlike the framework's envelope — so one violation is its clause; several are
/// joined, in the order the walk collected them.
pub fn stated_violations(operation: &str, violations: &[Violation]) -> Option<String> {
    match violations {
        [] => None,
        [only] => Some(violation_clause(operation, only)),
        many => Some(
            many.iter()
                .map(|violation| violation_clause(operation, violation))
                .collect::<Vec<String>>()
                .join("; "),
        ),
    }
}

/// The length the request actually sent, for a `@length` violation.
///
/// A `Violation` carries WHAT was sent, not its size, so the size is read off that
/// rendering: the framework renders a collection as `"<n> items"` and a string as the
/// string itself. A string longer than the framework's echo cap arrives elided, which is
/// the one case this count is short — it affects the number in the message and nothing
/// about the refusal.
fn found_length(violation: &Violation) -> usize {
    let Some(value) = violation.value.as_deref() else {
        return 0;
    };
    value
        .strip_suffix(" items")
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| value.chars().count())
}

/// The violation for a `@required` member the request did not send.
///
/// Absence is found while READING the request, so it arrives as a codec error rather
/// than from the constraint walk; this is the same `Violation` the framework's own
/// `refusal_of` builds for it, so one refusal is not stated two ways.
pub fn missing_member_violation(member_name: String) -> Violation {
    Violation {
        path: member_name,
        kind: ViolationKind::MissingRequiredMember,
        value: None,
    }
}

// =============================================================================
// The tagging surface: the `resourceArn` it admits, and the resource it names
// =============================================================================
//
// `ListTagsForResource` / `TagResource` / `UntagResource` all carry the resource's ARN
// in the `/tags/{resourceArn}` path label, and all three target the same `ResourceArn`
// shape. Both halves of that surface live here: the admission rule for the label, and
// the one lookup that maps an ARN onto the stored resource whose tags it names, so the
// three operations cannot disagree about either.

/// The HTTP status a successful tagging call that has NO content to report answers with.
///
/// `TagResource` and `UntagResource` leave their success code defaulted in the model
/// (`@http` carries no `code:`, i.e. `codeDefaulted: true`), so the Smithy default of 200
/// is not a statement about this service — the status is a behavioural fact, and the
/// recorded AWS responses are what states it: both answer `204 No Content` with an empty
/// body, which is what their output structures carry (`TagResourceResponse` and
/// `UntagResourceResponse` declare no members). A 200 there promises a body the response
/// does not carry, so every client path gated on 204 — conditional body parsing, caches
/// and proxies that key on it — takes the wrong branch.
///
/// `ListTagsForResource` keeps the defaulted 200: it HAS content, the `tags` map.
pub const NO_CONTENT_SUCCESS_STATUS: u16 = 204;

/// The modelled `ResourceArn` `@pattern` with its resource-type element read as
/// one-or-more of the characters the model's own character class names.
///
/// The model writes that element once (`[a-z-]`), which matches exactly ONE character,
/// so taken literally no IVS ARN that exists could satisfy it — every resource type in
/// this model's ARN shapes (`stage`, `encoder-configuration`, `ingest-configuration`,
/// `storage-configuration`, `composition`, `public-key`) is longer than one character,
/// and the recordings show AWS accepting `…:stage/<id>` on all three tagging operations
/// and round-tripping that stage's tags. Repeating the group is the only reading of the
/// model under which the recorded behaviour is possible; everything else about the
/// pattern — the literal `arn:aws:ivs:` prefix, the region, account and id classes — is
/// the model's, unchanged.
const RESOURCE_ARN_PATTERN: &str = r"^arn:aws:ivs:[a-z0-9-]+:[0-9]+:[a-z-]+/[a-zA-Z0-9-]+$";

/// `ResourceArn`: the modelled `@length(1..128)` plus [`RESOURCE_ARN_PATTERN`].
static RESOURCE_ARN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(128))));
    traits.insert(Box::new(PatternTrait::new(RESOURCE_ARN_PATTERN)));
    traits
});

/// The `resourceArn` member schema the framework's constraint walk reads: its name gives
/// the violation path and its trait map carries the constraints.
static M_RESOURCE_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ivsrealtime#TaggedResource$resourceArn",
        "com.amazonaws.ivsrealtime",
        "resourceArn",
    ),
    ShapeType::String,
    "resourceArn",
    0,
)
.with_traits(&RESOURCE_ARN_TRAITS);

/// The root of the shadow input the walk is driven over. Its member list is empty on
/// purpose: the walk visits what [`TaggedResource`] WRITES, and a root struct schema
/// carries no member name, so it contributes no path segment.
static TAGGED_RESOURCE_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.ivsrealtime#TaggedResource",
        "com.amazonaws.ivsrealtime",
        "TaggedResource",
    ),
    ShapeType::Structure,
    &[],
);

/// One request's `resourceArn`, written for the validating walk the way the codec writes
/// a string member.
struct TaggedResource<'a> {
    resource_arn: &'a str,
}

impl SerializableStruct for TaggedResource<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&M_RESOURCE_ARN, self.resource_arn)
    }
}

/// Check a tagging call's `resourceArn` against the member's modelled `@length` and
/// [`RESOURCE_ARN_PATTERN`]. `Err` is the sentence to put on the `ValidationException`.
///
/// The check is run by the FRAMEWORK's own constraint walk over the member schema above,
/// and the refusal is worded by [`stated_violations`] — the same two steps every other
/// refusal of this service goes through — so an ARN refused here is refused and worded
/// exactly as one the boundary walk refuses. The boundary cannot do it itself: the
/// modelled pattern is unsatisfiable (see [`RESOURCE_ARN_PATTERN`]), so each tagging
/// input carries `skip_validation = ["resourceArn"]` and this takes its place.
pub fn check_resource_arn(operation: &str, resource_arn: &str) -> Result<(), String> {
    let violations: Vec<Violation> = validate(&TaggedResource { resource_arn }, &TAGGED_RESOURCE_SCHEMA);
    match stated_violations(operation, &violations) {
        None => Ok(()),
        Some(message) => Err(message),
    }
}

/// Tags one resource may carry, from the modelled `Tags` `@length(0..50)`
/// (`smithy_query(query="shape", shape="Tags")`).
///
/// The framework's walk bounds the map ONE REQUEST sent; the bound is on the RESOURCE's
/// tag set, so `TagResource` checks the set its call would leave behind — a rule about
/// stored state, which no input shape expresses.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Refuse a call that would leave a resource carrying more than
/// [`MAX_TAGS_PER_RESOURCE`] tags. `Err` is the sentence to put on the
/// `ValidationException`.
///
/// Worded through [`violation_clause`], so the refusal reads like the `@length` refusal
/// the framework's walk words for a map member — the operation, the member path, the
/// maximum and what the request would leave.
pub fn check_tag_count(operation: &str, found: usize) -> Result<(), String> {
    if found <= MAX_TAGS_PER_RESOURCE {
        return Ok(());
    }
    Err(violation_clause(
        operation,
        &Violation {
            path: "tags".to_string(),
            kind: ViolationKind::MaxLength(
                i64::try_from(MAX_TAGS_PER_RESOURCE).unwrap_or(i64::MAX),
            ),
            // The walk renders a collection as `"<n> items"`, which is what
            // `violation_clause` reads the found count back off.
            value: Some(format!("{found} items")),
        },
    ))
}

/// The resource type an IVS ARN names (`stage`, `encoder-configuration`, …), or `None`
/// when the string carries no sixth `:`-field at all.
///
/// The type is that field up to its `/`, from the ARN form every ARN shape in this model
/// pins: `arn:aws:ivs:<region>:<account>:<type>/<id>`.
pub fn arn_resource_type(arn: &str) -> Option<&str> {
    arn.split(':').nth(5)?.split('/').next()
}

/// The tags attached to the resource `arn` names, or `None` when this store holds no
/// such resource.
///
/// A stage is the one resource type this store can hold: `CreateStage` is the only
/// create this service implements, so every other resource type in its ARN shapes names
/// nothing, and a well-formed ARN naming one is answered exactly as an ARN naming a
/// deleted stage is — not found. Each such create is its own gap issue, and when one
/// lands its record joins the match below.
pub fn resource_tags(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    arn: &str,
) -> Option<HashMap<String, String>> {
    match arn_resource_type(arn)? {
        "stage" => state.stages.get(arn).map(|stage| stage.tags.clone()),
        _ => None,
    }
}

/// The `tags` of the resource `arn` names, to be changed in place. `None` when this
/// store holds no such resource; see [`resource_tags`].
pub fn resource_tags_mut<'a>(
    state: &'a mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    arn: &str,
) -> Option<&'a mut HashMap<String, String>> {
    match arn_resource_type(arn)? {
        "stage" => state.stages.get_mut(arn).map(|stage| &mut stage.tags),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bound and a submitted value reach the recorded decimal rendering, and a value
    /// that already carries a fraction is not given a second one.
    #[test]
    fn a_numeric_bound_is_stated_in_decimal_form() {
        let cases = [
            ("an integral bound", "20160", "20160.0"),
            ("one", "1", "1.0"),
            ("zero", "0", "0.0"),
            ("a negative value", "-3", "-3.0"),
            ("an already-fractional value", "2.5", "2.5"),
        ];
        for (case, value, expected) in cases {
            let actual = as_decimal(value);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Every violation class states the operation, the member path and the requirement.
    #[test]
    fn a_violation_clause_names_the_operation_and_the_member() {
        let cases = [
            (
                "a range maximum",
                Violation {
                    path: "duration".to_string(),
                    kind: ViolationKind::MaxValue("20160".to_string()),
                    value: Some("20161".to_string()),
                },
                "CreateParticipantToken.duration is required to have a maximum value of \
                 20160.0, found 20161.0",
            ),
            (
                "a range minimum",
                Violation {
                    path: "duration".to_string(),
                    kind: ViolationKind::MinValue("1".to_string()),
                    value: Some("0".to_string()),
                },
                "CreateParticipantToken.duration is required to have a minimum value of 1.0, \
                 found 0.0",
            ),
            (
                "a pattern",
                Violation {
                    path: "stageArn".to_string(),
                    kind: ViolationKind::Pattern("^arn:aws:ivs:".to_string()),
                    value: Some("nope".to_string()),
                },
                "CreateParticipantToken.stageArn does not match the required pattern \
                 ^arn:aws:ivs:",
            ),
            (
                "a minimum length",
                Violation {
                    path: "stageArn".to_string(),
                    kind: ViolationKind::MinLength(1),
                    value: Some(String::new()),
                },
                "CreateParticipantToken.stageArn is required to have a minimum length of 1, \
                 found 0",
            ),
            (
                "a required member",
                missing_member_violation("stageArn".to_string()),
                "CreateParticipantToken.stageArn is required",
            ),
        ];
        for (case, violation, expected) in cases {
            let actual = violation_clause("CreateParticipantToken", &violation);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}


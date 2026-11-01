// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime service.
//!
//! The stage lifecycle (`CreateStage`, `GetStage`, `ListStages`, `UpdateStage`,
//! `DeleteStage`), the token issuance that makes a stage usable
//! (`CreateParticipantToken`), the stage sessions a stage's participants are
//! grouped into (`GetParticipant`, `ListParticipants`), the occurrences during one of
//! those sessions (`ListParticipantEvents`), the compositions a stage is composed
//! into (`StartComposition`, `GetComposition`, `StopComposition`) and the public keys a
//! customer signs its own participant tokens with (`ImportPublicKey`, `GetPublicKey`,
//! `ListPublicKeys`) all
//! read and write the records below, so the records, the id minting they share and the
//! modelled defaults they apply live here rather than in any one operation's file.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{validate, LengthTrait, PatternTrait};
use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    AutoParticipantRecordingConfiguration, Composition, CompositionRecordingHlsConfiguration,
    CompositionSummary, CompositionThumbnailConfiguration, ConflictException,
    CreateIngestConfigurationOutput, Destination, DestinationConfiguration, DestinationDetail,
    DestinationSummary, EncoderConfiguration, EncoderConfigurationSummary, Event,
    GridConfiguration, IngestConfiguration,
    IngestConfigurationSummary, InternalServerException, LayoutConfiguration, Participant,
    ParticipantRecordingHlsConfiguration, ParticipantReplica, ParticipantSummary,
    ParticipantThumbnailConfiguration, ParticipantToken, PipConfiguration, PublicKey,
    PublicKeySummary, RecordingConfiguration, RedundantIngestCredential,
    ResourceNotFoundException,
    S3DestinationConfiguration, S3StorageConfiguration,
    ServiceQuotaExceededException, Stage, StageEndpoints, StageSession, StageSessionSummary,
    StageSummary,
    StartParticipantReplicationOutput, StopParticipantReplicationOutput, StorageConfiguration,
    StorageConfigurationSummary, UpdateIngestConfigurationOutput, ValidationException, Video,
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
///
/// The model declares `@default false` for the same member, so the shape keeps the
/// member's PRESENCE (see the hand-written `AutoParticipantRecordingConfiguration` in
/// `crate::ops::types`) and this default applies only where the request carried no
/// value: a `false` a caller sends to disable replica participant recording is reported
/// as it arrived.
pub const DEFAULT_RECORD_PARTICIPANT_REPLICAS: bool = true;

/// Thumbnail-generation interval, when the thumbnail configuration does not set one.
///
/// Documented on `ParticipantThumbnailConfiguration$targetIntervalSeconds`: "The targeted
/// thumbnail-generation interval in seconds. … Default: 60." The thumbnail configuration
/// of a composition destination states the same value for the same member —
/// `CompositionThumbnailConfiguration$targetIntervalSeconds`: "The targeted
/// thumbnail-generation interval in seconds. Default: 60." — so both are reported from
/// here.
pub const DEFAULT_THUMBNAIL_INTERVAL_SECONDS: i32 = 60;

/// Thumbnail storage format, when the thumbnail configuration names none.
///
/// Documented on `ParticipantThumbnailConfiguration$storage`: "Indicates the format in
/// which thumbnails are recorded. … Default: `SEQUENTIAL`." The composition-side member
/// `CompositionThumbnailConfiguration$storage` states the same value ("You can enable
/// both `SEQUENTIAL` and `LATEST`. Default: `SEQUENTIAL`."), so both are reported from
/// here.
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

/// The value a member reports when the request left it out: `documented`, the default
/// the member's own doc states — never the Smithy `@default` the framework would
/// materialise for it.
///
/// `requested` is what arrived, which is `None` exactly when the request did not carry
/// the member: the shapes whose documented default differs from their declared
/// `@default` keep that presence (see `crate::ops::types`), so a value the caller DID
/// send — the declared default included — is reported as it arrived, and the documented
/// default is reported only in its absence. One member, one default, whatever its
/// siblings carry.
pub fn documented_default<T>(requested: Option<T>, documented: T) -> T {
    requested.unwrap_or(documented)
}

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
        recording_reconnect_window_seconds: Some(DEFAULT_RECORDING_RECONNECT_WINDOW_SECONDS),
        hls_configuration: Some(effective_hls_configuration(None)),
        record_participant_replicas: Some(DEFAULT_RECORD_PARTICIPANT_REPLICAS),
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
/// Otherwise each member the caller left out is filled in from its documented default,
/// through [`documented_default`] for the two the model also declares a Smithy
/// `@default` for — the shape keeps their presence, so a value the caller sent is
/// reported as it arrived and the documented default applies only where the member was
/// absent.
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
        recording_reconnect_window_seconds: Some(documented_default(
            requested.recording_reconnect_window_seconds,
            DEFAULT_RECORDING_RECONNECT_WINDOW_SECONDS,
        )),
        hls_configuration: Some(effective_hls_configuration(requested.hls_configuration)),
        // The same documented default [`default_recording_configuration`] reports, so
        // one member of one shape has ONE default whatever its siblings carry.
        record_participant_replicas: Some(documented_default(
            requested.record_participant_replicas,
            DEFAULT_RECORD_PARTICIPANT_REPLICAS,
        )),
    }
}

/// Whether `configuration` records a participant that is a REPLICA of another stage.
///
/// `recordParticipantReplicas` is the member that says so — "Optional field to disable
/// replica participant recording" — carrying what the request set it to where it set it,
/// and [`DEFAULT_RECORD_PARTICIPANT_REPLICAS`] where it did not. The ONE reading of that
/// member, so what a read of a participant reports about its recording and what an update
/// refuses over cannot disagree about it.
pub fn records_participant_replicas(
    configuration: &AutoParticipantRecordingConfiguration,
) -> bool {
    documented_default(
        configuration.record_participant_replicas,
        DEFAULT_RECORD_PARTICIPANT_REPLICAS,
    )
}

/// The dotted path of the member that names the storage configuration a stage records
/// its participants into, as the wire spells it — what a refusal of that member names.
pub const RECORDING_STORAGE_CONFIGURATION_MEMBER: &str =
    "autoParticipantRecordingConfiguration.storageConfigurationArn";

/// The resource kind a recording configuration's reference names, as
/// [`unresolved_reference_message`] states it.
const STORAGE_CONFIGURATION_RESOURCE: &str = "storage configuration";

/// The recording configuration a stage holds, given what its create or update request
/// supplied AND the storage configurations this account actually holds: the ONE place a
/// supplied configuration is both defaulted ([`effective_recording_configuration`]) and
/// resolved, so a create and an update accept and refuse the same request.
///
/// A non-empty `storageConfigurationArn` is a REFERENCE to a storage configuration of
/// this same service ("ARN of the StorageConfiguration resource to use for individual
/// participant recording"), so one this store holds no configuration for is refused
/// instead of being copied into the stage: a stage attached to a configuration that does
/// not exist would report recording as wired up while nothing can record. `Err` is the
/// sentence to put on the calling operation's `ValidationException`, worded by
/// [`unresolved_reference_message`]; `operation` is that operation's name. Called before
/// anything is minted or written, so a refused request leaves the store as it was.
///
/// The empty string names no resource — it is the documented "no storage configuration"
/// value — so it is resolved by nothing and keeps the documented reset.
///
/// The reference is resolved in `region`'s view of the store, the region the calling
/// request was processed in: a configuration of another region is not one this account
/// view holds, so it resolves to nothing exactly as an ARN nothing was ever created for
/// does ([`in_request_region`]).
pub fn resolved_recording_configuration(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    operation: &str,
    requested: Option<AutoParticipantRecordingConfiguration>,
    region: &str,
) -> Result<AutoParticipantRecordingConfiguration, String> {
    let configuration = effective_recording_configuration(requested);
    let arn = configuration.storage_configuration_arn.as_str();
    if !arn.is_empty() && state.storage_configuration_in(region, arn).is_none() {
        return Err(unresolved_reference_message(
            operation,
            RECORDING_STORAGE_CONFIGURATION_MEMBER,
            STORAGE_CONFIGURATION_RESOURCE,
            arn,
        ));
    }
    Ok(configuration)
}

/// The member that names the stage an ingest configuration is associated with, as the
/// wire spells it — what a refusal of that member names.
pub const INGEST_CONFIGURATION_STAGE_MEMBER: &str = "stageArn";

/// The resource kind a stage reference names, as [`unresolved_reference_message`]
/// states it.
const STAGE_RESOURCE: &str = "stage";

/// The stage an ingest configuration is associated with, given what its create or update
/// request supplied AND the stages this account actually holds: the ONE place a supplied
/// association is both defaulted and resolved, so a create and an update accept and
/// refuse the same request.
///
/// A non-empty `stageArn` is a REFERENCE to a stage of this same service ("ARN of the
/// stage with which the IngestConfiguration is associated"), so one this store holds no
/// stage for is refused instead of being persisted: the association is what a publisher's
/// ingest is routed into, so a configuration pointing at a stage that does not exist
/// would report an association nothing can serve. `Err` is the sentence to put on the
/// calling operation's `ValidationException`, worded by [`unresolved_reference_message`];
/// `operation` is that operation's name. Called before anything is minted or written, so
/// a refused request leaves the store as it was.
///
/// An absent member, and the empty string the modelled `IngestConfigurationStageArn`
/// admits, both name no stage — [`NO_ASSOCIATED_STAGE_ARN`] — so they are resolved by
/// nothing.
///
/// The reference is resolved in `region`'s view of the store, the region the calling
/// request was processed in, the same way a stage's recording reference is: a stage of
/// another region is not one this account view holds.
pub fn resolved_stage_association(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    operation: &str,
    requested: Option<String>,
    region: &str,
) -> Result<String, String> {
    let stage_arn = requested.unwrap_or_else(|| NO_ASSOCIATED_STAGE_ARN.to_string());
    if stage_arn != NO_ASSOCIATED_STAGE_ARN && state.stage_in(region, &stage_arn).is_none() {
        return Err(unresolved_reference_message(
            operation,
            INGEST_CONFIGURATION_STAGE_MEMBER,
            STAGE_RESOURCE,
            &stage_arn,
        ));
    }
    Ok(stage_arn)
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

/// Page size a listing uses when the request omits `maxResults`.
///
/// The model documents it on `ListStagesRequest$maxResults` ("Maximum number of
/// results to return. Default: 50.") and states the same default on
/// `ListParticipantsRequest$maxResults`, `ListParticipantReplicasRequest$maxResults`,
/// `ListParticipantEventsRequest$maxResults` and `ListStageSessionsRequest$maxResults`,
/// and the same again on `ListPublicKeysRequest$maxResults`, so those listings page by
/// it.
pub const DEFAULT_LIST_PAGE_SIZE: usize = 50;

/// Total size an `attributes` map may reach, on every shape of this service that
/// carries one.
///
/// The model documents it on `CreateParticipantTokenRequest$attributes`,
/// `ParticipantTokenConfiguration$attributes` and
/// `CreateIngestConfigurationRequest$attributes` — each "The maximum length of this
/// field is 1 KB total." It is a documented limit the model carries no constraint trait
/// for, so the handler enforces it and answers `ValidationException`.
///
/// The measure is the one [`attributes_size`] computes: the sum of the key and value
/// bytes the caller sent, with no allowance for the JSON punctuation around them. The
/// model states the limit without stating how it is counted, so the counted thing is
/// the caller's own data.
pub const MAX_ATTRIBUTES_BYTES: usize = 1024;

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
// capped at [`MAX_ATTRIBUTES_BYTES`]).
// ---------------------------------------------------------------------------

/// Stages this emulator keeps for one engine. Not an AWS quota — this emulator's
/// own resource guard on the one store `CreateStage` grows.
pub const MAX_STAGES: usize = 1_000;

/// Participant-token records this emulator keeps per stage. Not an AWS quota — this
/// emulator's own resource guard on the store `CreateParticipantToken` grows.
pub const MAX_PARTICIPANT_TOKENS_PER_STAGE: usize = 1_000;

/// Ingest configurations this emulator keeps for one engine. Not an AWS quota — this
/// emulator's own resource guard on the one store `CreateIngestConfiguration` grows,
/// set the same way [`MAX_STAGES`] is and answered with the same modelled
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_INGEST_CONFIGURATIONS: usize = 1_000;

/// Encoder configurations this emulator keeps for one engine. Not an AWS quota — this
/// emulator's own resource guard on the one store `CreateEncoderConfiguration` grows,
/// set the same way [`MAX_STAGES`] is and answered with the same modelled
/// `ServiceQuotaExceededException` (HTTP 402). A record is reclaimed only by an
/// explicit `DeleteEncoderConfiguration`.
pub const MAX_ENCODER_CONFIGURATIONS: usize = 1_000;

// ---------------------------------------------------------------------------
// The modelled defaults of an encoder configuration's video settings
//
// `EncoderConfiguration$video` is reported for every encoder configuration, and each
// member of `Video` is optional with a documented default, so a request that supplies
// none — or only some — still reads back a complete video configuration. Every constant
// below cites the member doc it comes from; none is this emulator's choice.
// ---------------------------------------------------------------------------

/// Video-resolution width of an encoder configuration whose request did not set one.
///
/// Documented on `Video$width`: "Video-resolution width. This must be an even number.
/// … Default: 1280."
pub const DEFAULT_VIDEO_WIDTH: i32 = 1280;

/// Video-resolution height of an encoder configuration whose request did not set one.
///
/// Documented on `Video$height`: "Video-resolution height. This must be an even number.
/// … Default: 720."
pub const DEFAULT_VIDEO_HEIGHT: i32 = 720;

/// Frame rate of an encoder configuration whose request did not set one.
///
/// Documented on `Video$framerate`: "Video frame rate, in fps. Default: 30."
pub const DEFAULT_VIDEO_FRAMERATE: f32 = 30.0;

/// Output bitrate, in bps, of an encoder configuration whose request did not set one.
///
/// Documented on `Video$bitrate`: "Bitrate for generated output, in bps. Default:
/// 2500000." — the "bitrate 2500 kbps" the `EncoderConfiguration$video` doc names.
pub const DEFAULT_VIDEO_BITRATE: i32 = 2_500_000;

/// Total pixels (`width` times `height`) a video configuration may reach.
///
/// Documented on both `Video$width` and `Video$height`: "Note that the maximum value is
/// determined by `width` times `height`, such that the maximum total pixels is 2073600
/// (1920x1080 or 1080x1920)." The model bounds each member on its own (`@range 2..1920`)
/// and carries no trait for their product, so the handler enforces it and answers the
/// modelled `ValidationException`.
pub const MAX_VIDEO_PIXELS: i64 = 2_073_600;

/// The video configuration an encoder configuration holds, given what its creating
/// request supplied: each member the request left out filled in from the model's
/// documented default for it.
///
/// Every member is reported, so a caller reading `video` back finds the resolution,
/// frame rate and bitrate the encoder will actually use rather than the absence of the
/// members it did not name.
pub fn effective_video(requested: Option<Video>) -> Video {
    let requested = requested.unwrap_or_default();
    Video {
        width: Some(requested.width.unwrap_or(DEFAULT_VIDEO_WIDTH)),
        height: Some(requested.height.unwrap_or(DEFAULT_VIDEO_HEIGHT)),
        framerate: Some(requested.framerate.unwrap_or(DEFAULT_VIDEO_FRAMERATE)),
        bitrate: Some(requested.bitrate.unwrap_or(DEFAULT_VIDEO_BITRATE)),
    }
}

/// Page size `ListEncoderConfigurations` uses when the request omits `maxResults`.
///
/// The model documents it on `ListEncoderConfigurationsRequest$maxResults`: "Maximum
/// number of results to return. Default: 100."
pub const DEFAULT_ENCODER_CONFIGURATION_PAGE_SIZE: usize = 100;

/// Storage configurations this emulator keeps for one engine. Not an AWS quota — this
/// emulator's own resource guard on the one store `CreateStorageConfiguration` grows,
/// set the same way [`MAX_STAGES`] is and answered with the same modelled
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_STORAGE_CONFIGURATIONS: usize = 1_000;

/// The ceiling the model puts on one `ListStorageConfigurations` page, from
/// `MaxStorageConfigurationResults`' `@range(1..100)`
/// (`smithy_query(query="shape",shape="MaxStorageConfigurationResults")`).
pub const MAX_STORAGE_CONFIGURATION_RESULTS: usize = 100;

/// Page size `ListStorageConfigurations` uses when the request omits `maxResults`.
///
/// The model documents it on `ListStorageConfigurationsRequest$maxResults`: "Default:
/// your service quota or 100, whichever is smaller." The quota on this store is
/// [`MAX_STORAGE_CONFIGURATIONS`] and the published 100 is the member's own modelled
/// maximum, so the default is computed from the two rather than written out — a listing
/// with a smaller quota than 100 pages by the quota, exactly as documented.
pub const DEFAULT_STORAGE_CONFIGURATION_PAGE_SIZE: usize =
    if MAX_STORAGE_CONFIGURATIONS < MAX_STORAGE_CONFIGURATION_RESULTS {
        MAX_STORAGE_CONFIGURATIONS
    } else {
        MAX_STORAGE_CONFIGURATION_RESULTS
    };

/// Public keys this emulator keeps for one engine. Not an AWS quota — this emulator's
/// own resource guard on the one store `ImportPublicKey` grows, set the same way
/// [`MAX_STAGES`] is and answered with the same modelled `ServiceQuotaExceededException`
/// (HTTP 402).
pub const MAX_PUBLIC_KEYS: usize = 1_000;

/// Stage sessions this emulator keeps per stage. Not an AWS quota — this emulator's
/// own resource guard on the store [`join_stage_session`] grows, set the same way
/// [`MAX_STAGES`] is. A session is not created by an API call (see
/// [`StageSessionRecord`]), so reaching it is not a request to refuse: the join is
/// simply not recorded.
pub const MAX_STAGE_SESSIONS_PER_STAGE: usize = 1_000;

/// Participants this emulator keeps per stage session, bounded the same way and for
/// the same reason as [`MAX_STAGE_SESSIONS_PER_STAGE`].
pub const MAX_PARTICIPANTS_PER_SESSION: usize = 1_000;

/// Occurrences this emulator keeps per stage session, bounded the same way and for the
/// same reason as [`MAX_STAGE_SESSIONS_PER_STAGE`]: an occurrence is not an API call
/// either (see [`EventRecord`]), so reaching the bound is not a request to refuse — the
/// occurrence is simply not recorded.
pub const MAX_EVENTS_PER_SESSION: usize = 10_000;

// ---------------------------------------------------------------------------
// What an ingest configuration reports beyond what the creating request said
//
// `CreateIngestConfiguration` takes the protocol, the stage and the publisher's
// identity; the three members an `IngestConfiguration` reports that no request
// supplies — its state, its stage when the request named none, and its redundant
// credentials — come from here, each from the model's own doc for that member. The
// state is the one of them that is not fixed: it follows the publishers the
// configuration's stage holds ([`ingest_configuration_publishers`]).
// ---------------------------------------------------------------------------

/// State of an ingest configuration nobody is publishing to.
///
/// One of the two values the modelled `IngestConfigurationState` carries
/// (`smithy_query(query="shape",shape="IngestConfigurationState")`), and the one a
/// configuration reports while no publisher is broadcasting with it. It is not a
/// transition the clock owes: what moves a configuration off it is a publisher
/// connecting, not time passing, so it does not change on its own.
pub const NO_PUBLISHER_INGEST_CONFIGURATION_STATE: &str = "INACTIVE";

/// State of an ingest configuration a publisher is broadcasting with right now.
///
/// The other value of the modelled `IngestConfigurationState`, and the one its member
/// doc names: `IngestConfiguration$state` "is `ACTIVE` if a publisher currently is
/// publishing to the stage associated with the ingest configuration". Which publishers
/// those are is [`ingest_configuration_publishers`].
pub const PUBLISHING_INGEST_CONFIGURATION_STATE: &str = "ACTIVE";

/// `stageArn` of an ingest configuration associated with no stage.
///
/// `IngestConfiguration$stageArn` is `@required` while
/// `CreateIngestConfigurationRequest$stageArn` is optional, and the
/// `IngestConfigurationStageArn` both target admits the empty string (its pattern's
/// first alternative, `^$`). Reporting the member as empty is the one reading under
/// which both hold: a caller that reads `stageArn` off a configuration always finds it
/// and compares it against a stage ARN, exactly as [`NO_ACTIVE_SESSION_ID`] works for a
/// stage's session.
pub const NO_ASSOCIATED_STAGE_ARN: &str = "";

// ---------------------------------------------------------------------------
// The members this service MATERIALISES when the creating request leaves them out
//
// Two optional members of a created resource are reported by the recorded responses as
// the empty string rather than left out: an encoder configuration's `name` and an ingest
// configuration's `userId`. Each targets a string whose `@length` admits the empty value
// (`EncoderConfigurationName` is `@length(0..128)`, `UserId` is `@length(0..128)` —
// `smithy_query(query="shape",shape="EncoderConfigurationName,UserId")`), so the empty
// string is a value the member can hold rather than a stand-in for absence.
//
// So each is materialised ONCE, by the operation that creates the resource, and stored on
// the record as a `String`. That is what makes every projection of that resource — the
// create response, the single-resource read, the update response and a listing's summary —
// report the member: none of them decides whether to carry it, because the record holds a
// value. It is the same reading [`NO_ACTIVE_SESSION_ID`] and [`NO_ASSOCIATED_STAGE_ARN`]
// already apply to the two members whose documentation states the empty default outright.
// ---------------------------------------------------------------------------

/// `name` of a resource whose creating request named none.
///
/// `EncoderConfiguration$name` is "optional name to identify the resource", and a
/// configuration created without one reports the member as empty rather than omitting it.
/// `PublicKey$name` ("Public key name", `@length(0..128)` through `PublicKeyName`) is read
/// the same way, so one constant stands for both.
pub const UNNAMED_RESOURCE_NAME: &str = "";

/// `userId` of an ingest configuration whose creating request supplied none.
///
/// `IngestConfiguration$userId` is the "customer-assigned name to help identify the
/// participant using the IngestConfiguration", and a configuration created without one
/// reports the member as empty rather than omitting it.
pub const UNNAMED_PARTICIPANT_USER_ID: &str = "";

/// Credentials an ingest configuration carries BESIDE its own `streamKey` when
/// `redundantIngest` is enabled.
///
/// `IngestConfiguration$redundantIngestCredentials` is "a list of redundant ingest
/// credentials, present only when `redundantIngest` is set to `true`", and the Redundant
/// Ingest section of *IVS RTMP Publishing* it points at describes redundant ingest as
/// publishing one source over a second, backup path. The configuration's own
/// `streamKey`/`participantId` are the primary path, so one further credential is what a
/// backup path takes. The model states no count, so the COUNT is this reading of that
/// doc; what the model does state is applied exactly — the member is absent unless
/// redundant ingest is on, and each entry carries its own participant id and its own
/// ingest key.
pub const REDUNDANT_INGEST_CREDENTIALS: usize = 1;

// ---------------------------------------------------------------------------
// Participant replication
//
// `StartParticipantReplication` replicates a PUBLISHING participant of one stage into
// another, and `ListParticipantReplicas` reports the replicas a participant has. The
// values neither request supplies — the replication state, the reconnect window a
// request leaves out, the capability that makes a participant a publisher — are fixed
// here, each from the model's own doc or value list.
// ---------------------------------------------------------------------------

/// The replication state of a replica that has been started and not stopped.
///
/// `ReplicationState`'s value list is `ACTIVE` | `STOPPED`
/// (`smithy_query(query="shape",shape="ReplicationState")`), and the model's
/// `StartParticipantReplicationRequest$reconnectWindowSeconds` doc names the running
/// state: "replication will continue to be `ACTIVE`".
pub const REPLICATION_STATE_ACTIVE: &str = "ACTIVE";

/// The replication state of a replica whose replication has been stopped.
///
/// The other value of that same list, and the one `StopParticipantReplication` leaves a
/// replication in: the list carries no intermediate value, so a stopped replication is
/// stopped from the moment the stop is accepted and a reader never sees it running again.
pub const REPLICATION_STATE_STOPPED: &str = "STOPPED";

/// The capability a participant token must grant for its participant to publish.
///
/// From `ParticipantTokenCapability`'s value list (`PUBLISH` | `SUBSCRIBE`), the same
/// value [`DEFAULT_TOKEN_CAPABILITIES`] grants by default. Replication is of a
/// *publishing* participant, so a participant that cannot publish has nothing to
/// replicate.
pub const PUBLISH_CAPABILITY: &str = "PUBLISH";

/// Reconnect window a replication gets when the request omits `reconnectWindowSeconds`.
///
/// The model documents it on `StartParticipantReplicationRequest$reconnectWindowSeconds`:
/// "If the participant disconnects and then reconnects within the specified interval,
/// replication will continue to be `ACTIVE`. Default: 0."
pub const DEFAULT_REPLICATION_RECONNECT_WINDOW_SECONDS: i32 = 0;

/// Participant replicas this emulator keeps per source stage. Not an AWS quota — this
/// emulator's own resource guard on the one store `StartParticipantReplication` grows,
/// set the same way [`MAX_STAGES`] is and answered with the same modelled
/// `ServiceQuotaExceededException` (HTTP 402). The records are reclaimed when either
/// stage of a replica is deleted.
pub const MAX_PARTICIPANT_REPLICAS_PER_STAGE: usize = 1_000;

// ---------------------------------------------------------------------------
// Compositions
//
// `StartComposition` composites a stage into one or two destinations, and
// `GetComposition` reports the composition back. The values no request supplies — the
// layout members a request leaves out, the state a started composition is in, and when
// the clock owes it the next one — are fixed here, each from the model's own doc or
// value list.
// ---------------------------------------------------------------------------

/// `CompositionState` a composition is in from the moment `StartComposition` returns
/// until the clock owes it [`COMPOSITION_ACTIVATION_SPAN`] later.
///
/// From `CompositionState`'s value list (`STARTING` | `ACTIVE` | `STOPPING` | `FAILED` |
/// `STOPPED`, `crate::ops::types::COMPOSITION_STATE_VALUES`): a composition that has just
/// been started is starting, not yet running.
pub const COMPOSITION_STATE_STARTING: &str = "STARTING";

/// `CompositionState` a started composition settles into, from the same value list.
pub const COMPOSITION_STATE_ACTIVE: &str = "ACTIVE";

/// `DestinationState` a destination of a starting composition is in, from
/// `DestinationState`'s value list (`crate::ops::types::DESTINATION_STATE_VALUES`).
pub const DESTINATION_STATE_STARTING: &str = "STARTING";

/// `DestinationState` a destination settles into with its composition.
pub const DESTINATION_STATE_ACTIVE: &str = "ACTIVE";

/// How long after `StartComposition` the clock owes a composition and its destinations
/// the `STARTING` -> `ACTIVE` transition.
///
/// AWS publishes no span for it, so this is the emulator's own, inside the band this
/// tree fixes for a transition the clock owes. It is deliberately NOT zero: a caller
/// that reads the composition back straight after starting it must still observe
/// `STARTING`, the way it would against AWS.
pub const COMPOSITION_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// Aspect ratio of a grid layout's non-featured tiles when the request sets none.
///
/// Documented on `GridConfiguration$videoAspectRatio`: "`VIDEO` is 16:9, `SQUARE` is
/// 1:1, and `PORTRAIT` is 3:4. Default: `VIDEO`."
pub const DEFAULT_GRID_VIDEO_ASPECT_RATIO: &str = "VIDEO";

/// Fill mode of a grid layout's tiles when the request sets none.
///
/// Documented on `GridConfiguration$videoFillMode`: "When not set, `videoFillMode`
/// defaults to `COVER` fill mode for participants in the grid" — the tiles the grid
/// lays out, which is what this member configures.
pub const DEFAULT_GRID_VIDEO_FILL_MODE: &str = "COVER";

/// Fill mode of a picture-in-picture layout's tiles when the request sets none.
///
/// Documented on `PipConfiguration$videoFillMode`: "Default: `COVER`."
pub const DEFAULT_PIP_VIDEO_FILL_MODE: &str = "COVER";

/// What a picture-in-picture window does when every participant has left, when the
/// request does not say.
///
/// Documented on `PipConfiguration$pipBehavior`: "`STATIC` (maintains original
/// position/size) or `DYNAMIC` (expands to full composition). Default: `STATIC`."
pub const DEFAULT_PIP_BEHAVIOR: &str = "STATIC";

/// Corner a picture-in-picture window sits in when the request does not say.
///
/// Documented on `PipConfiguration$pipPosition`: "Default: `BOTTOM_RIGHT`."
pub const DEFAULT_PIP_POSITION: &str = "BOTTOM_RIGHT";

/// Spacing between a grid layout's participant tiles, when the request does not size it.
///
/// Documented on `GridConfiguration$gridGap`: "Specifies the spacing between participant
/// tiles in pixels. Default: `2`." `PipConfiguration$gridGap` targets the same `GridGap`
/// shape and documents `0` instead, so the default belongs to the member rather than to
/// the shape, and the picture-in-picture layout keeps its own.
///
/// The model declares `@default 0` for the member, so the shape keeps the member's
/// PRESENCE (see the hand-written `GridConfiguration` in `crate::ops::types`) and this
/// default applies only where the request carried no value: a `0` the caller asks for —
/// the published minimum of the member's `@range` — is reported as it arrived.
pub const DEFAULT_GRID_GAP_PIXELS: i32 = 2;

/// Whether a grid omits participants with stopped video, when the request does not say.
///
/// Documented on `GridConfiguration$omitStoppedVideo`: "Determines whether to omit
/// participants with stopped video in the composition. Default: `false`" — the same
/// value the model declares as the member's `@default`.
pub const DEFAULT_OMIT_STOPPED_VIDEO: bool = false;

/// `featuredParticipantAttribute` of a layout that features no participant.
///
/// Documented on `GridConfiguration$featuredParticipantAttribute` and, in the same words,
/// on `PipConfiguration$featuredParticipantAttribute`: "Default: `\"\"` (no featured
/// participant)." Both layouts report the member from here, the way every other
/// empty-string documented default of this service is reported (see
/// [`NO_STORAGE_CONFIGURATION_ARN`]).
pub const NO_FEATURED_PARTICIPANT_ATTRIBUTE: &str = "";

/// `pipParticipantAttribute` of a picture-in-picture layout that names no PiP participant.
///
/// Documented on `PipConfiguration$pipParticipantAttribute`: "Default: `\"\"` (no PiP
/// participant)."
pub const NO_PIP_PARTICIPANT_ATTRIBUTE: &str = "";

/// Compositions this emulator keeps for one engine. Not an AWS quota — this emulator's
/// own resource guard on the one store `StartComposition` grows, set the same way
/// [`MAX_STAGES`] is and answered with the same modelled
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_COMPOSITIONS: usize = 1_000;

/// Page size `ListCompositions` uses when the request omits `maxResults`.
///
/// The model documents it on `ListCompositionsRequest$maxResults`: "Maximum number of
/// results to return. Default: 100."
pub const DEFAULT_COMPOSITION_PAGE_SIZE: usize = 100;

/// Characters in a minted composition id. The model's `CompositionArn` pattern allows
/// `[a-zA-Z0-9-]+` after `composition/`, and caps the whole ARN at 128 characters.
pub const COMPOSITION_ID_LEN: usize = 12;

/// Characters in a minted destination id. `Destination$id` is a plain `String`
/// ("Unique identifier for this destination, assigned by IVS"), so the id charset
/// [`mint_id`] produces satisfies it.
pub const DESTINATION_ID_LEN: usize = 12;

/// The composition ARN for `composition_id`, in the form the model's `CompositionArn`
/// pattern requires: `arn:aws:ivs:<region>:<account>:composition/<id>`.
pub fn composition_arn(region: &str, composition_id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("composition/{composition_id}"))
}

/// The grid layout `requested` describes, with the documented default for every member
/// it left out — each applied where the request carried no value, so a value the request
/// did carry is reported as it arrived.
///
/// `gridGap` is the member whose documented default (`2`) differs from the `@default`
/// (`0`) the model declares for it, which is why the shape keeps every member's
/// presence; `omitStoppedVideo` documents the value the model declares, so its default
/// is reached by the same route and reports the same thing either way.
///
/// `participantOrderAttribute` is the one member left absent when the request omits it:
/// its doc states no default, only that an unset and an empty attribute name order
/// participants the same way.
fn effective_grid(requested: GridConfiguration) -> GridConfiguration {
    GridConfiguration {
        featured_participant_attribute: Some(
            requested
                .featured_participant_attribute
                .unwrap_or_else(|| NO_FEATURED_PARTICIPANT_ATTRIBUTE.to_string()),
        ),
        omit_stopped_video: Some(documented_default(
            requested.omit_stopped_video,
            DEFAULT_OMIT_STOPPED_VIDEO,
        )),
        video_aspect_ratio: Some(
            requested
                .video_aspect_ratio
                .unwrap_or_else(|| DEFAULT_GRID_VIDEO_ASPECT_RATIO.to_string()),
        ),
        video_fill_mode: Some(
            requested
                .video_fill_mode
                .unwrap_or_else(|| DEFAULT_GRID_VIDEO_FILL_MODE.to_string()),
        ),
        grid_gap: Some(documented_default(
            requested.grid_gap,
            DEFAULT_GRID_GAP_PIXELS,
        )),
        participant_order_attribute: requested.participant_order_attribute,
    }
}

/// The picture-in-picture layout `requested` describes, with the documented default for
/// every member it left out.
///
/// `gridGap`, `pipOffset` and `omitStoppedVideo` each document the same default the model
/// declares as their `@default`, so the value the framework materialised for an absent
/// member is already the documented one and is reported as it arrived — which is why this
/// shape keeps the macro's members where `GridConfiguration` keeps its own presence.
///
/// `pipWidth` and `pipHeight` are deliberately left absent when the request omits them:
/// the model documents that each is then "based on the size of the composition and the
/// aspect ratio of the participant's video" rather than taking a fixed value, and
/// `participantOrderAttribute` is left absent because its doc states no default.
fn effective_pip(requested: PipConfiguration) -> PipConfiguration {
    PipConfiguration {
        featured_participant_attribute: Some(
            requested
                .featured_participant_attribute
                .unwrap_or_else(|| NO_FEATURED_PARTICIPANT_ATTRIBUTE.to_string()),
        ),
        pip_participant_attribute: Some(
            requested
                .pip_participant_attribute
                .unwrap_or_else(|| NO_PIP_PARTICIPANT_ATTRIBUTE.to_string()),
        ),
        video_fill_mode: Some(
            requested
                .video_fill_mode
                .unwrap_or_else(|| DEFAULT_PIP_VIDEO_FILL_MODE.to_string()),
        ),
        pip_behavior: Some(
            requested
                .pip_behavior
                .unwrap_or_else(|| DEFAULT_PIP_BEHAVIOR.to_string()),
        ),
        pip_position: Some(
            requested
                .pip_position
                .unwrap_or_else(|| DEFAULT_PIP_POSITION.to_string()),
        ),
        ..requested
    }
}

/// The layout a composition runs with, from what the request asked for.
///
/// `Composition$layout` is `@required`, so every composition reports one even when the
/// starting request named none: the model documents the fallback on
/// `LayoutConfiguration$grid` — "Default: Grid layout" — so a request that configures
/// neither layout gets a grid with every documented grid default.
pub fn effective_layout(requested: Option<LayoutConfiguration>) -> LayoutConfiguration {
    let requested = requested.unwrap_or_default();
    if let Some(pip) = requested.pip {
        return LayoutConfiguration {
            grid: None,
            pip: Some(effective_pip(pip)),
        };
    }
    LayoutConfiguration {
        grid: Some(effective_grid(requested.grid.unwrap_or_default())),
        pip: None,
    }
}

// ---------------------------------------------------------------------------
// The modelled defaults of a composition destination's configuration
//
// A destination configuration is stored and reported the way every other configuration
// this service stores is: each member the request left out of an object it DID supply is
// reported with the default that member's own doc states. The constants come from the
// member docs; none is this emulator's choice.
// ---------------------------------------------------------------------------

/// Target duration of a segment recorded by composite recording, when the recording
/// configuration of a composition destination does not set one.
///
/// Documented on `CompositionRecordingHlsConfiguration$targetSegmentDurationSeconds`:
/// "Defines the target duration for recorded segments generated when using composite
/// recording. Default: 2." Composite recording is not the participant recording
/// [`DEFAULT_HLS_TARGET_SEGMENT_DURATION_SECONDS`] cites, and the two members document
/// different values, so each has its own constant.
pub const DEFAULT_COMPOSITE_HLS_TARGET_SEGMENT_DURATION_SECONDS: i32 = 2;

/// The HLS configuration a composition destination's recording reports, with the one
/// member the request left out filled in from the model's documented default for it.
fn effective_composite_hls_configuration(
    requested: Option<CompositionRecordingHlsConfiguration>,
) -> CompositionRecordingHlsConfiguration {
    CompositionRecordingHlsConfiguration {
        target_segment_duration_seconds: Some(
            requested
                .and_then(|configuration| configuration.target_segment_duration_seconds)
                .unwrap_or(DEFAULT_COMPOSITE_HLS_TARGET_SEGMENT_DURATION_SECONDS),
        ),
    }
}

/// The recording configuration a composition destination reports, given the one its
/// starting request supplied.
///
/// `hlsConfiguration` is reported for every recording configuration that exists: the
/// member is there "to return information about how the recording will be configured",
/// and the recording is configured with the documented segment duration whether the
/// request named it or not.
///
/// `format` is reported as it arrived: `RecordingConfiguration$format` documents no
/// default, so an absent one is not filled in.
fn effective_composite_recording_configuration(
    requested: RecordingConfiguration,
) -> RecordingConfiguration {
    RecordingConfiguration {
        hls_configuration: Some(effective_composite_hls_configuration(
            requested.hls_configuration,
        )),
        format: requested.format,
    }
}

/// One thumbnail configuration of a composition destination, with each member the request
/// left out filled in from the model's documented default for it — the composition-side
/// counterpart of [`effective_thumbnail_configuration`], over the shape that carries no
/// `recordingMode`.
fn effective_composition_thumbnail_configuration(
    requested: CompositionThumbnailConfiguration,
) -> CompositionThumbnailConfiguration {
    CompositionThumbnailConfiguration {
        target_interval_seconds: Some(
            requested
                .target_interval_seconds
                .unwrap_or(DEFAULT_THUMBNAIL_INTERVAL_SECONDS),
        ),
        storage: Some(requested.storage.unwrap_or_else(default_thumbnail_storage)),
    }
}

/// The S3 destination a composition records to, from what the starting request asked for:
/// every member of every configuration object the request supplied, with the model's
/// documented default where the request left the member out.
///
/// The two optional objects are defaulted only where the request SUPPLIED them, which is
/// what the model gives their absence meaning for: `recordingConfiguration` is "an
/// optional customer specification", and `thumbnailConfigurations` is what "allows you to
/// enable/disable the recording of thumbnails for a `Composition`" — and this thumbnail
/// shape, unlike the stage-side one, carries no `recordingMode` member that could report
/// thumbnails as off, so an element conjured for a request that supplied none would
/// report a composition as generating thumbnails it never asked for. A documented default
/// fills a member left out of an object that exists; it does not create the object.
/// `storageConfigurationArn` and `encoderConfigurationArns` are `@required`, so neither
/// can be absent.
fn effective_s3_destination_configuration(
    requested: S3DestinationConfiguration,
) -> S3DestinationConfiguration {
    S3DestinationConfiguration {
        storage_configuration_arn: requested.storage_configuration_arn,
        encoder_configuration_arns: requested.encoder_configuration_arns,
        recording_configuration: requested
            .recording_configuration
            .map(effective_composite_recording_configuration),
        thumbnail_configurations: requested.thumbnail_configurations.map(|configurations| {
            configurations
                .into_iter()
                .map(effective_composition_thumbnail_configuration)
                .collect()
        }),
    }
}

/// The configuration one destination of a composition runs with: the ONE place a
/// destination configuration's documented defaults are applied, so what `StartComposition`
/// persists already carries them and every read of the destination reports the same thing.
///
/// A channel destination has nothing to fill in: `ChannelDestinationConfiguration$channelArn`
/// is `@required` and its `encoderConfigurationArn` documents no default, so it is
/// reported as it arrived — as is `DestinationConfiguration$name`.
pub fn effective_destination_configuration(
    requested: DestinationConfiguration,
) -> DestinationConfiguration {
    DestinationConfiguration {
        name: requested.name,
        channel: requested.channel,
        s3: requested.s3.map(effective_s3_destination_configuration),
    }
}

// Each constant below marks the page token of ONE listing, so a token from elsewhere is
// refused rather than silently read as a position. The marker a token actually carries is
// the constant plus the SCOPE the issuing request paged within ([`scope_marker`]), so two
// calls of the same listing over different records — another region, another stage,
// another session, another participant — do not share a token either.

/// Marks a `ListStages` page token as this service's, so a token from elsewhere is
/// refused rather than silently read as a position.
const PAGE_TOKEN_PREFIX: &str = "ivs-realtime-stage-after:";

/// Marks a `ListParticipants` page token, the same way [`PAGE_TOKEN_PREFIX`] marks a
/// `ListStages` one. The prefixes differ so a token issued by one listing names no
/// position in the other: the two page over different records, and a position in one
/// is meaningless in the other.
const PARTICIPANT_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-participant-after:";

/// The same marker for a `ListParticipantReplicas` page, kept distinct from
/// [`PAGE_TOKEN_PREFIX`] so a token issued by one listing names no position in the
/// other and is refused there.
const REPLICA_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-replica-after:";

/// The same marker for a `ListStorageConfigurations` page, kept distinct from the two
/// above so a token issued by one listing names no position in another and is refused
/// there.
const STORAGE_CONFIGURATION_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-storage-configuration-after:";

/// The same marker for a `ListStageSessions` page, kept distinct from the others so a
/// token issued by one listing names no position in another and is refused there.
const STAGE_SESSION_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-stage-session-after:";

/// The same marker for a `ListParticipantEvents` page, kept distinct from the others so
/// a token issued by one listing names no position in another and is refused there.
const PARTICIPANT_EVENT_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-participant-event-after:";

/// The same marker for a `ListCompositions` page, kept distinct from the others so a
/// token issued by one listing names no position in another and is refused there.
const COMPOSITION_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-composition-after:";

/// The same marker for a `ListEncoderConfigurations` page, kept distinct from the others
/// so a token issued by one listing names no position in another and is refused there.
const ENCODER_CONFIGURATION_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-encoder-configuration-after:";

/// The same marker for a `ListIngestConfigurations` page, kept distinct from the others
/// so a token issued by one listing names no position in another and is refused there.
const INGEST_CONFIGURATION_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-ingest-configuration-after:";

/// The same marker for a `ListPublicKeys` page, kept distinct from the others so a token
/// issued by one listing names no position in another and is refused there.
const PUBLIC_KEY_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-public-key-after:";

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

/// One ingest credential this service minted: the participant a publisher appears as in
/// the stage, and the ingest key it publishes with.
///
/// Reported both as an `IngestConfiguration`'s own `participantId`/`streamKey` and as one
/// entry of its `redundantIngestCredentials`, which the model gives the same two members.
#[derive(Debug, Clone)]
pub struct IngestCredential {
    pub participant_id: String,
    pub stream_key: String,
}

impl IngestCredential {
    /// The modelled `RedundantIngestCredential` this credential reports.
    pub fn to_redundant_ingest_credential(&self) -> RedundantIngestCredential {
        RedundantIngestCredential {
            participant_id: Some(self.participant_id.clone()),
            stream_key: Some(self.stream_key.clone()),
        }
    }
}

/// One ingest configuration, as `CreateIngestConfiguration` persisted it and
/// `GetIngestConfiguration` reports it.
#[derive(Debug, Clone)]
pub struct IngestConfigurationRecord {
    /// The ingest-configuration ARN, and the single key every lookup uses.
    pub arn: String,
    pub name: Option<String>,
    /// `RTMP` or `RTMPS`: the protocol the creating request chose, which the model
    /// makes `@required` on both the request and the reported configuration.
    pub ingest_protocol: String,
    /// The primary ingest key, minted once at creation ([`mint_stream_key`]) and
    /// reported unchanged on every read, so a publisher that cached it keeps working.
    pub stream_key: String,
    /// The stage this configuration is associated with, or [`NO_ASSOCIATED_STAGE_ARN`]
    /// when the creating request named none.
    pub stage_arn: String,
    /// The participant this configuration's publisher appears as in the stage.
    pub participant_id: String,
    /// The user id the creating request gave, or [`UNNAMED_PARTICIPANT_USER_ID`] when it
    /// gave none: materialised at creation, so it is a value this record HOLDS rather
    /// than one each projection decides, and every projection reports it.
    pub user_id: String,
    pub redundant_ingest: bool,
    /// The backup credentials redundant ingest takes, minted at creation beside the
    /// primary one. Empty unless [`Self::redundant_ingest`] is set; see
    /// [`REDUNDANT_INGEST_CREDENTIALS`].
    pub redundant_ingest_credentials: Vec<IngestCredential>,
    pub attributes: HashMap<String, String>,
    pub tags: HashMap<String, String>,
    /// Creation order, and the stable key a page over these records resumes from — the
    /// same role it plays on a [`StageRecord`].
    pub sequence: u64,
}

impl IngestConfigurationRecord {
    /// The modelled `IngestConfiguration` this record reports, read against `store`:
    /// the configuration's `state` is not a stored value but a fact about the stage it
    /// is associated with right now, so it is computed from the records that hold that
    /// fact on every read ([`Self::state`]).
    pub fn to_ingest_configuration(
        &self,
        store: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    ) -> IngestConfiguration {
        IngestConfiguration {
            name: self.name.clone(),
            arn: self.arn.clone(),
            ingest_protocol: self.ingest_protocol.clone(),
            stream_key: self.stream_key.clone(),
            stage_arn: self.stage_arn.clone(),
            participant_id: self.participant_id.clone(),
            state: self.state(store),
            // Always reported, the empty string included: the user id is materialised on
            // the record ([`Self::user_id`]), so a caller cannot read "no user id" as
            // "the member is not served".
            user_id: Some(self.user_id.clone()),
            redundant_ingest: self.redundant_ingest,
            // "present only when `redundantIngest` is set to `true`", as the member's
            // own doc states it: see [`REDUNDANT_INGEST_CREDENTIALS`].
            redundant_ingest_credentials: self.redundant_ingest.then(|| {
                self.redundant_ingest_credentials
                    .iter()
                    .map(IngestCredential::to_redundant_ingest_credential)
                    .collect()
            }),
            // Reported only when the configuration carries attributes, the same way a
            // participant token's are: the member is optional, and an absent member is
            // how the model says "none".
            attributes: (!self.attributes.is_empty()).then(|| self.attributes.clone()),
            // Always reported, empty map included: `tags` is how a caller reads the tags
            // back, exactly as on a stage.
            tags: Some(self.tags.clone()),
        }
    }

    /// The modelled `IngestConfigurationSummary` this record reports —
    /// `ListIngestConfigurations`' "summary information only".
    ///
    /// Every member it carries is the member of the same name on the full
    /// [`Self::to_ingest_configuration`], read off the same record at the same instant,
    /// so a listing and a read cannot disagree about one configuration. The summary shape
    /// carries neither the ingest keys (`streamKey`,
    /// `redundantIngestCredentials`) nor `attributes` nor `tags`
    /// (`smithy_query(query="shape",shape="IngestConfigurationSummary")`), which is why a
    /// page reports fewer members than a read rather than different ones.
    pub fn to_summary(
        &self,
        store: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    ) -> IngestConfigurationSummary {
        IngestConfigurationSummary {
            name: self.name.clone(),
            arn: self.arn.clone(),
            ingest_protocol: self.ingest_protocol.clone(),
            stage_arn: self.stage_arn.clone(),
            participant_id: self.participant_id.clone(),
            state: self.state(store),
            // The same member of the same record [`Self::to_ingest_configuration`]
            // reports, so a summary and a read answer alike.
            user_id: Some(self.user_id.clone()),
            redundant_ingest: self.redundant_ingest,
        }
    }

    /// The `state` this configuration reports right now, read against `store`.
    ///
    /// `ACTIVE` exactly while a publisher is broadcasting with this configuration, as
    /// the member's own doc states it; `INACTIVE` otherwise. Not a stored value but a
    /// fact about the stage the configuration is associated with at this instant, so it
    /// is computed from the records that hold that fact
    /// ([`ingest_configuration_publishers`]) on every read — and computed HERE, the one
    /// place, so a full read and a listing's summary cannot report two states for one
    /// configuration.
    pub fn state(
        &self,
        store: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    ) -> String {
        if ingest_configuration_publishers(store, self).is_empty() {
            NO_PUBLISHER_INGEST_CONFIGURATION_STATE.to_string()
        } else {
            PUBLISHING_INGEST_CONFIGURATION_STATE.to_string()
        }
    }

    /// Leave this configuration associated with NO stage, reporting
    /// [`NO_ASSOCIATED_STAGE_ARN`] for `stageArn`.
    ///
    /// The ONE writer of the "no stage" reading, so every operation that ends an
    /// association (a publisher's disconnect, the deletion of the stage itself) leaves
    /// the same value on the record — and `stageArn` never reports a stage this store
    /// no longer holds.
    pub fn clear_stage_association(&mut self) {
        self.stage_arn = NO_ASSOCIATED_STAGE_ARN.to_string();
    }

    /// Whether `participant_id` is one of the participants THIS configuration's ingest
    /// credentials were minted for: its own, or one of its redundant ones.
    ///
    /// The ONE test for "is this participant an identity of this configuration", so the
    /// lookup that resolves a participant of a stage ([`stage_participant`]) and the one
    /// that finds a configuration's publishers ([`ingest_configuration_publishers`])
    /// cannot drift apart.
    pub fn mints_participant(&self, participant_id: &str) -> bool {
        self.participant_id == participant_id
            || self
                .redundant_ingest_credentials
                .iter()
                .any(|credential| credential.participant_id == participant_id)
    }

    /// Whether the participant `record` broadcasts with THIS configuration: its join
    /// named this configuration, or it publishes as one of the participants this
    /// configuration's ingest credentials were minted for.
    ///
    /// The ONE predicate for "does this participant broadcast with this configuration",
    /// so the lookup that finds a configuration's publishers
    /// ([`ingest_configuration_publishers`]) and the one that finds a participant's
    /// configuration ([`participant_ingest_configuration`]) cannot drift apart.
    pub fn publishes_participant(&self, record: &ParticipantRecord) -> bool {
        record.ingest_configuration_arn.as_deref() == Some(self.arn.as_str())
            || self.mints_participant(&record.participant_id)
    }
}

/// One encoder configuration, as `CreateEncoderConfiguration` persisted it and
/// `GetEncoderConfiguration` reports it.
#[derive(Debug, Clone)]
pub struct EncoderConfigurationRecord {
    /// The encoder-configuration ARN, and the single key every lookup and delete uses.
    pub arn: String,
    /// The name the creating request gave, or [`UNNAMED_RESOURCE_NAME`] when it gave
    /// none: materialised at creation, so it is a value this record HOLDS rather than one
    /// each projection decides, and every projection reports it.
    pub name: String,
    /// The transcoding settings. Not optional: every encoder configuration has them,
    /// with the modelled defaults where the creating request supplied none
    /// ([`effective_video`]).
    pub video: Video,
    pub tags: HashMap<String, String>,
    /// Creation order, and the stable key `ListEncoderConfigurations` pages over — the
    /// same role [`StageRecord::sequence`] plays for `ListStages`.
    pub sequence: u64,
}

impl EncoderConfigurationRecord {
    /// The modelled `EncoderConfiguration` this record reports.
    pub fn to_encoder_configuration(&self) -> EncoderConfiguration {
        EncoderConfiguration {
            arn: self.arn.clone(),
            // Always reported, the empty string included: the name is materialised on the
            // record ([`Self::name`]), so a caller reading it back finds a value wherever
            // the configuration has one.
            name: Some(self.name.clone()),
            video: Some(self.video.clone()),
            // Always reported, empty map included: `tags` is how a caller reads the tags
            // back, exactly as on a stage.
            tags: Some(self.tags.clone()),
        }
    }

    /// The modelled `EncoderConfigurationSummary` a page of this record reports.
    ///
    /// `EncoderConfigurationSummary` carries three of the four members
    /// `EncoderConfiguration` does — `arn`, `name` and `tags`, but not `video`
    /// (`smithy_query(query="shape",shape="EncoderConfiguration,EncoderConfigurationSummary")`)
    /// — and each is projected off this one record, so a summary and a read answer alike
    /// on every member both carry.
    pub fn to_summary(&self) -> EncoderConfigurationSummary {
        EncoderConfigurationSummary {
            arn: self.arn.clone(),
            // The same member of the same record [`Self::to_encoder_configuration`]
            // reports, so a summary and a read answer alike.
            name: Some(self.name.clone()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// One storage configuration, as `CreateStorageConfiguration` persisted it and
/// `GetStorageConfiguration` / `ListStorageConfigurations` report it.
#[derive(Debug, Clone)]
pub struct StorageConfigurationRecord {
    /// The storage-configuration ARN, and the single key every lookup uses.
    pub arn: String,
    /// The name the creating request gave, which "does not need to be unique"
    /// (`CreateStorageConfigurationRequest$name`), or `None` when it named none.
    pub name: Option<String>,
    /// The S3 bucket recorded video is written to. `@required` on the creating
    /// request, so every record has one.
    pub bucket_name: String,
    pub tags: HashMap<String, String>,
    /// When the configuration was created. Reported by no member of the modelled
    /// `StorageConfiguration`, and recorded for the same reason
    /// [`StageRecord::created_at`] is.
    pub created_at: SystemTime,
    /// Creation order, and the stable key `ListStorageConfigurations` pages over — the
    /// same role [`StageRecord::sequence`] plays for `ListStages`.
    pub sequence: u64,
}

impl StorageConfigurationRecord {
    /// The modelled `StorageConfiguration` this record reports.
    pub fn to_storage_configuration(&self) -> StorageConfiguration {
        StorageConfiguration {
            arn: self.arn.clone(),
            name: self.name.clone(),
            s3: Some(self.to_s3()),
            // Always reported, empty map included: `tags` is how a caller reads the tags
            // back, exactly as on a stage.
            tags: Some(self.tags.clone()),
        }
    }

    /// The modelled `StorageConfigurationSummary` this record reports in a
    /// `ListStorageConfigurations` page.
    ///
    /// `StorageConfigurationSummary` carries the same four members as
    /// `StorageConfiguration` (`smithy_query(query="shape",
    /// shape="StorageConfiguration,StorageConfigurationSummary")`), so a summary and a
    /// read answer alike, member for member.
    pub fn to_summary(&self) -> StorageConfigurationSummary {
        StorageConfigurationSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            s3: Some(self.to_s3()),
            tags: Some(self.tags.clone()),
        }
    }

    /// The S3 destination both projections report, built once so they cannot differ.
    fn to_s3(&self) -> S3StorageConfiguration {
        S3StorageConfiguration {
            bucket_name: self.bucket_name.clone(),
        }
    }
}

/// One public key, as `ImportPublicKey` persisted it and `GetPublicKey` reports it.
///
/// A public key is the half of a customer-generated key pair this service holds, so the
/// stage participant tokens the customer signs with the private half can be verified:
/// nothing of the private half reaches this record.
#[derive(Debug, Clone)]
pub struct PublicKeyRecord {
    /// The public-key ARN, and the single key every lookup and delete uses.
    pub arn: String,
    /// The name the importing request gave, or [`UNNAMED_RESOURCE_NAME`] when it gave
    /// none: materialised at import, so it is a value this record HOLDS rather than one
    /// each projection decides, exactly as [`EncoderConfigurationRecord::name`] is.
    pub name: String,
    /// The PEM text the import carried, reported back verbatim by `PublicKey$publicKeyMaterial`
    /// ("Public key material").
    pub public_key_material: String,
    /// The key's fingerprint, computed from its DER `SubjectPublicKeyInfo` at import by
    /// `crate::ops::import_public_key`. Stored rather than recomputed per read, so every
    /// projection of this key answers the one value — and it identifies the key MATERIAL,
    /// which is what makes a re-import of the same key a conflict.
    pub fingerprint: String,
    pub tags: HashMap<String, String>,
    /// When the key was imported. Reported by no member of the modelled `PublicKey`, and
    /// recorded for the same reason [`StageRecord::created_at`] is.
    pub created_at: SystemTime,
    /// Import order, and the stable key `ListPublicKeys` pages over — the same role
    /// [`StageRecord::sequence`] plays for `ListStages`.
    pub sequence: u64,
}

impl PublicKeyRecord {
    /// The modelled `PublicKey` this record reports.
    pub fn to_public_key(&self) -> PublicKey {
        PublicKey {
            arn: Some(self.arn.clone()),
            // Always reported, the empty string included: the name is materialised on the
            // record ([`Self::name`]), so a caller reading it back finds a value wherever
            // the key has one.
            name: Some(self.name.clone()),
            public_key_material: Some(self.public_key_material.clone()),
            fingerprint: Some(self.fingerprint.clone()),
            // Always reported, empty map included: `tags` is how a caller reads the tags
            // back, exactly as on a stage.
            tags: Some(self.tags.clone()),
        }
    }

    /// The modelled `PublicKeySummary` this record reports in a `ListPublicKeys` page.
    ///
    /// `PublicKeySummary` carries three of the five members `PublicKey` does — `arn`,
    /// `name` and `tags`, and neither `publicKeyMaterial` nor `fingerprint`
    /// (`smithy_query(query="shape",shape="PublicKey,PublicKeySummary")`) — and each is
    /// projected off this one record, so a summary and a read answer alike on every
    /// member both carry.
    pub fn to_summary(&self) -> PublicKeySummary {
        PublicKeySummary {
            arn: Some(self.arn.clone()),
            // The same members of the same record [`Self::to_public_key`] reports, so a
            // summary and a read answer alike.
            name: Some(self.name.clone()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// One destination of a composition, as `StartComposition` persisted it.
#[derive(Debug, Clone)]
pub struct DestinationRecord {
    /// The id IVS assigns the destination (`Destination$id`), minted at start.
    pub id: String,
    /// One of `crate::ops::types::DESTINATION_STATE_VALUES`;
    /// [`DESTINATION_STATE_STARTING`] until the composition's
    /// [`CompositionRecord::active_at`].
    pub state: String,
    /// When the destination started, which is when its composition did.
    pub started_at: SystemTime,
    /// When it stopped; `None` while it is running.
    pub ended_at: Option<SystemTime>,
    /// What the starting request asked this destination to be —
    /// `Destination$configuration`, "configuration used to create this destination" —
    /// with the model's documented default for every member it left out
    /// ([`effective_destination_configuration`]).
    pub configuration: DestinationConfiguration,
    /// Where an S3 destination's recording lands; `None` for a channel destination,
    /// which records nothing. See [`recording_prefix`].
    pub detail: Option<DestinationDetail>,
}

impl DestinationRecord {
    /// Whether this destination refers to the resource `arn` of THIS endpoint's store.
    ///
    /// The members a destination configuration names such a resource with: the encoder
    /// configuration of a channel destination (`ChannelDestinationConfiguration$encoderConfigurationArn`),
    /// and the storage configuration and encoder configurations of an S3 one
    /// (`S3DestinationConfiguration$storageConfigurationArn` and `$encoderConfigurationArns`).
    /// `channelArn` is deliberately not one of them: it names a low-latency channel of
    /// the sibling endpoint, which this store does not hold — the same boundary
    /// `StartComposition` draws when it resolves these references.
    pub fn references(&self, arn: &str) -> bool {
        let channel_reference = self.configuration.channel.as_ref().is_some_and(|channel| {
            channel.encoder_configuration_arn.as_deref() == Some(arn)
        });
        let s3_reference = self.configuration.s3.as_ref().is_some_and(|s3| {
            s3.storage_configuration_arn == arn
                || s3
                    .encoder_configuration_arns
                    .iter()
                    .any(|encoder_configuration_arn| encoder_configuration_arn == arn)
        });
        channel_reference || s3_reference
    }

    /// Whether this destination has the ENCODER configuration `arn` attached: the
    /// encoder configuration of a channel destination
    /// (`ChannelDestinationConfiguration$encoderConfigurationArn`) or one of those of an
    /// S3 one (`S3DestinationConfiguration$encoderConfigurationArns`).
    ///
    /// Narrower than [`Self::references`], which answers "does this destination name that
    /// resource at all" for any resource of this store, a storage configuration included:
    /// `ListCompositions`' `filterByEncoderConfigurationArn` selects on the encoder
    /// configuration alone.
    pub fn references_encoder_configuration(&self, arn: &str) -> bool {
        let channel_reference = self
            .configuration
            .channel
            .as_ref()
            .is_some_and(|channel| channel.encoder_configuration_arn.as_deref() == Some(arn));
        let s3_reference = self.configuration.s3.as_ref().is_some_and(|s3| {
            s3.encoder_configuration_arns
                .iter()
                .any(|encoder_configuration_arn| encoder_configuration_arn == arn)
        });
        channel_reference || s3_reference
    }

    /// The modelled `Destination` this record reports.
    pub fn to_destination(&self) -> Destination {
        Destination {
            id: self.id.clone(),
            state: self.state.clone(),
            start_time: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            end_time: self.ended_at.map(::aws_smithy_types::DateTime::from),
            configuration: self.configuration.clone(),
            detail: self.detail.clone(),
        }
    }

    /// The modelled `DestinationSummary` a `ListCompositions` page reports this
    /// destination as.
    ///
    /// `DestinationSummary` carries the four members `Destination` carries besides the
    /// configuration and the detail
    /// (`smithy_query(query="shape",shape="Destination,DestinationSummary")`), each read
    /// off the same field [`Self::to_destination`] reports it from, so a summary and a
    /// read answer alike.
    pub fn to_summary(&self) -> DestinationSummary {
        DestinationSummary {
            id: self.id.clone(),
            state: self.state.clone(),
            start_time: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            end_time: self.ended_at.map(::aws_smithy_types::DateTime::from),
        }
    }
}

/// The S3 prefix an S3 destination's recording is stored under.
///
/// `S3Detail$recordingPrefix` is documented only as "The S3 bucket prefix under which
/// the recording is stored" and the model publishes no format for it, so the prefix is
/// DERIVED from the two ids that identify the recording — the composition and the
/// destination within it. That makes it unique per destination and stable across reads,
/// which is what a caller can rely on; it is not read back off any request.
pub fn recording_prefix(composition_id: &str, destination_id: &str) -> String {
    format!("{composition_id}/{destination_id}")
}

/// One composition, as `StartComposition` persisted it and `GetComposition` reports it.
#[derive(Debug, Clone)]
pub struct CompositionRecord {
    /// The composition ARN, and the single key every lookup uses.
    pub arn: String,
    /// The id the ARN ends in, which [`recording_prefix`] is built from.
    pub composition_id: String,
    /// The stage being composited.
    pub stage_arn: String,
    /// One of `crate::ops::types::COMPOSITION_STATE_VALUES`;
    /// [`COMPOSITION_STATE_STARTING`] until [`Self::active_at`].
    pub state: String,
    /// The layout the composition runs with: what the request asked for, with the
    /// model's documented default for everything it left out
    /// ([`effective_layout`]). Not optional — `Composition$layout` is `@required`.
    pub layout: LayoutConfiguration,
    /// The composition's destinations, in the order the request listed them.
    pub destinations: Vec<DestinationRecord>,
    pub tags: HashMap<String, String>,
    /// The `@idempotencyToken` the starting request carried, if any: a repeat of that
    /// same request answers with this composition instead of starting a second one.
    pub idempotency_token: Option<String>,
    /// What the starting request asked for, as one comparable value — see
    /// [`composition_fingerprint`]. A repeat under [`Self::idempotency_token`] is only a
    /// repeat when it asks for the same thing; a token reused for a different request is
    /// refused rather than answered with this composition.
    pub request_fingerprint: String,
    /// When the composition started (`Composition$startTime`).
    pub started_at: SystemTime,
    /// When it stopped (`Composition$endTime`); `None` while it is running.
    pub ended_at: Option<SystemTime>,
    /// The instant the clock owes this composition the `STARTING` -> `ACTIVE`
    /// transition: its start plus [`COMPOSITION_ACTIVATION_SPAN`]. Stored, never
    /// slept on.
    pub active_at: SystemTime,
    /// The instant the clock owes this composition the stop that ends it because the
    /// stage it composites has held no participant since one
    /// [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] before it — so that the end it begins is
    /// over, resource and all, when the published [`COMPOSITION_EMPTY_STAGE_TIMEOUT`] runs
    /// out; `None` while a participant is in that stage.
    ///
    /// Written only by [`Self::observe_stage_occupancy`], through the store-wide
    /// `ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::observe_stage_occupancy`
    /// that every change to a stage's participants goes through, so what the timeout is
    /// measured from is the instant the stage was actually left empty. Stored, never
    /// slept on.
    pub empty_stage_stop_at: Option<SystemTime>,
    /// Start order, and the stable key a listing of compositions pages over.
    pub sequence: u64,
}

impl CompositionRecord {
    /// Materialise the transition the clock has already made due.
    ///
    /// A starting composition — and every destination of it — becomes `ACTIVE`
    /// [`COMPOSITION_ACTIVATION_SPAN`] after the call that started it, and a stopping one
    /// becomes `STOPPED` [`COMPOSITION_STOP_SPAN`] after the call that stopped it,
    /// whether or not anybody calls again: this only writes down a change that was
    /// already owed at `now`, so a reporting operation may call it. A composition whose
    /// stage has held no participant for a whole [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] is
    /// stopped here too, by the clock rather than by a caller ([`Self::begin_stop`] at
    /// the instant that span ran out), and walks the same steps from there. The last
    /// step a
    /// stopped composition owes — the deletion of the ephemeral resource — is not a state
    /// to write but a record to drop, which is [`Self::is_reclaimed`]'s question and
    /// `ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::materialize_due_compositions`'s
    /// to act on.
    pub fn materialize_due(&mut self, now: SystemTime) {
        // The stop the clock owes a composition whose stage has been empty for a whole
        // [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] begins AT the instant it fell due, not at
        // this call: everything the stop owes afterwards is owed from there, so a caller
        // that reads late sees exactly what one reading on time saw.
        if self.is_running() {
            if let Some(due) = self.empty_stage_stop_at.filter(|due| now >= *due) {
                self.begin_stop(due);
            }
        }
        let due = self.state_at(now);
        if due == self.state {
            return;
        }
        let due = due.to_string();
        if due == COMPOSITION_STATE_ACTIVE {
            for destination in &mut self.destinations {
                if destination.state == DESTINATION_STATE_STARTING {
                    destination.state = DESTINATION_STATE_ACTIVE.to_string();
                }
            }
        } else if due == COMPOSITION_STATE_STOPPED {
            for destination in &mut self.destinations {
                if destination.state == DESTINATION_STATE_STOPPING {
                    destination.state = DESTINATION_STATE_STOPPED.to_string();
                }
            }
        }
        self.state = due;
    }

    /// The `CompositionState` this composition is in at `now`, whether or not anybody has
    /// called since the clock made it due.
    ///
    /// A composition's position in its lifecycle is a function of the instants the start
    /// and the end recorded — [`Self::active_at`] for `STARTING` -> `ACTIVE`, one
    /// [`COMPOSITION_STOP_SPAN`] after the instant its end began ([`Self::stop_began_at`]
    /// — a caller's stop, or the empty-stage timeout running out) for `STOPPING` ->
    /// `STOPPED` — so this is what every reader asks, and the stored `state` is only where
    /// [`Self::materialize_due`] writes the answer down. A reader holding the store
    /// immutably therefore reaches the same answer as one that can write, instead of
    /// seeing a transition that is due but not yet recorded. A state the clock owes
    /// nothing from is reported as stored.
    pub fn state_at(&self, now: SystemTime) -> &str {
        // A composition whose stage has been empty for a whole
        // [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] is stopped by the clock exactly as a
        // caller's stop stops it, so both run out of the one instant the stop began
        // ([`Self::stop_began_at`]) and reach the same two states from it.
        if self.is_running() {
            if let Some(began) = self.stop_began_at(now) {
                return if now >= began + COMPOSITION_STOP_SPAN {
                    COMPOSITION_STATE_STOPPED
                } else {
                    COMPOSITION_STATE_STOPPING
                };
            }
        }
        match self.state.as_str() {
            COMPOSITION_STATE_STARTING if now >= self.active_at => COMPOSITION_STATE_ACTIVE,
            COMPOSITION_STATE_STOPPING
                if self
                    .ended_at
                    .is_some_and(|ended_at| now >= ended_at + COMPOSITION_STOP_SPAN) =>
            {
                COMPOSITION_STATE_STOPPED
            }
            state => state,
        }
    }

    /// The modelled `Composition` this record reports.
    pub fn to_composition(&self) -> Composition {
        Composition {
            arn: self.arn.clone(),
            stage_arn: self.stage_arn.clone(),
            state: self.state.clone(),
            layout: self.layout.clone(),
            destinations: self
                .destinations
                .iter()
                .map(DestinationRecord::to_destination)
                .collect(),
            // Always reported, empty map included: `tags` is how a caller reads the
            // tags back, and an absent member is not the same answer as "none" — the
            // same rule [`StageRecord::to_stage`] follows.
            tags: Some(self.tags.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            end_time: self.ended_at.map(::aws_smithy_types::DateTime::from),
        }
    }

    /// Whether any destination of this composition has the encoder configuration `arn`
    /// attached — see [`DestinationRecord::references_encoder_configuration`].
    ///
    /// `ListCompositionsRequest$filterByEncoderConfigurationArn` filters the list "to
    /// match the specified EncoderConfiguration attached to at least one of its output",
    /// which is exactly this question.
    pub fn uses_encoder_configuration(&self, arn: &str) -> bool {
        self.destinations
            .iter()
            .any(|destination| destination.references_encoder_configuration(arn))
    }

    /// The modelled `CompositionSummary` this record reports in a `ListCompositions`
    /// page.
    ///
    /// `CompositionSummary` carries the same members as `Composition` except the layout
    /// and the per-destination configuration and detail
    /// (`smithy_query(query="shape",shape="Composition,CompositionSummary")`), so a
    /// summary and a read answer alike member for member on everything both carry.
    pub fn to_summary(&self) -> CompositionSummary {
        CompositionSummary {
            arn: self.arn.clone(),
            stage_arn: self.stage_arn.clone(),
            destinations: self
                .destinations
                .iter()
                .map(DestinationRecord::to_summary)
                .collect(),
            state: self.state.clone(),
            // Always reported, empty map included, the way [`Self::to_composition`]
            // reports it.
            tags: Some(self.tags.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            end_time: self.ended_at.map(::aws_smithy_types::DateTime::from),
        }
    }
}

// ---------------------------------------------------------------------------
// Stopping a composition
//
// `StopComposition` ends a composition `StartComposition` started, and so does the clock
// when the stage a composition composites is left with nobody in it. A composition is
// EPHEMERAL: the model's `StartComposition` doc says it "is an ephemeral resource that
// exists after this operation returns successfully" and that the "Composition stops and
// the resource is deleted … When StopComposition is called", "After a 1-minute timeout,
// when all participants are disconnected from the stage" and "After a 1-minute timeout,
// if there are no participants in the stage when StartComposition is called". So an end
// is the start of the composition's end, whichever of those began it — the clock owes it
// `STOPPED` one span later and the deletion of the resource one retention span after
// that, both
// materialised by [`CompositionRecord::materialize_due`] beside the `STARTING` ->
// `ACTIVE` transition a start owes, and neither ever slept on.
// ---------------------------------------------------------------------------

/// `CompositionState` a composition is in from the moment `StopComposition` returns until
/// the clock owes it [`COMPOSITION_STOP_SPAN`] later, from `CompositionState`'s value
/// list (`crate::ops::types::COMPOSITION_STATE_VALUES`).
pub const COMPOSITION_STATE_STOPPING: &str = "STOPPING";

/// `CompositionState` a stopping composition settles into, from the same value list.
/// Terminal: the ephemeral resource is reclaimed from this state and reaches no other.
pub const COMPOSITION_STATE_STOPPED: &str = "STOPPED";

/// `DestinationState` a destination of a stopping composition is in, from
/// `DestinationState`'s value list (`crate::ops::types::DESTINATION_STATE_VALUES`).
pub const DESTINATION_STATE_STOPPING: &str = "STOPPING";

/// `DestinationState` a destination settles into with the composition it belongs to.
pub const DESTINATION_STATE_STOPPED: &str = "STOPPED";

/// How long after `StopComposition` the clock owes a composition and its destinations
/// the `STOPPING` -> `STOPPED` transition. The deletion of the ephemeral resource that
/// follows is one [`COMPOSITION_RETENTION_SPAN`] later, not one of these.
///
/// AWS publishes no span for it, so this is the emulator's own, inside the band
/// this tree fixes for a transition the clock owes and set the same way
/// [`COMPOSITION_ACTIVATION_SPAN`] is. It is deliberately NOT zero: a caller that reads
/// the composition back straight after stopping it must still observe `STOPPING`, the way
/// it would against AWS.
pub const COMPOSITION_STOP_SPAN: Duration = Duration::from_millis(500);

/// How long a composition that has reached [`COMPOSITION_STATE_STOPPED`] goes on being
/// reported before the clock deletes the ephemeral resource.
///
/// The model says the "Composition stops and the resource is deleted" but publishes no
/// span between the two, and the stop is what a caller WAITS for: the documented way to
/// learn that a broadcast is over — and when it ended, which `Composition$endTime` is the
/// only report of — is to read the composition back until it answers with its terminal
/// state. A record that outlived its stop by less than the interval such a caller reads
/// at would never be seen in that state at all: the read before it would answer `STOPPING`
/// and the one after it the modelled `ResourceNotFoundException`, which is also the answer
/// for a composition that never existed. So the terminal state is kept for a span measured
/// in SECONDS, long enough that several reads of an ordinary poll loop land inside it,
/// after which the ephemeral resource is deleted rather than kept for ever.
pub const COMPOSITION_RETENTION_SPAN: Duration = Duration::from_secs(12);

/// How long after the stage it composites is left without participants a composition still
/// EXISTS — the span after which it has stopped and its ephemeral resource is gone.
///
/// PUBLISHED, so it keeps the value AWS states rather than a span of this tree's
/// choosing: `StartComposition` documents that a "Composition stops and the resource is
/// deleted … After a 1-minute timeout, when all participants are disconnected from the
/// stage. After a 1-minute timeout, if there are no participants in the stage when
/// StartComposition is called." Both clauses are the same timeout on the same condition —
/// the stage has nobody in it — so one span serves both, measured from the instant the
/// stage was left empty or, for a composition started against an empty stage, from the
/// start.
///
/// What the minute bounds is the whole end the doc states, the stop AND the deletion, so
/// the end BEGINS one [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] in rather than when the minute
/// runs out: a caller watching a composition nobody publishes to sees it reach `STOPPED`
/// with its end time while the minute is still running, and finds the resource gone once
/// it has run out.
pub const COMPOSITION_EMPTY_STAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a composition goes on composing while the stage it composites holds no
/// participant, before the clock owes it the stop that ends it.
///
/// The published [`COMPOSITION_EMPTY_STAGE_TIMEOUT`] is what a composition of an empty
/// stage has left, and an end takes time the doc gives no span for: `STOPPING` for one
/// [`COMPOSITION_STOP_SPAN`], then `STOPPED` for one [`COMPOSITION_RETENTION_SPAN`], and
/// only then the deletion the same sentence states. Begin that end when the minute runs
/// out and the minute would bound only its first step, leaving the terminal state readable
/// for a span no caller can observe; so it begins one whole end before the minute is up,
/// which is what this span is. The composition therefore stops inside the published
/// timeout and is deleted when it expires, both — as the doc has them — "after a 1-minute
/// timeout".
pub const COMPOSITION_EMPTY_STAGE_STOP_SPAN: Duration = COMPOSITION_EMPTY_STAGE_TIMEOUT
    .saturating_sub(COMPOSITION_RETENTION_SPAN)
    .saturating_sub(COMPOSITION_STOP_SPAN);

// The end has to FIT inside the published timeout for that derivation to mean anything: a
// subtraction that saturated would stop a composition the instant its stage fell empty,
// which is neither what the doc states nor a span a caller could observe the composing
// state in. Changing one of the three spans so it no longer fits is therefore a compile
// error rather than a silent change of behaviour.
const _: () = assert!(!COMPOSITION_EMPTY_STAGE_STOP_SPAN.is_zero());

impl CompositionRecord {
    /// Whether this composition is still composing, so a stop is the next thing that can
    /// happen to it.
    ///
    /// The two states a composition reaches on its way up ([`COMPOSITION_STATE_STARTING`]
    /// and [`COMPOSITION_STATE_ACTIVE`]); from any other one the broadcast is already
    /// ending or has ended, which is what makes a second stop the modelled conflict.
    ///
    /// Asked of the stored state, because the only transitions the clock owes are between
    /// two states this answers the same for ([`Self::state_at`] moves `STARTING` to
    /// `ACTIVE` and `STOPPING` to `STOPPED`): whether the due transition has been written
    /// down yet cannot change the answer.
    pub fn is_running(&self) -> bool {
        self.state == COMPOSITION_STATE_STARTING || self.state == COMPOSITION_STATE_ACTIVE
    }

    /// Whether this composition is still using the resources its destinations name, so a
    /// delete of one of them would leave it composing against a resource that is gone.
    ///
    /// Every state on this side of a FINISHED stop: the model tells a caller that wants
    /// to delete such a resource to "first use StopComposition and wait for it to
    /// complete", so a composition that is starting, composing or still stopping is using
    /// them, and one whose stop has completed is not.
    pub fn is_using_resources(&self) -> bool {
        self.is_running() || self.state == COMPOSITION_STATE_STOPPING
    }

    /// Whether any destination of this composition refers to the resource `arn` — see
    /// [`DestinationRecord::references`].
    pub fn references(&self, arn: &str) -> bool {
        self.destinations
            .iter()
            .any(|destination| destination.references(arn))
    }

    /// Begin the stop of this composition at `now`: the composition and every destination
    /// it broadcasts to enter their stopping state and record when the broadcast ended.
    ///
    /// The one place a composition's end begins, for both of the ways the model says it
    /// does: a caller's `StopComposition`, and the clock running out the
    /// [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] of a composition whose stage holds nobody
    /// ([`Self::materialize_due`], which passes the instant the timeout ran out).
    ///
    /// Everything after that is the clock's ([`Self::materialize_due`]): `STOPPED` one
    /// [`COMPOSITION_STOP_SPAN`] later, and the deletion of the ephemeral resource one
    /// [`COMPOSITION_RETENTION_SPAN`] after THAT.
    pub fn begin_stop(&mut self, now: SystemTime) {
        self.state = COMPOSITION_STATE_STOPPING.to_string();
        self.ended_at = Some(now);
        for destination in &mut self.destinations {
            destination.state = DESTINATION_STATE_STOPPING.to_string();
            destination.ended_at = Some(now);
        }
    }

    /// Whether the clock has reclaimed this composition's ephemeral resource by `now`, so
    /// nothing may report it any more.
    ///
    /// Due one [`COMPOSITION_RETENTION_SPAN`] after the stop FINISHED — which is that span
    /// plus one [`COMPOSITION_STOP_SPAN`] after the stop began — computed from the instant
    /// that end began ([`Self::stop_began_at`]) and [`Self::state_at`] rather than from the
    /// stored state, so a reader that holds the record immutably reaches the same answer as
    /// one that can write ([`Self::materialize_due`] is what removes it).
    ///
    /// Both ways a composition's end begins — a caller's `StopComposition` and the clock
    /// running out [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] on a stage holding nobody — are
    /// answered from that one instant, so a stop nobody called for is readable as
    /// `STOPPED`, with its end time, for exactly as long as one a caller asked for.
    pub fn is_reclaimed(&self, now: SystemTime) -> bool {
        self.stopped_at(now)
            .is_some_and(|stopped| now >= stopped + COMPOSITION_RETENTION_SPAN)
    }

    /// The instant this composition's stop FINISHED, as of `now`: one
    /// [`COMPOSITION_STOP_SPAN`] after the end began ([`Self::stop_began_at`]), and `None`
    /// until the clock has reached it — so this answers "is this composition in its
    /// terminal state, and since when".
    fn stopped_at(&self, now: SystemTime) -> Option<SystemTime> {
        if self.state_at(now) != COMPOSITION_STATE_STOPPED {
            return None;
        }
        self.stop_began_at(now)
            .map(|began| began + COMPOSITION_STOP_SPAN)
    }

    /// The instant this composition's broadcast ends, as of `now`: the instant a stop
    /// recorded ([`Self::begin_stop`]), or — for a composition no caller stopped — the
    /// empty-stage deadline once the clock has reached it.
    ///
    /// `None` while the composition is still composing and the clock owes it no stop, so
    /// this is the one question "has this composition's end begun, and when" that both
    /// ends of the lifecycle are answered from.
    fn stop_began_at(&self, now: SystemTime) -> Option<SystemTime> {
        self.ended_at
            .or_else(|| self.empty_stage_stop_at.filter(|due| now >= *due))
    }

    /// Record what the stage this composition composites holds at `now`: a participant in
    /// it keeps the composition composing, and a stage left without one puts the
    /// composition on the published [`COMPOSITION_EMPTY_STAGE_TIMEOUT`], one whole end
    /// ([`COMPOSITION_EMPTY_STAGE_STOP_SPAN`]) into which the clock owes it the stop that
    /// ends it.
    ///
    /// Idempotent while the stage stays empty — the deadline is measured from the instant
    /// the stage was LEFT empty, so observing the same emptiness again does not push it
    /// back — and it says nothing about a composition that is no longer composing: one
    /// already ending has the instant of its end recorded.
    pub fn observe_stage_occupancy(&mut self, occupied: bool, now: SystemTime) {
        if !self.is_running() {
            return;
        }
        if occupied {
            self.empty_stage_stop_at = None;
        } else if self.empty_stage_stop_at.is_none() {
            self.empty_stage_stop_at = Some(now + COMPOSITION_EMPTY_STAGE_STOP_SPAN);
        }
    }
}

// ---------------------------------------------------------------------------
// Stage sessions and the participants in them
//
// A participant reaches a stage through the stage's DATA plane — it joins over WebRTC
// with the participant token it was issued, or publishes to the WHIP/RTMP endpoints
// `CreateStage` advertises. No operation of this control plane makes a participant
// join: the sibling `ivs` emulator records the same thing about a channel going live
// (`is_broadcasting`), and this service already records it about a publisher
// ([`NO_PUBLISHER_INGEST_CONFIGURATION_STATE`]). So the records below are written by
// [`join_stage_session`] / [`leave_stage_session`] — the one entry point a stage data
// plane uses, and the one place the modelled defaults of a joining participant are
// applied — over the session [`ensure_active_session`] opens, and by the one control-plane
// call that does put a participant into a stage, a replication starting
// ([`record_replicated_participant`], which goes through that same entry point); and the
// reporting operations (`GetParticipant`, `ListParticipants`, `ListStageSessions` and
// `GetStageSession`) answer from exactly what those recorded, never from a value
// invented for a join that never happened.
// ---------------------------------------------------------------------------

/// `ParticipantState` of a participant that is in the stage right now.
///
/// One of the two values the modelled `ParticipantState` enum carries
/// (`smithy_query(query="shape",shape="ParticipantState")`), and the state a
/// participant is in from the moment it joins.
pub const PARTICIPANT_STATE_CONNECTED: &str = "CONNECTED";

/// `ParticipantState` of a participant that has left the stage, the other value that
/// enum carries. A participant that leaves is not forgotten: the model documents
/// `ParticipantSummary$firstJoinTime` as "when the participant FIRST joined the stage
/// session" and `published` as "whether the participant EVER published", so the
/// session keeps the record and moves its state.
pub const PARTICIPANT_STATE_DISCONNECTED: &str = "DISCONNECTED";

/// `ParticipantRecordingState` of a participant nothing is recording.
///
/// `DISABLED` is the value the modelled `ParticipantRecordingState` carries for
/// exactly this (and the only one of its values that is NOT a
/// `ParticipantRecordingFilterByRecordingState`, which lists the five states a
/// recording passes through). It is what a joining participant gets unless the
/// recorder reports otherwise: this emulator writes no participant media — it serves
/// no S3 data plane — so a state out of that five would name work nothing ran.
pub const NOT_RECORDED_PARTICIPANT_RECORDING_STATE: &str = "DISABLED";

/// The `ParticipantRecordingState` values a recording that has ENDED carries.
///
/// `ParticipantRecordingState` carries six values
/// (`smithy_query(query="shape",shape="ParticipantRecordingState")`):
/// [`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`], which is nothing recording the
/// participant at all, and the five states a recording passes through (the values of
/// `ParticipantRecordingFilterByRecordingState`). These two are the ones a recording
/// comes to REST in — it stopped, or it failed — and the three that remain (`STARTING`,
/// `ACTIVE`, `STOPPING`) are the ones media is still being written in.
pub const FINISHED_PARTICIPANT_RECORDING_STATES: [&str; 2] = ["STOPPED", "FAILED"];

/// Whether `recording_state` is a state of a recording that is RUNNING right now.
///
/// The one reading of "recording is active" for a participant: neither "nothing records
/// it" ([`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`]) nor a recording that has come to
/// rest ([`FINISHED_PARTICIPANT_RECORDING_STATES`]) is a running recording; every other
/// value of the enum is one.
pub fn participant_recording_running(recording_state: &str) -> bool {
    recording_state != NOT_RECORDED_PARTICIPANT_RECORDING_STATE
        && !FINISHED_PARTICIPANT_RECORDING_STATES.contains(&recording_state)
}

/// `ReplicationType` of a participant that is neither replicated to another stage nor
/// a replica from one.
///
/// The model documents it on `ParticipantSummary$replicationType`: "Indicates if the
/// participant has been replicated to another stage or is a replica from another
/// stage. Default: `NONE`." `StartParticipantReplication` is what moves a participant
/// off it, which [`session_participant_replications`] reads back.
pub const REPLICATION_TYPE_NONE: &str = "NONE";

/// `ReplicationType` of a participant that is being replicated INTO another stage: the
/// publishing participant a replication names, as the session it publishes in reports
/// it.
///
/// One of the three values the modelled `ReplicationType` carries
/// (`smithy_query(query="shape",shape="ReplicationType")`), and the "has been
/// replicated to another stage" half of what the member indicates.
pub const REPLICATION_TYPE_SOURCE: &str = "SOURCE";

/// `ReplicationType` of a participant that IS a replica from another stage: the record
/// the DESTINATION stage's session holds for a replicated publisher.
///
/// The third value of that enum, the "is a replica from another stage" half of the
/// member, and the only one whose `sourceStageArn` / `sourceSessionId` the model reports
/// ("if `replicationType` is `REPLICA`").
pub const REPLICATION_TYPE_REPLICA: &str = "REPLICA";

/// How one participant of one session takes part in a replication right now: the
/// `replicationType` it reports, that replication's own state, and — for a replica — the
/// stage and session it is replicated from.
///
/// Held by no [`ParticipantRecord`]: a replication is stored once, as the
/// [`ParticipantReplicaRecord`] that started it, and these four members are read off it
/// ([`session_participant_replications`]), so a participant cannot report a replication this store
/// no longer holds and two readers cannot disagree about one replication.
#[derive(Debug, Clone)]
pub struct ParticipantReplication {
    /// [`REPLICATION_TYPE_NONE`], [`REPLICATION_TYPE_SOURCE`] or
    /// [`REPLICATION_TYPE_REPLICA`].
    pub replication_type: String,
    /// The replication's own state ([`REPLICATION_STATE_ACTIVE`] / `STOPPED`), absent
    /// when no replication names this participant.
    pub replication_state: Option<String>,
    /// Where a replica is replicated from, both reported only "if `replicationType` is
    /// `REPLICA`", as the two members' docs state.
    pub source_stage_arn: Option<String>,
    pub source_session_id: Option<String>,
}

impl Default for ParticipantReplication {
    /// What a participant no replication names reports: the default the model documents
    /// on `replicationType` ("Default: `NONE`"), with the three members that describe a
    /// replication absent.
    fn default() -> Self {
        Self {
            replication_type: REPLICATION_TYPE_NONE.to_string(),
            replication_state: None,
            source_stage_arn: None,
            source_session_id: None,
        }
    }
}

/// `ParticipantProtocol` of a participant this store holds no broadcast protocol for.
///
/// One of the four values the modelled `ParticipantProtocol` enum carries
/// (`smithy_query(query="shape",shape="ParticipantProtocol")`), and the only one that is
/// not a protocol: `Participant$protocol` is the "type of ingest protocol that the
/// participant employs for broadcasting", so a participant that broadcasts nothing — and
/// one whose join named no protocol and used no ingest configuration — carries this.
pub const PARTICIPANT_PROTOCOL_UNKNOWN: &str = "UNKNOWN";

/// What `Participant$recordingS3BucketName` and `$recordingS3Prefix` report for a
/// participant that is not being recorded.
///
/// Both members document exactly this value: the bucket and prefix the participant is
/// being recorded to "if individual participant recording is enabled, or `\"\"` (empty
/// string), if recording is not enabled".
pub const NOT_RECORDED_PARTICIPANT_RECORDING_S3_LOCATION: &str = "";

/// One participant of one stage session, as the stage's data plane recorded it
/// joining ([`join_stage_session`]) and `ListParticipants` reports it.
#[derive(Debug, Clone)]
pub struct ParticipantRecord {
    /// The participant id it joined under: the one its participant token carries, or
    /// the one its ingest credential publishes as.
    pub participant_id: String,
    /// The customer-assigned name the token carried, when it carried one.
    pub user_id: Option<String>,
    /// The attributes the join carried: the ones its token was issued with, or — for a
    /// replica — the ones the replication merged for it. No member of
    /// `ParticipantSummary` reports them — `Participant$attributes` does, which
    /// `GetParticipant` reports — so they are recorded here where the join supplies
    /// them, and a later join of the same participant records its own
    /// ([`join_stage_session`]).
    pub attributes: HashMap<String, String>,
    /// [`PARTICIPANT_STATE_CONNECTED`] or [`PARTICIPANT_STATE_DISCONNECTED`].
    pub state: String,
    /// When this participant FIRST joined this session; unchanged by a later rejoin,
    /// as the member's own doc requires.
    pub first_join_time: SystemTime,
    /// Whether it EVER published to the session, so it stays set once set.
    pub published: bool,
    /// What the recorder reports for it; [`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`]
    /// when nothing records it.
    pub recording_state: String,
    /// Whether the publisher reached the stage over redundant ingest.
    pub redundant_ingest: bool,
    /// The ingest configuration the join NAMED, when it named one. `None` leaves
    /// [`participant_ingest_configuration`] to resolve the configuration whose ingest
    /// credentials were minted for this participant, which is what the reported
    /// `ingestConfigurationArn` ([`participant_ingest_configuration_arn`]) and `protocol`
    /// ([`participant_protocol`]) are both read from.
    pub ingest_configuration_arn: Option<String>,
    /// What the stage's data plane observed about the joining client. No member of
    /// `ParticipantSummary` reports any of it; `Participant`'s client members do, which
    /// is why it is recorded here where the join observes it.
    pub client: ParticipantClient,
    /// The broadcast protocol the data plane saw this participant publish with, when it
    /// saw one. `None` leaves [`participant_protocol`] to resolve it from the ingest
    /// configuration the participant publishes through, and finally to
    /// [`PARTICIPANT_PROTOCOL_UNKNOWN`].
    pub protocol: Option<String>,
    /// Join order within the session, and the stable key `ListParticipants` pages
    /// over — the same role [`StageRecord::sequence`] plays for `ListStages`.
    pub sequence: u64,
}

/// What a stage's data plane observed about the client one participant joined with.
///
/// The six members `Participant` carries about the joining client — its Internet
/// Service Provider, operating system, browser and SDK version — each of which the model
/// targets at `ParticipantClientAttribute` and none of which any request of this control
/// plane supplies. A member the data plane did not observe stays `None`, which is how the
/// model says "not known": the alternative would be a value nothing reported.
#[derive(Debug, Default, Clone)]
pub struct ParticipantClient {
    pub isp_name: Option<String>,
    pub os_name: Option<String>,
    pub os_version: Option<String>,
    pub browser_name: Option<String>,
    pub browser_version: Option<String>,
    pub sdk_version: Option<String>,
}

impl ParticipantClient {
    /// Take over every member `observed` names, keeping what an earlier connection
    /// observed for the ones it does not.
    ///
    /// The ONE rule for "this connection observed nothing about that", applied per member
    /// and shared with the rest of a rejoin ([`join_stage_session`]): an absent member
    /// means the data plane did not observe it on this connection, never that an earlier
    /// observation is withdrawn — exactly as an absent `ingestConfigurationArn` or
    /// recording state leaves the record's own value standing.
    pub fn observe(&mut self, observed: &ParticipantClient) {
        for (member, value) in [
            (&mut self.isp_name, &observed.isp_name),
            (&mut self.os_name, &observed.os_name),
            (&mut self.os_version, &observed.os_version),
            (&mut self.browser_name, &observed.browser_name),
            (&mut self.browser_version, &observed.browser_version),
            (&mut self.sdk_version, &observed.sdk_version),
        ] {
            if value.is_some() {
                *member = value.clone();
            }
        }
    }
}

/// Where one participant's individual recording is written: the S3 bucket and the prefix
/// under it, both empty for a participant that is not being recorded.
#[derive(Debug, Clone)]
pub struct ParticipantRecordingLocation {
    pub bucket_name: String,
    pub prefix: String,
}

impl ParticipantRecordingLocation {
    /// What both members report when nothing records the participant — see
    /// [`NOT_RECORDED_PARTICIPANT_RECORDING_S3_LOCATION`].
    pub fn not_recorded() -> Self {
        Self {
            bucket_name: NOT_RECORDED_PARTICIPANT_RECORDING_S3_LOCATION.to_string(),
            prefix: NOT_RECORDED_PARTICIPANT_RECORDING_S3_LOCATION.to_string(),
        }
    }
}

/// The S3 prefix one participant's individual recording is stored under.
///
/// `Participant$recordingS3Prefix` is documented only as the "S3 prefix of the S3 bucket
/// where the participant is being recorded" and the model publishes no format for it
/// beyond `@pattern ^[a-zA-Z0-9-]*$` (which admits no `/`), so the prefix is DERIVED from
/// the two ids that identify the recording — the session and the participant within it,
/// both minted by this service from [`mint_id`]'s id characters. That makes it unique per
/// participant and stable across reads, which is what a caller can rely on, and it keeps
/// a publisher that disconnects and reconnects on the same prefix within the session, as
/// the member's merge note describes; it is not read back off any request.
///
/// Every id character outside the member's own pattern is rendered as `-`, so the prefix
/// satisfies it whatever the ids carry: `ParticipantId` admits `_`, which
/// `recordingS3Prefix` does not, and the data plane names the participant that joined.
///
/// The companion of [`recording_prefix`], which does the same for a composition's S3
/// destination under its own member's pattern.
pub fn participant_recording_prefix(session_id: &str, participant_id: &str) -> String {
    format!("{session_id}-{participant_id}")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// Where the participant `record` of the session `session_id` of `stage_arn` is being
/// recorded, or [`ParticipantRecordingLocation::not_recorded`] when nothing records it.
///
/// Both members report their value only "if individual participant recording is
/// enabled", and the stage's `autoParticipantRecordingConfiguration` states every
/// condition of that, each on one of its members. Recording has somewhere to go:
/// `storageConfigurationArn` names the storage configuration the media is written to, and
/// [`NO_STORAGE_CONFIGURATION_ARN`] is that member's documented "no storage configuration
/// is specified". And the configuration does not EXCLUDE this participant:
/// `recordParticipantReplicas` is "an optional field to disable replica participant
/// recording. If this is set to `false` when a participant is a replica, replica
/// participants are not recorded" — so a participant this session holds as a
/// [`REPLICATION_TYPE_REPLICA`] ([`session_participant_is_replica`], the one answer the
/// reported `replicationType` is read from too) is recorded nowhere on a stage whose
/// configuration turns replica recording off, whatever storage it names.
///
/// Beside the stage's own configuration it is the RECORDER that reports whether this
/// participant is being recorded, which [`ParticipantRecord::recording_state`] holds:
/// [`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`] is the one value of
/// `ParticipantRecordingState` that is not a state of a running recording, so a
/// participant carrying it is recorded nowhere even on a stage that configures storage.
/// The two members answer the empty string exactly when one of those conditions fails.
///
/// The bucket is then the one that storage configuration holds
/// ([`StorageConfigurationRecord::bucket_name`]) and the prefix is
/// [`participant_recording_prefix`] — both derived from the stored records on every read,
/// the way an ingest configuration's `state` is, so a stage whose recording configuration
/// changes reports the change at once.
pub fn participant_recording_location(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    record: &ParticipantRecord,
) -> ParticipantRecordingLocation {
    if record.recording_state == NOT_RECORDED_PARTICIPANT_RECORDING_STATE {
        return ParticipantRecordingLocation::not_recorded();
    }
    let Some(configuration) = state
        .stages
        .get(stage_arn)
        .map(|stage| &stage.auto_participant_recording_configuration)
    else {
        return ParticipantRecordingLocation::not_recorded();
    };
    if !records_participant_replicas(configuration)
        && session_participant_is_replica(state, stage_arn, session_id, &record.participant_id)
    {
        return ParticipantRecordingLocation::not_recorded();
    }
    let storage = Some(configuration.storage_configuration_arn.as_str())
        .filter(|arn| *arn != NO_STORAGE_CONFIGURATION_ARN)
        .and_then(|arn| state.storage_configurations.get(arn));
    match storage {
        Some(storage) => ParticipantRecordingLocation {
            bucket_name: storage.bucket_name.clone(),
            prefix: participant_recording_prefix(session_id, &record.participant_id),
        },
        None => ParticipantRecordingLocation::not_recorded(),
    }
}

/// One participant of a stage whose individual recording is running right now: the
/// session it is in, the participant it is, and the recording state the recorder reports
/// for it.
#[derive(Debug, Clone)]
pub struct RecordingParticipant {
    pub session_id: String,
    pub participant_id: String,
    /// The `ParticipantRecordingState` the recorder reports — one of the values
    /// [`participant_recording_running`] admits.
    pub recording_state: String,
}

/// The participants of `stage_arn` whose individual recording is running right now, in
/// the order the sessions of the stage hold them.
///
/// The ONE lookup for "is a recording running in this stage", so what a read of a
/// participant reports about its recording and what an update refuses over cannot
/// disagree: a recording is running for a participant that is still
/// [`PARTICIPANT_STATE_CONNECTED`], whose recorder state is a running one
/// ([`participant_recording_running`]), and whose media has somewhere to go that the
/// stage's `autoParticipantRecordingConfiguration` does not exclude it from — which is
/// exactly when [`participant_recording_location`] reports a bucket for it: the
/// configuration names a storage configuration this store holds ("Individual participant
/// recording cannot be started unless a storage configuration is specified", as
/// `AutoParticipantRecordingConfiguration$storageConfigurationArn` states) and, for a
/// participant that is a replica, it has not turned replica recording off
/// ([`records_participant_replicas`]).
pub fn stage_recording_participants(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
) -> Vec<RecordingParticipant> {
    let mut recording = Vec::new();
    let Some(sessions) = state.stage_sessions.get(stage_arn) else {
        return recording;
    };
    for session in sessions {
        for participant in &session.participants {
            if participant.state != PARTICIPANT_STATE_CONNECTED
                || !participant_recording_running(&participant.recording_state)
            {
                continue;
            }
            let location =
                participant_recording_location(state, stage_arn, &session.session_id, participant);
            if location.bucket_name == NOT_RECORDED_PARTICIPANT_RECORDING_S3_LOCATION {
                continue;
            }
            recording.push(RecordingParticipant {
                session_id: session.session_id.clone(),
                participant_id: participant.participant_id.clone(),
                recording_state: participant.recording_state.clone(),
            });
        }
    }
    recording
}

/// The ingest configuration the participant `record` of `stage_arn` broadcasts with, or
/// `None` when it broadcasts with none.
///
/// The one its join named, and otherwise one of `stage_arn`'s configurations whose ingest
/// credentials were minted for that participant — the one predicate
/// [`IngestConfigurationRecord::publishes_participant`] states, which
/// [`ingest_configuration_publishers`] answers in the other direction, so what a read of
/// a participant says about its ingest and what a read of the configuration says about
/// its publishers cannot disagree.
pub fn participant_ingest_configuration<'a>(
    state: &'a ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    record: &ParticipantRecord,
) -> Option<&'a IngestConfigurationRecord> {
    if let Some(arn) = record.ingest_configuration_arn.as_deref() {
        if let Some(configuration) = state.ingest_configurations.get(arn) {
            return Some(configuration);
        }
    }
    // By creation order, not by however the store happens to hold the configurations: a
    // read of one participant must not report one configuration's protocol now and
    // another's on the next call.
    state
        .ingest_configurations
        .values()
        .filter(|configuration| {
            configuration.stage_arn == stage_arn && configuration.publishes_participant(record)
        })
        .min_by_key(|configuration| configuration.sequence)
}

/// The ARN `Participant$ingestConfigurationArn` reports for the participant `record` of
/// `stage_arn` — "the participant's ingest configuration" — or `None` when it broadcasts
/// with none.
///
/// Read off the configuration [`participant_ingest_configuration`] resolves, the one
/// answer to "which configuration does this participant broadcast with", rather than off
/// the join's own member: a publisher that reached the stage on a configuration's ingest
/// credential named no ARN when it joined, yet broadcasts with that configuration — which
/// is what the read in the other direction ([`ingest_configuration_publishers`]) reports
/// of it. So the ARN a participant reports and the protocol it reports
/// ([`participant_protocol`]) name ONE configuration, and neither contradicts what a read
/// of that configuration says about its publishers.
pub fn participant_ingest_configuration_arn(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    record: &ParticipantRecord,
) -> Option<String> {
    participant_ingest_configuration(state, stage_arn, record)
        .map(|configuration| configuration.arn.clone())
}

/// The `ParticipantProtocol` the participant `record` of `stage_arn` broadcasts with.
///
/// `Participant$protocol` is the "type of ingest protocol that the participant employs
/// for broadcasting", so it is the protocol of whatever the participant publishes
/// through: the one the data plane observed, else the `ingestProtocol` of the ingest
/// configuration it broadcasts with ([`participant_ingest_configuration`]) — `RTMP` or
/// `RTMPS`, the two values that member carries — and [`PARTICIPANT_PROTOCOL_UNKNOWN`]
/// when this store holds neither.
pub fn participant_protocol(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    record: &ParticipantRecord,
) -> String {
    if let Some(protocol) = record.protocol.as_deref() {
        return protocol.to_string();
    }
    participant_ingest_configuration(state, stage_arn, record)
        .map(|configuration| configuration.ingest_protocol.clone())
        .unwrap_or_else(|| PARTICIPANT_PROTOCOL_UNKNOWN.to_string())
}

impl ParticipantRecord {
    /// The modelled `ParticipantSummary` this record reports in a `ListParticipants`
    /// page.
    ///
    /// `replication` is what the replications this store holds say about this participant
    /// of this session ([`session_participant_replications`]): the four replication members are a
    /// fact about a replication, not about the join that recorded this participant, so
    /// they are reported from the replication itself on every read. `ingestConfigurationArn`
    /// is likewise a fact about the configurations `store` holds for `stage_arn`
    /// ([`participant_ingest_configuration_arn`]), computed on every read exactly as the
    /// full [`Self::to_participant`] computes it.
    pub fn to_summary(
        &self,
        store: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
        stage_arn: &str,
        replication: &ParticipantReplication,
    ) -> ParticipantSummary {
        ParticipantSummary {
            participant_id: Some(self.participant_id.clone()),
            // Reported only when the token named one: the member is optional, and an
            // absent member is how the model says "no user id", exactly as on a
            // participant token.
            user_id: self.user_id.clone(),
            state: Some(self.state.clone()),
            first_join_time: Some(::aws_smithy_types::DateTime::from(self.first_join_time)),
            published: self.published,
            recording_state: Some(self.recording_state.clone()),
            replication_type: Some(replication.replication_type.clone()),
            replication_state: replication.replication_state.clone(),
            source_stage_arn: replication.source_stage_arn.clone(),
            source_session_id: replication.source_session_id.clone(),
            redundant_ingest: self.redundant_ingest,
            ingest_configuration_arn: participant_ingest_configuration_arn(
                store, stage_arn, self,
            ),
        }
    }

    /// The modelled `Participant` this record reports — what `GetParticipant` answers,
    /// read against `store` for the session `session_id` of `stage_arn`.
    ///
    /// Every member `ParticipantSummary` also carries is reported exactly as
    /// [`Self::to_summary`] reports it, so a read of one participant and the page it
    /// appears in cannot disagree about it. The members only `Participant` carries come
    /// from the same record — the attributes the join supplied and what it observed about
    /// the client — except the two that are not stored values but facts about the stage
    /// right now: where the participant's media is being recorded
    /// ([`participant_recording_location`]), the ingest configuration it broadcasts with
    /// ([`participant_ingest_configuration_arn`]) and the protocol that configuration
    /// ingests ([`participant_protocol`]), each computed on every read — and the four
    /// replication members, read off the replications the store holds
    /// ([`session_participant_replications`]) exactly as a summary's are.
    pub fn to_participant(
        &self,
        store: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
        stage_arn: &str,
        session_id: &str,
    ) -> Participant {
        let recording = participant_recording_location(store, stage_arn, session_id, self);
        let replication = session_participant_replications(store, stage_arn, session_id)
            .remove(&self.participant_id)
            .unwrap_or_default();
        Participant {
            participant_id: Some(self.participant_id.clone()),
            // Reported only when the token named one, exactly as on a summary.
            user_id: self.user_id.clone(),
            state: Some(self.state.clone()),
            first_join_time: Some(::aws_smithy_types::DateTime::from(self.first_join_time)),
            // Reported only when the participant's identity carried attributes, the same
            // way a participant token's and an ingest configuration's are: the member is
            // optional, and an absent member is how the model says "none".
            attributes: (!self.attributes.is_empty()).then(|| self.attributes.clone()),
            published: self.published,
            isp_name: self.client.isp_name.clone(),
            os_name: self.client.os_name.clone(),
            os_version: self.client.os_version.clone(),
            browser_name: self.client.browser_name.clone(),
            browser_version: self.client.browser_version.clone(),
            sdk_version: self.client.sdk_version.clone(),
            // Always reported, the empty value included: both members document that
            // empty string as what a participant nothing records carries.
            recording_s3_bucket_name: Some(recording.bucket_name),
            recording_s3_prefix: Some(recording.prefix),
            recording_state: Some(self.recording_state.clone()),
            protocol: Some(participant_protocol(store, stage_arn, self)),
            replication_type: Some(replication.replication_type),
            replication_state: replication.replication_state,
            source_stage_arn: replication.source_stage_arn,
            source_session_id: replication.source_session_id,
            redundant_ingest: self.redundant_ingest,
            ingest_configuration_arn: participant_ingest_configuration_arn(
                store, stage_arn, self,
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// The occurrences of a stage session
//
// The model's `Event` doc fixes what one is: "An occurrence during a stage session." So
// an event is recorded where the occurrence happens and nowhere else — a participant
// joining or leaving the session ([`join_stage_session`] / [`leave_stage_session`]) and
// a publisher's replication starting (`StartParticipantReplication`) — and
// `ListParticipantEvents` reports exactly those records. The names of `EventName` that
// nothing here records are the ones nothing here does: this emulator serves no WebRTC
// subscribe, fails no ingest and exchanges no participant token, so a `SUBSCRIBE_*`, an
// error event or a `TOKEN_EXCHANGED` would name something that never happened.
// ---------------------------------------------------------------------------

/// `EventName` of a participant joining the session — one of the twelve values the
/// modelled `EventName` carries (`smithy_query(query="shape",shape="EventName")`), and
/// the occurrence [`join_stage_session`] records.
pub const EVENT_JOINED: &str = "JOINED";

/// `EventName` of a participant leaving the session, the counterpart of
/// [`EVENT_JOINED`] and what [`leave_stage_session`] records.
pub const EVENT_LEFT: &str = "LEFT";

/// `EventName` of a participant starting to publish to the session.
///
/// A join that publishes is a join AND a publish — `ParticipantSummary$published` is
/// "whether the participant ever published" — so the publisher's first media is its own
/// occurrence beside the join's.
pub const EVENT_PUBLISH_STARTED: &str = "PUBLISH_STARTED";

/// `EventName` of a publishing participant's media ending, the counterpart of
/// [`EVENT_PUBLISH_STARTED`]: a publisher that leaves the session stops publishing to
/// it, so the two names alternate for one participant
/// ([`StageSessionRecord::is_publishing`]).
pub const EVENT_PUBLISH_STOPPED: &str = "PUBLISH_STOPPED";

/// `EventName` of a publishing participant beginning to be replicated into another
/// stage, which is what `StartParticipantReplication` does. The two members the model
/// marks "applicable only if the event name is `REPLICATION_STARTED` or
/// `REPLICATION_STOPPED`" (`destinationStageArn`, `destinationSessionId`) are carried by
/// exactly this event.
pub const EVENT_REPLICATION_STARTED: &str = "REPLICATION_STARTED";

/// `EventName` of that replication ending, which is what `StopParticipantReplication`
/// does — the counterpart of [`EVENT_REPLICATION_STARTED`] and the second name the two
/// destination members apply to.
pub const EVENT_REPLICATION_STOPPED: &str = "REPLICATION_STOPPED";

/// One occurrence during one stage session, as the data-plane seam that observed it
/// recorded it ([`record_participant_event`]) and `ListParticipantEvents` reports it.
#[derive(Debug, Clone)]
pub struct EventRecord {
    /// One of the modelled `EventName` values — the `EVENT_*` constants above.
    pub name: String,
    /// The participant the occurrence happened to: "the participant who triggered the
    /// event", as `Event$participantId` states, and the id
    /// `ListParticipantEvents` selects an event by.
    pub participant_id: String,
    /// When it occurred.
    pub event_time: SystemTime,
    /// The other participant the occurrence involves — "for a subscribe event, this is
    /// the publisher. For a publish or join event, this is null", as
    /// `Event$remoteParticipantId` states.
    pub remote_participant_id: Option<String>,
    /// The modelled `EventErrorCode` of an error event; "if the event is not an error
    /// event, this field is null", as `Event$errorCode` states.
    pub error_code: Option<String>,
    /// Where the participant is replicated, carried only by a replication event.
    pub destination_stage_arn: Option<String>,
    /// The session of that destination stage, carried only by a replication event.
    pub destination_session_id: Option<String>,
    /// Whether [`Self::participant_id`] is a replicated participant.
    pub replica: bool,
    /// Occurrence order within the session, and the stable key `ListParticipantEvents`
    /// pages over — the same role [`ParticipantRecord::sequence`] plays for
    /// `ListParticipants`.
    pub sequence: u64,
}

impl EventRecord {
    /// The modelled `Event` this record reports in a `ListParticipantEvents` page.
    ///
    /// `previousToken` and `newToken` describe a `TOKEN_EXCHANGED` event ("Source
    /// participant token for `TOKEN_EXCHANGED` event" / "Participant token created
    /// during `TOKEN_EXCHANGED` event"), which no occurrence of this emulator is, so
    /// they stay absent rather than reporting a token exchange that did not happen.
    pub fn to_event(&self) -> Event {
        Event {
            name: Some(self.name.clone()),
            participant_id: Some(self.participant_id.clone()),
            event_time: Some(::aws_smithy_types::DateTime::from(self.event_time)),
            remote_participant_id: self.remote_participant_id.clone(),
            error_code: self.error_code.clone(),
            destination_stage_arn: self.destination_stage_arn.clone(),
            destination_session_id: self.destination_session_id.clone(),
            replica: self.replica,
            previous_token: None,
            new_token: None,
        }
    }
}

/// What a data-plane seam observed occurring: the modelled name of the occurrence, the
/// participant it happened to, and only those members that name applies to.
///
/// Everything else an [`EventRecord`] holds — when it occurred and where it sits in the
/// session's occurrence order — is [`record_participant_event`]'s own doing, so no two
/// callers can record an occurrence differently.
#[derive(Debug, Default, Clone)]
pub struct ParticipantEvent {
    pub name: String,
    pub participant_id: String,
    pub remote_participant_id: Option<String>,
    pub error_code: Option<String>,
    pub destination_stage_arn: Option<String>,
    pub destination_session_id: Option<String>,
    pub replica: bool,
}

impl ParticipantEvent {
    /// The occurrence `name` happening to `participant_id`, with every member that does
    /// not apply to that name left unset.
    pub fn new(name: &str, participant_id: &str) -> Self {
        Self {
            name: name.to_string(),
            participant_id: participant_id.to_string(),
            ..Default::default()
        }
    }
}

/// Record `event` as having occurred at `at` in the session `session_id` of `stage_arn`.
///
/// The ONE place an event record is created, so every seam that observes an occurrence
/// writes the same representation of it: the instant is the one it occurred at — which is
/// `at`, supplied by the seam that observed it rather than read off the wall clock here,
/// so an occurrence the clock owed at a deadline is stamped with that deadline and not
/// with the call that happens to write it down — and the position is the session's next
/// occurrence position, which is what keeps a page of events in occurrence order.
///
/// `at` is the instant the occurrence HAPPENED, which for one a caller performs is that
/// call and for one the clock owed is the instant it fell due — never the later call that
/// writes it down — so what a reader is told about when something happened does not depend
/// on when something else looked.
///
/// `false` — nothing recorded — when this store holds no such session, or when the
/// session is at [`MAX_EVENTS_PER_SESSION`] events.
pub fn record_participant_event(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    event: ParticipantEvent,
    at: SystemTime,
) -> bool {
    if state
        .stage_session(stage_arn, session_id)
        .is_none_or(|session| session.events.len() >= MAX_EVENTS_PER_SESSION)
    {
        return false;
    }
    let sequence = state.next_sequence();
    let Some(session) = session_mut(state, stage_arn, session_id) else {
        return false;
    };
    session.events.push(EventRecord {
        name: event.name,
        participant_id: event.participant_id,
        event_time: at,
        remote_participant_id: event.remote_participant_id,
        error_code: event.error_code,
        destination_stage_arn: event.destination_stage_arn,
        destination_session_id: event.destination_session_id,
        replica: event.replica,
        sequence,
    });
    true
}

/// One started replication of one publishing participant into one destination stage, as
/// `StartParticipantReplication` persisted it and `ListParticipantReplicas` reports it.
#[derive(Debug, Clone)]
pub struct ParticipantReplicaRecord {
    /// The stage the replicated participant publishes in.
    pub source_stage_arn: String,
    /// The session of [`Self::source_stage_arn`] the publisher is in.
    pub source_session_id: String,
    /// The stage the participant is replicated into.
    pub destination_stage_arn: String,
    /// The session of [`Self::destination_stage_arn`] the replica joined.
    pub destination_session_id: String,
    /// The publisher being replicated — a participant of the source stage.
    pub participant_id: String,
    /// [`REPLICATION_STATE_ACTIVE`] while the replication is running.
    pub replication_state: String,
    /// The window a disconnected publisher may reconnect in before replication ends,
    /// or [`DEFAULT_REPLICATION_RECONNECT_WINDOW_SECONDS`] when the request omitted it.
    /// It is the span the clock measures [`Self::reconnect_deadline`] with, which is what
    /// the caller asked replication to do.
    pub reconnect_window_seconds: i32,
    /// The instant the clock ends this replication at, because the publisher it
    /// replicates has left the session it publishes in and its reconnect window runs out
    /// then — `None` while that publisher is connected, which is every replication of a
    /// publisher that has not gone away.
    ///
    /// Written only by [`observe_replication_publisher`], the one place a publisher's
    /// connection is observed, and read only by [`materialize_due_replications`], the
    /// clock that acts on it.
    pub reconnect_deadline: Option<SystemTime>,
    /// The attributes the replica participant carries in the destination stage: the
    /// source participant's own, with the starting request's merged over them — the rule
    /// `StartParticipantReplicationRequest$attributes` states ("These attributes are
    /// merged with any attributes set for this participant when creating the token. If
    /// there is overlap in keys, the values in these attributes are replaced."). No
    /// member of `ParticipantReplica` reports them; the participant operations
    /// (`GetParticipant` / `ListParticipants`) are what report a replica participant's
    /// attributes.
    pub attributes: HashMap<String, String>,
    /// When replication started. Reported by no member of `ParticipantReplica`, and
    /// recorded for the same reason [`StageRecord::created_at`] is.
    pub started_at: SystemTime,
    /// Start order, and the stable key `ListParticipantReplicas` pages over — the same
    /// role [`StageRecord::sequence`] plays for `ListStages`.
    pub sequence: u64,
}

impl ParticipantReplicaRecord {
    /// Move this replication to its stopped state.
    ///
    /// The ONE place a replication stops, whichever way it ended — a caller's
    /// `StopParticipantReplication`, a stage it names going away
    /// ([`end_replications_of_stage`]), or its publisher staying away for its whole
    /// reconnect window ([`materialize_due_replications`]) — so every end leaves the same
    /// record: [`REPLICATION_STATE_STOPPED`], which is what every reader reports from here
    /// on, and no deadline, because a replication that has ended is owed no further end.
    pub fn stop(&mut self) {
        self.replication_state = REPLICATION_STATE_STOPPED.to_string();
        self.reconnect_deadline = None;
    }

    /// Whether this replication is still running ([`REPLICATION_STATE_ACTIVE`]).
    pub fn is_active(&self) -> bool {
        self.replication_state == REPLICATION_STATE_ACTIVE
    }

    /// The modelled `ParticipantReplica` this record reports. Every member of the shape
    /// is `@required`, so each one is reported from the record and none is optional.
    pub fn to_participant_replica(&self) -> ParticipantReplica {
        ParticipantReplica {
            source_stage_arn: self.source_stage_arn.clone(),
            participant_id: self.participant_id.clone(),
            source_session_id: self.source_session_id.clone(),
            destination_stage_arn: self.destination_stage_arn.clone(),
            destination_session_id: self.destination_session_id.clone(),
            replication_state: self.replication_state.clone(),
        }
    }
}

/// One stage session: the stretch of a stage's life its participants are grouped
/// into.
///
/// The model's `StageSession` doc fixes what one is: "A stage session begins when the
/// first participant joins a stage and ends after the last participant leaves the
/// stage." So a session is not created or deleted by an API call — [`join_stage_session`]
/// opens one when a stage has none, and [`leave_stage_session`] closes it when the last
/// participant has gone.
#[derive(Debug, Clone)]
pub struct StageSessionRecord {
    /// The modelled `StageSessionId` ([`stage_session_id`]), and the key
    /// `ListParticipants` resolves the session by.
    pub session_id: String,
    /// When the first participant joined.
    pub start_time: SystemTime,
    /// When the last participant left, or `None` while the session is active — "This
    /// is null if the stage is active", as `StageSession$endTime` states.
    pub end_time: Option<SystemTime>,
    /// Every participant that joined this session, in join order, the ones that have
    /// left included.
    pub participants: Vec<ParticipantRecord>,
    /// Every occurrence during this session, in occurrence order — what
    /// `ListParticipantEvents` reports. Written only by
    /// [`record_participant_event`].
    pub events: Vec<EventRecord>,
    /// Start order, the key a page over the sessions of one stage resumes from.
    pub sequence: u64,
}

impl StageSessionRecord {
    /// The modelled `StageSessionSummary` this record reports in a `ListStageSessions`
    /// page.
    ///
    /// `endTime` is reported only once the session has ended — "This is null if the
    /// stage is active", as `StageSessionSummary$endTime` states — which is exactly
    /// what [`Self::end_time`] holds.
    pub fn to_summary(&self) -> StageSessionSummary {
        StageSessionSummary {
            session_id: Some(self.session_id.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
        }
    }

    /// The modelled `StageSession` this record reports when `GetStageSession` names it.
    ///
    /// Every member of `StageSession` is the same member of `StageSessionSummary` with
    /// the same doc, so the two are reported from one place and `GetStageSession` and
    /// `ListStageSessions` cannot disagree about a session: `endTime` is reported only
    /// once the session has ended ("This is null if the stage is active", as
    /// `StageSession$endTime` states).
    pub fn to_stage_session(&self) -> StageSession {
        let summary = self.to_summary();
        StageSession {
            session_id: summary.session_id,
            start_time: summary.start_time,
            end_time: summary.end_time,
        }
    }

    /// This session's participants in join order — the order `ListParticipants` pages
    /// over, so a record added or left mid-pagination cannot shift a later page onto
    /// an already-returned one.
    pub fn participants_in_join_order(&self) -> Vec<&ParticipantRecord> {
        let mut participants: Vec<&ParticipantRecord> = self.participants.iter().collect();
        participants.sort_by_key(|record| record.sequence);
        participants
    }

    /// This session's occurrences in the order they happened — the order
    /// `ListParticipantEvents` pages over, so an occurrence recorded mid-pagination
    /// cannot shift a later page onto an already-returned one.
    ///
    /// The stored order IS that order: the one writer
    /// ([`record_participant_event`]) only ever appends, under a strictly increasing
    /// position, so no ordering copy is needed.
    pub fn events_in_occurrence_order(&self) -> &[EventRecord] {
        &self.events
    }

    /// Whether `participant_id` is publishing to this session right now.
    ///
    /// Read off the occurrences themselves: [`EVENT_PUBLISH_STARTED`] and
    /// [`EVENT_PUBLISH_STOPPED`] are what begin and end one participant's media, so the
    /// later of the two is its publishing state and the two alternate rather than
    /// repeating. [`ParticipantRecord::published`] cannot answer this — it is "whether
    /// the participant EVER published", which stays set once set.
    pub fn is_publishing(&self, participant_id: &str) -> bool {
        // Over `self.events` directly, and from the back: the one writer
        // ([`record_participant_event`]) only ever appends, under an increasing
        // position, so the last matching record is the latest one and no ordering copy
        // is needed to find it.
        self.events
            .iter()
            .rfind(|event| {
                event.participant_id == participant_id
                    && (event.name == EVENT_PUBLISH_STARTED || event.name == EVENT_PUBLISH_STOPPED)
            })
            .is_some_and(|event| event.name == EVENT_PUBLISH_STARTED)
    }
}

/// What a stage's data plane observes about one participant joining a stage: the
/// identity its token or ingest credential carries, and what it does in the stage.
///
/// Everything else a [`ParticipantRecord`] holds is the join's own doing and is
/// applied by [`join_stage_session`], so no two callers can record a join
/// differently.
#[derive(Debug, Default, Clone)]
pub struct JoinedParticipant {
    /// The id the joining client's participant token carries, or the one its ingest
    /// credential publishes as. Both are minted by this service
    /// ([`issue_token_record`], [`mint_ingest_credential`]).
    pub participant_id: String,
    pub user_id: Option<String>,
    pub attributes: HashMap<String, String>,
    /// Whether this participant is publishing media, rather than only subscribing.
    pub published: bool,
    /// What the recorder reports for this participant, when something records it.
    /// `None` is [`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`].
    pub recording_state: Option<String>,
    /// Whether the publisher reached the stage over redundant ingest.
    pub redundant_ingest: bool,
    /// The ingest configuration it broadcast with, when it used one.
    pub ingest_configuration_arn: Option<String>,
    /// Whether this participant IS a replica of a participant publishing in another
    /// stage, which is what `Event$replica` reports of the occurrences it is party to
    /// ("If true, this indicates the `participantId` is a replicated participant").
    /// What the participant itself reports about that replication is read off the
    /// replication ([`session_participant_replications`]), not from here.
    pub replica: bool,
    /// What the data plane observed about the joining client; see
    /// [`ParticipantClient`]. Nothing observed is the default.
    pub client: ParticipantClient,
    /// The broadcast protocol it was seen publishing with, when the data plane saw one
    /// — one of the values the modelled `ParticipantProtocol` carries. `None` leaves
    /// [`participant_protocol`] to resolve it.
    pub protocol: Option<String>,
}

/// Record the participant `joined` describes joining `stage_arn`, opening a session
/// for the stage when no session is active, and return the record the session now
/// holds for it.
///
/// The ONE place a participant record is created, so every operation that reports one
/// answers the same representation: a joining participant is
/// [`PARTICIPANT_STATE_CONNECTED`], its `firstJoinTime` is the instant it joined, and it
/// is recorded with the recording state the recorder reports (or
/// [`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`]). What it reports about replication is
/// not recorded here: it is read off the replications this store holds
/// ([`session_participant_replications`]).
///
/// A participant the session already holds REJOINS it: the model documents
/// `firstJoinTime` as the first join and `published` as whether the participant ever
/// published, so the record keeps its id, its first join instant and its page
/// position, comes back to `CONNECTED`, and keeps `published` once set. Everything else
/// the join supplies is what the record reports from this join on — including the
/// identity it carries, its user id and its attributes — because a record reports the
/// connection it holds, not the one before it.
///
/// The session itself is opened by [`ensure_active_session`], the one place a session
/// of this service begins, so a participant that joins and a replica that is
/// replicated in are in the SAME session of the stage.
///
/// The join is also an occurrence of the session ([`EVENT_JOINED`]), and a join that
/// publishes is a publish occurrence beside it ([`EVENT_PUBLISH_STARTED`]), so
/// `ListParticipantEvents` reports what this recorded and nothing else.
///
/// `None` — nothing recorded — when this store holds no such stage, when the stage is
/// at [`MAX_STAGE_SESSIONS_PER_STAGE`] sessions, or when the session is at
/// [`MAX_PARTICIPANTS_PER_SESSION`] participants.
pub fn join_stage_session(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    joined: JoinedParticipant,
) -> Option<ParticipantRecord> {
    // One instant for the whole join, so the record, its occurrences and what the
    // replication and composition clocks are told all agree about when it happened.
    let now = SystemTime::now();
    // The session the stage is in, or a fresh one: "a stage session begins when the
    // first participant joins a stage".
    let session_id = ensure_active_session(state, stage_arn, now)?;
    // Read off before the join is applied, because applying it consumes `joined`.
    let participant_id = joined.participant_id.clone();
    let publishes = joined.published;
    let replica = joined.replica;

    // A participant the session already holds is REJOINING it, so it keeps its id, its
    // first join instant and its page position.
    let rejoined = {
        let session = session_mut(state, stage_arn, &session_id)?;
        if let Some(rejoined) = session
            .participants
            .iter_mut()
            .find(|participant| participant.participant_id == joined.participant_id)
        {
            rejoined.state = PARTICIPANT_STATE_CONNECTED.to_string();
            rejoined.published = rejoined.published || joined.published;
            // The identity this join carries is what the record reports from now on:
            // `attributes` is a member every join supplies, so the map it brings is the
            // participant's whole set of attributes — an empty one means this join
            // carries none, never that an earlier join's are kept — and a `user_id` the
            // join names replaces the earlier one under the optional-member rule the
            // rest of this block follows.
            rejoined.attributes = joined.attributes.clone();
            if joined.user_id.is_some() {
                rejoined.user_id = joined.user_id.clone();
            }
            if let Some(recording_state) = joined.recording_state.clone() {
                rejoined.recording_state = recording_state;
            }
            rejoined.redundant_ingest = joined.redundant_ingest;
            if joined.ingest_configuration_arn.is_some() {
                rejoined.ingest_configuration_arn = joined.ingest_configuration_arn.clone();
            }
            // A rejoin is a fresh connection, so what the data plane observes about it
            // — the client it came from and the protocol it publishes with — is what
            // the participant carries from now on. A member this connection did not
            // observe leaves the earlier observation standing, the rule every other
            // optional member of this block follows
            // ([`ParticipantClient::observe`]).
            rejoined.client.observe(&joined.client);
            if joined.protocol.is_some() {
                rejoined.protocol = joined.protocol.clone();
            }
            Some(rejoined.clone())
        } else {
            // Bound the store this join grows: the records are reclaimed only with the
            // stage.
            if session.participants.len() >= MAX_PARTICIPANTS_PER_SESSION {
                return None;
            }
            None
        }
    };

    let record = match rejoined {
        Some(record) => record,
        None => new_participant_record(state, stage_arn, &session_id, joined, now)?,
    };

    // Both a first join and a rejoin are a participant joining the session, so each is
    // recorded; a publish is recorded only where the media actually begins, which a
    // rejoin of a participant already publishing is not.
    //
    // Every occurrence of a join says whether the participant it happened to is a replica
    // of a publisher in another stage, which is what `Event$replica` reports.
    let occurrence = |name: &str| {
        let mut event = ParticipantEvent::new(name, &participant_id);
        event.replica = replica;
        event
    };
    record_participant_event(state, stage_arn, &session_id, occurrence(EVENT_JOINED), now);
    if publishes
        && !state
            .stage_session(stage_arn, &session_id)
            .is_some_and(|session| session.is_publishing(&participant_id))
    {
        record_participant_event(
            state,
            stage_arn,
            &session_id,
            occurrence(EVENT_PUBLISH_STARTED),
            now,
        );
    }
    // A publisher that is back is publishing again, so every replication it sources is
    // owed no end: "if the participant disconnects and then reconnects within the
    // specified interval, replication will continue to be `ACTIVE`", through the one place
    // a publisher's connection is observed.
    observe_replication_publisher(state, stage_arn, &session_id, &participant_id, now);
    // The stage now has somebody in it, so no composition of it is counting out the
    // empty-stage timeout the model's `StartComposition` doc ends a composition on.
    state.observe_stage_occupancy(stage_arn, now);
    Some(record)
}

/// Put the participant `joined` describes into the session `session_id` of `stage_arn`
/// as one NEW to it, and return the record the session now holds.
///
/// Split out of [`join_stage_session`] so the join's two paths — a rejoin and a first
/// join — meet again at one place that records the occurrence, rather than one of them
/// returning early.
fn new_participant_record(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    joined: JoinedParticipant,
    at: SystemTime,
) -> Option<ParticipantRecord> {
    // Only a participant that is NEW to the session takes a page position, so the join
    // order of a session's participants stays contiguous.
    let sequence = state.next_sequence();
    let record = ParticipantRecord {
        participant_id: joined.participant_id,
        user_id: joined.user_id,
        attributes: joined.attributes,
        state: PARTICIPANT_STATE_CONNECTED.to_string(),
        first_join_time: at,
        published: joined.published,
        recording_state: joined
            .recording_state
            .unwrap_or_else(|| NOT_RECORDED_PARTICIPANT_RECORDING_STATE.to_string()),
        redundant_ingest: joined.redundant_ingest,
        ingest_configuration_arn: joined.ingest_configuration_arn,
        client: joined.client,
        protocol: joined.protocol,
        sequence,
    };
    session_mut(state, stage_arn, session_id)?
        .participants
        .push(record.clone());
    Some(record)
}

/// The session `session_id` of the stage `stage_arn`, mutably: the one lookup
/// [`join_stage_session`] and [`leave_stage_session`] resolve a session to write
/// through, keyed exactly as the reading
/// [`ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::stage_session`]
/// is.
fn session_mut<'a>(
    state: &'a mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
) -> Option<&'a mut StageSessionRecord> {
    state
        .stage_sessions
        .get_mut(stage_arn)?
        .iter_mut()
        .find(|session| session.session_id == session_id)
}

/// Record the participant `participant_id` leaving the session `session_id` of
/// `stage_arn` NOW, and close that session when it was the last one in it.
///
/// The counterpart of [`join_stage_session`], and the one place a participant leaves:
/// the record stays in the session and moves to
/// [`PARTICIPANT_STATE_DISCONNECTED`] — `ListParticipants` reports the participants
/// of a session, which the ones that have left are — and when every participant has
/// left, the session "ends after the last participant leaves the stage": its
/// `endTime` is set and the stage reports no active session again, so the next join
/// begins a new session.
///
/// The departure is also an occurrence of the session ([`EVENT_LEFT`]), preceded by the
/// end of its media ([`EVENT_PUBLISH_STOPPED`]) when the participant was publishing.
///
/// A departure that happened EARLIER than this call — one the clock owed — is recorded
/// through [`leave_stage_session_at`], which this is the "it happened now" case of.
///
/// `false` — nothing recorded — when this store holds no such stage, session or
/// participant, or when that participant has already left.
pub fn leave_stage_session(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
) -> bool {
    leave_stage_session_at(
        state,
        stage_arn,
        session_id,
        participant_id,
        SystemTime::now(),
    )
}

/// [`leave_stage_session`] for a departure that happened at `at` rather than now.
///
/// A departure the CLOCK owed — the replica of a replication whose publisher stayed away
/// for its whole reconnect window ([`materialize_due_replications`]) — happened at the
/// instant it fell due, whether or not anybody called then, so everything it decides is
/// measured from THAT instant: the session's end, the occurrences it records, the reconnect
/// deadline it earns a replication the departing participant sources, and the empty-stage
/// deadline it earns every composition of the stage. Writing it down late therefore leaves
/// exactly the store an on-time materialisation would have left, which is what keeps a read
/// from changing what a later caller sees.
pub fn leave_stage_session_at(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
    at: SystemTime,
) -> bool {
    let session_ended = {
        let Some(session) = session_mut(state, stage_arn, session_id) else {
            return false;
        };
        let Some(leaving) = session
            .participants
            .iter_mut()
            .find(|participant| participant.participant_id == participant_id)
        else {
            return false;
        };
        if leaving.state == PARTICIPANT_STATE_DISCONNECTED {
            return false;
        }
        leaving.state = PARTICIPANT_STATE_DISCONNECTED.to_string();
        let emptied = session
            .participants
            .iter()
            .all(|participant| participant.state == PARTICIPANT_STATE_DISCONNECTED);
        if emptied {
            session.end_time = Some(at);
        }
        emptied
    };
    // Every occurrence says whether the participant it happened to is a replica of a
    // publisher in another stage, which is what `Event$replica` reports. That is a fact
    // about the replications this store holds rather than about the departure, so it is
    // read off them ([`session_participant_replications`]) exactly as every other report
    // of it is — the same statement the join of that replica recorded.
    let replica = session_participant_replications(state, stage_arn, session_id)
        .get(participant_id)
        .is_some_and(|replication| replication.replication_type == REPLICATION_TYPE_REPLICA);
    let occurrence = |name: &str| {
        let mut event = ParticipantEvent::new(name, participant_id);
        event.replica = replica;
        event
    };
    // A participant that is publishing when it goes stops publishing first: the two
    // publish names alternate ([`StageSessionRecord::is_publishing`]), and the departure
    // is the last occurrence either way.
    if state
        .stage_session(stage_arn, session_id)
        .is_some_and(|session| session.is_publishing(participant_id))
    {
        record_participant_event(
            state,
            stage_arn,
            session_id,
            occurrence(EVENT_PUBLISH_STOPPED),
            at,
        );
    }
    record_participant_event(state, stage_arn, session_id, occurrence(EVENT_LEFT), at);
    if session_ended {
        if let Some(stage) = state.stages.get_mut(stage_arn) {
            if stage.active_session_id.as_deref() == Some(session_id) {
                stage.active_session_id = None;
            }
        }
    }
    // A replication replicates a PUBLISHING participant, so a publisher leaving puts every
    // replication it sources from this session on the reconnect deadline its own window
    // earns it, through the one place a publisher's connection is observed. The end itself
    // is the clock's ([`materialize_due_replications`]), so this records a deadline and
    // nothing else.
    observe_replication_publisher(state, stage_arn, session_id, participant_id, at);
    // A departure can leave the stage with no participant in it at all, which is when the
    // timeout the model's `StartComposition` doc ends a composition on starts running for
    // every composition of that stage ("After a 1-minute timeout, when all participants
    // are disconnected from the stage") — measured from the instant the stage was actually
    // left empty, which is this departure's.
    state.observe_stage_occupancy(stage_arn, at);
    true
}

/// Record the end of a now-ENDED replication, which happened AT `at`, as an occurrence of
/// the session its replicated publisher is in.
///
/// The ONE place the end of a replication is written into the SOURCE session, whichever way
/// that replication ended — a caller stopping it, or a stage it names going away — so the
/// session that recorded [`EVENT_REPLICATION_STARTED`] when the replication began always
/// records the counterpart when it ends, and a reader of that session's occurrences is told
/// why the replication it saw start is no longer running. The model marks
/// `Event$destinationStageArn` and `Event$destinationSessionId` "applicable only if the
/// event name is REPLICATION_STARTED or REPLICATION_STOPPED", so the occurrence carries the
/// destination the ended replication named. The publisher itself is not a replica — it
/// publishes in the source stage — so `replica` stays false.
///
/// `ended` must already have stopped being [`REPLICATION_STATE_ACTIVE`] in this store (the
/// caller has stopped it, or is about to drop its record), so that no replication still
/// running is reported as having ended. `at` is the instant it ended — a caller's stop, the
/// delete of a stage it names, or the deadline the clock ran out on it.
///
/// `false` — nothing recorded — when this store no longer holds that source session, which
/// is what the SOURCE stage of the replication being the one going away leaves.
pub fn record_replication_stopped(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    ended: &ParticipantReplicaRecord,
    at: SystemTime,
) -> bool {
    let mut event = ParticipantEvent::new(EVENT_REPLICATION_STOPPED, &ended.participant_id);
    event.destination_stage_arn = Some(ended.destination_stage_arn.clone());
    event.destination_session_id = Some(ended.destination_session_id.clone());
    record_participant_event(
        state,
        &ended.source_stage_arn,
        &ended.source_session_id,
        event,
        at,
    )
}

/// Take the replica a now-ENDED replication put into its destination stage back out of
/// that stage's session, as of `at` — the instant that replication ended.
///
/// The ONE place an ended replication's replica leaves, whichever way the replication
/// ended — a caller stopping it, or a stage it names being deleted — so no participant
/// stays [`PARTICIPANT_STATE_CONNECTED`] in a stage on behalf of a replication this store
/// no longer runs. The departure itself is [`leave_stage_session`]: the record stays and is
/// reported [`PARTICIPANT_STATE_DISCONNECTED`], and the destination session ends when it
/// held nobody else. The publisher in the SOURCE stage is untouched — it is publishing
/// there whether or not anything replicates it.
///
/// `ended` must already have stopped being [`REPLICATION_STATE_ACTIVE`] in this store (the
/// caller has stopped it, or is about to drop its record), because the replica stays in the
/// destination stage while ANY replication still running puts that same participant into
/// that same session: only the last one to end takes the replica out. `at` is the instant
/// that last replication ended, which is the instant the replica left
/// ([`leave_stage_session_at`]).
///
/// `false` — nothing recorded — when another running replication still puts that
/// participant there, or when there is no such departure to record.
pub fn release_ended_replica(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    ended: &ParticipantReplicaRecord,
    at: SystemTime,
) -> bool {
    let still_replicated = state.participant_replicas.values().flatten().any(|replica| {
        replica.participant_id == ended.participant_id
            && replica.destination_stage_arn == ended.destination_stage_arn
            && replica.destination_session_id == ended.destination_session_id
            && replica.is_active()
    });
    if still_replicated {
        return false;
    }
    leave_stage_session_at(
        state,
        &ended.destination_stage_arn,
        &ended.destination_session_id,
        &ended.participant_id,
        at,
    )
}

/// Tell the replications that `participant_id` SOURCES from the session `session_id` of
/// `stage_arn` what that publisher's connection is at `now`.
///
/// A replication replicates "a publishing participant", so it runs only while that
/// publisher is in the session it publishes in: the model states the rule on
/// `StartParticipantReplicationRequest$reconnectWindowSeconds` — "If the participant
/// disconnects and then reconnects within the specified interval, replication will continue
/// to be `ACTIVE`" — so a publisher that has left owes each replication it sources its end
/// one window after it went, and the same publisher back inside that window takes that
/// deadline away again and the replication goes on running.
///
/// Called from the two places a participant's connection changes ([`join_stage_session`] and
/// [`leave_stage_session`]), so every way a publisher goes or comes back decides the
/// deadline in one place. It is the publisher of THAT session this answers about: a
/// replication names the source session its publisher publishes in, which
/// `ParticipantReplica$sourceSessionId` reports and which no later call changes, and a
/// session "ends after the last participant leaves the stage" — so a departure that ended
/// the session left no session to come back to and the end stands.
///
/// Recording the deadline is all this does — the end itself is the clock's, written down by
/// [`materialize_due_replications`], which is also what takes the replica out of the
/// destination stage — so this is never the call that ends a replication and cannot
/// re-enter the departure it is called from.
fn observe_replication_publisher(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
    now: SystemTime,
) {
    let connected = session_participant(state, stage_arn, session_id, participant_id)
        .is_some_and(|participant| participant.state == PARTICIPANT_STATE_CONNECTED);
    // The replications SOURCED from this stage are the ones this stage's key holds; a
    // replication INTO it is a replication of another stage's publisher, whose connection
    // this says nothing about.
    let Some(sourced) = state.participant_replicas.get_mut(stage_arn) else {
        return;
    };
    for replication in sourced.iter_mut().filter(|replication| {
        replication.participant_id == participant_id
            && replication.source_session_id == session_id
            && replication.is_active()
    }) {
        if connected {
            replication.reconnect_deadline = None;
        } else if replication.reconnect_deadline.is_none() {
            // The window is the one the start asked for, so a replication whose caller
            // asked for none is owed its end at the instant the publisher went.
            let window =
                Duration::from_secs(u64::try_from(replication.reconnect_window_seconds).unwrap_or(
                    u64::try_from(DEFAULT_REPLICATION_RECONNECT_WINDOW_SECONDS).unwrap_or(0),
                ));
            replication.reconnect_deadline = Some(now + window);
        }
    }
}

/// Write down the end of every replication the clock has already made due at `now`.
///
/// A replication whose publisher has been gone for its whole reconnect window is over —
/// the rule the model states on `reconnectWindowSeconds`, read off the deadline
/// [`observe_replication_publisher`] recorded — and it ends exactly as a caller's stop ends
/// it: the record reaches its stopped state ([`ParticipantReplicaRecord::stop`]), the end is
/// an occurrence of the publisher's session ([`record_replication_stopped`]) and the replica
/// leaves the destination stage ([`release_ended_replica`]).
///
/// Each of those ends was owed at that deadline whether or not anybody called, so this
/// changes nothing a caller reading at the same instant would have seen differently — which
/// is why a reporting operation may call it, exactly as it may materialise what the clock
/// owes a composition
/// ([`ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::materialize_due_compositions`]).
///
/// Every end is written down AS OF its own deadline rather than as of this call, the same
/// discipline [`CompositionRecord::materialize_due`] keeps: the consequences an end carries
/// — the session it closes, the occurrences it records, the empty-stage deadline it earns
/// every composition of the destination stage — all run out of that instant, so a store
/// materialised late holds exactly what one materialised on time held and no read can
/// change what a later caller sees.
pub fn materialize_due_replications(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    now: SystemTime,
) {
    // Ending one replication takes its replica out of the destination stage, and that
    // replica can itself be the publisher of a FURTHER replication sourced from that stage,
    // whose own deadline is then recorded — one window after the end that released it, so
    // still in the past of `now` — and can already be due, so the walk repeats until this
    // store holds no due replication. Every pass ends at least one of the finitely many
    // running replications, so it terminates.
    loop {
        // Each end beside the deadline it fell due at, which is the instant it happened.
        // It is carried here because stopping the record is what clears the deadline it is
        // read from ([`ParticipantReplicaRecord::stop`]).
        let mut due: Vec<(SystemTime, ParticipantReplicaRecord)> = Vec::new();
        for replication in state.participant_replicas.values_mut().flatten() {
            let owed = if replication.is_active() {
                replication
                    .reconnect_deadline
                    .filter(|deadline| *deadline <= now)
            } else {
                None
            };
            if let Some(deadline) = owed {
                replication.stop();
                due.push((deadline, replication.clone()));
            }
        }
        if due.is_empty() {
            return;
        }
        // In the order they ENDED, ties in start order, so what this writes down does not
        // depend on the order this store happens to hold its replications in: the
        // occurrences of one session are in occurrence order, the departures they cause are
        // applied in the order they happened, and two replications ending at the same
        // instant are recorded in the order they were started.
        due.sort_by_key(|(deadline, ended)| (*deadline, ended.sequence));
        // Each end is written over a store in which every replication ending in this pass
        // has ALREADY stopped, which is the premise both of these take: no replication
        // still running is reported as having ended, and a replica stays in the destination
        // stage while any other replication still puts it there.
        for (deadline, ended) in &due {
            record_replication_stopped(state, ended, *deadline);
        }
        for (deadline, ended) in &due {
            release_ended_replica(state, ended, *deadline);
        }
    }
}

/// End every replication the stage `stage_arn` is an end of AT `at`, and drop the records
/// of them.
///
/// A replica is a participant of its source stage replicated into its destination stage, so
/// neither end of a replication can outlive the stage it names: a stage going away ends
/// every replication it sources and every replication into it. Ending one is not just
/// dropping its record — the replica that replication put into the DESTINATION stage leaves
/// that stage's session, through the one place an ended replication's replica leaves
/// ([`release_ended_replica`]) — so the stage that survives this delete is left exactly as
/// it would be had each of those replications been stopped instead.
///
/// The replicas replicated INTO `stage_arn` need no departure: those records are records of
/// this stage's own sessions, which go with the stage. Their SOURCE session is what survives
/// that delete, and it is left as a stop would have left it: the end of each of those
/// replications is an occurrence of it ([`record_replication_stopped`]).
///
/// `at` is the instant the stage went away, which is when each of these replications ended.
pub fn end_replications_of_stage(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    at: SystemTime,
) {
    // Each replication this stage sources reaches its stopped state FIRST, as a caller
    // stopping it would leave it: that is what makes it no longer one of the running
    // replications [`release_ended_replica`] keeps a replica in a stage for, and it is
    // still the record every report of that departing participant reads its replication
    // members off.
    let mut sourced_here: Vec<ParticipantReplicaRecord> = Vec::new();
    if let Some(sourced) = state.participant_replicas.get_mut(stage_arn) {
        for replication in sourced.iter_mut().filter(|replication| replication.is_active()) {
            replication.stop();
            sourced_here.push(replication.clone());
        }
    }
    // Every replication INTO this stage ends with it too, and reaches its stopped state the
    // same way first. Only the ones still RUNNING end here: a replication a caller already
    // stopped recorded its end then.
    let mut into_here: Vec<ParticipantReplicaRecord> = Vec::new();
    for replications in state.participant_replicas.values_mut() {
        for replication in replications.iter_mut().filter(|replication| {
            replication.destination_stage_arn == stage_arn && replication.is_active()
        }) {
            replication.stop();
            into_here.push(replication.clone());
        }
    }
    for replication in sourced_here.iter().chain(into_here.iter()) {
        // The end of a replication is an occurrence of the session its publisher is in,
        // through the one place that is written — which records nothing for a replication
        // this stage SOURCED, whose source session went with the stage above.
        record_replication_stopped(state, replication, at);
    }
    for replication in &sourced_here {
        release_ended_replica(state, replication, at);
    }
    // The replications sourced from this stage go with its key; the ones replicated INTO it
    // are dropped from the stages that source them.
    state.participant_replicas.remove(stage_arn);
    for replicas in state.participant_replicas.values_mut() {
        replicas.retain(|replica| replica.destination_stage_arn != stage_arn);
    }
}

/// The sessions of the stage `stage_arn` in which `participant_id` is still
/// [`PARTICIPANT_STATE_CONNECTED`], in the order the stage holds them.
///
/// The ONE lookup for "where in this stage is this participant still present", beside the
/// [`leave_stage_session`] that acts on the answer, so an operation that takes a
/// participant out of a stage and the records `ListParticipants` reports for it cannot
/// disagree about which sessions it was in. Empty when this store holds no such stage, or
/// when every record it holds for that participant has already left.
pub fn connected_sessions_of(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    participant_id: &str,
) -> Vec<String> {
    let Some(sessions) = state.stage_sessions.get(stage_arn) else {
        return Vec::new();
    };
    sessions
        .iter()
        .filter(|session| {
            session.participants.iter().any(|participant| {
                participant.participant_id == participant_id
                    && participant.state == PARTICIPANT_STATE_CONNECTED
            })
        })
        .map(|session| session.session_id.clone())
        .collect()
}

/// The record the session `session_id` of `stage_arn` holds for `participant_id`, if it
/// holds one.
///
/// Keyed by all three, exactly as the reading
/// [`ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::stage_session`]
/// is: a participant is a participant OF one session of one stage.
fn session_participant<'a>(
    state: &'a ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
) -> Option<&'a ParticipantRecord> {
    state
        .stage_session(stage_arn, session_id)?
        .participants
        .iter()
        .find(|participant| participant.participant_id == participant_id)
}

/// The same lookup mutably, for the one write a caller makes to a participant already in
/// the session it names.
fn session_participant_mut<'a>(
    state: &'a mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
) -> Option<&'a mut ParticipantRecord> {
    session_mut(state, stage_arn, session_id)?
        .participants
        .iter_mut()
        .find(|participant| participant.participant_id == participant_id)
}

/// Why the participants of a started replication could not both be recorded.
///
/// Each names the stage whose session could not take the record, so the operation that
/// asked can answer the modelled failure that condition is rather than one standing for
/// both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicatedParticipantRefusal {
    /// That stage's session is at [`MAX_PARTICIPANTS_PER_SESSION`] participants — this
    /// emulator's own resource guard on the records a session holds.
    SessionFull { stage_arn: String },
    /// This store holds no OPEN session of that id for that stage to record in.
    NoSuchSession { stage_arn: String },
}

/// Record the participants a started replication puts into the two sessions it names.
///
/// A replication replicates "a publishing participant from a source stage to a destination
/// stage", so the replicated publisher is a participant of BOTH sessions `replica` names:
/// the session it publishes in — which is why its stage has a session at all — and the
/// destination stage's session, where its replica is. Those are the records
/// `ListParticipants` reports, created through [`join_stage_session`], the one place a
/// participant record is created, and each of the two publishes: the participant being
/// replicated is a publisher, and its replica carries that same media into the destination
/// stage.
///
/// A session that already holds the participant as [`PARTICIPANT_STATE_CONNECTED`] keeps
/// the record it has, beyond its having published: that participant is in the stage, and a
/// replication starting is not it joining again. Where that record had not published yet,
/// the media this replication carries beginning is an occurrence of the session like any
/// other publish ([`EVENT_PUBLISH_STARTED`]). A record that has LEFT the session is not in
/// the stage any more, so a replication of that participant IS it joining again, recorded
/// the way any rejoin is ([`join_stage_session`]), and its state and the session's
/// occurrences stay in step.
///
/// What each record REPORTS about the replication is not written here — it is read off the
/// replication itself ([`session_participant_replications`]) — so `identity` supplies only what
/// issued the participant: the name its token or ingest credential carries, and the
/// attributes a replica's own are merged over.
///
/// `Err` — nothing recorded at either end — naming the stage and the condition
/// ([`ReplicatedParticipantRefusal`]): a stage with no OPEN session of that id to record
/// in, or one whose session is at [`MAX_PARTICIPANTS_PER_SESSION`] participants. Both ends
/// are checked before either is written, so one stage cannot end up holding a participant
/// the other does not, and the session checked is the session written.
pub fn record_replicated_participant(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    replica: &ParticipantReplicaRecord,
    identity: &StageParticipant,
) -> Result<(), ReplicatedParticipantRefusal> {
    // The two ends of one replication: the session each record goes into, the attributes
    // that record carries, and whether the participant there IS the replica. The
    // publisher carries the attributes its own identity was issued with; the replica
    // carries the ones the replication merged for it.
    let ends = [
        (
            &replica.source_stage_arn,
            &replica.source_session_id,
            &identity.attributes,
            false,
        ),
        (
            &replica.destination_stage_arn,
            &replica.destination_session_id,
            &replica.attributes,
            true,
        ),
    ];
    // Neither end is written until both can hold the record.
    for (stage_arn, session_id, _, _) in &ends {
        // A session that has ENDED is no session to record in: the write below joins the
        // stage's open session ([`join_stage_session`]), and a stage has an open session
        // exactly while it has a participant in one, so refusing here is what keeps the
        // session checked and the session written the same session.
        let open = state
            .stage_session(stage_arn, session_id)
            .filter(|session| session.end_time.is_none());
        let Some(session) = open else {
            return Err(ReplicatedParticipantRefusal::NoSuchSession {
                stage_arn: (*stage_arn).clone(),
            });
        };
        // A session already holding this participant records no further record for it, so
        // its size is not what stands in the way.
        let holds = session
            .participants
            .iter()
            .any(|participant| participant.participant_id == replica.participant_id);
        if !holds && session.participants.len() >= MAX_PARTICIPANTS_PER_SESSION {
            return Err(ReplicatedParticipantRefusal::SessionFull {
                stage_arn: (*stage_arn).clone(),
            });
        }
    }
    for (stage_arn, session_id, attributes, is_replica) in ends {
        // Only a participant the session holds as present keeps its record untouched: one
        // whose record has left the session is brought back in by the join below.
        let present = session_participant(state, stage_arn, session_id, &replica.participant_id)
            .is_some_and(|present| present.state == PARTICIPANT_STATE_CONNECTED);
        if present {
            // The participant is in this stage already, and a replication starting is not
            // it joining again, so its record keeps what the join recorded. It HAS
            // published, though: only a publishing participant is replicated, and
            // `published` is "whether the participant ever published to the stage session",
            // which stays set once set.
            if let Some(present) =
                session_participant_mut(state, stage_arn, session_id, &replica.participant_id)
            {
                present.published = true;
            }
            // The media this replication carries is beginning where the session had not
            // seen that participant publishing: [`EVENT_PUBLISH_STARTED`] is what begins
            // one participant's media and the two publish names alternate
            // ([`StageSessionRecord::is_publishing`]), so the occurrence is recorded for
            // exactly that transition and what `ListParticipantEvents` reports cannot
            // disagree with what `ListParticipants` does.
            if !state
                .stage_session(stage_arn, session_id)
                .is_some_and(|session| session.is_publishing(&replica.participant_id))
            {
                let mut event =
                    ParticipantEvent::new(EVENT_PUBLISH_STARTED, &replica.participant_id);
                event.replica = is_replica;
                record_participant_event(state, stage_arn, session_id, event, SystemTime::now());
            }
            continue;
        }
        let joined = JoinedParticipant {
            participant_id: replica.participant_id.clone(),
            user_id: identity.user_id.clone(),
            attributes: attributes.clone(),
            published: true,
            // How a publisher reached a stage — whether anything records it, which ingest
            // configuration it broadcast with, whether that used redundant ingest — is
            // what the stage's DATA plane observes, which a replication starting is not,
            // so each stays at what a join with nothing observed records.
            recording_state: None,
            redundant_ingest: false,
            ingest_configuration_arn: None,
            client: ParticipantClient::default(),
            protocol: None,
            replica: is_replica,
        };
        if join_stage_session(state, stage_arn, joined).is_none() {
            // The session exists and has room — both checked above — so the records it
            // holds are the only thing a join of a participant new to it can run out of.
            return Err(ReplicatedParticipantRefusal::SessionFull {
                stage_arn: stage_arn.clone(),
            });
        }
    }
    Ok(())
}

/// What the participants of the session `session_id` of `stage_arn` report about
/// replication right now, keyed by participant id, read off the replications this store
/// holds.
///
/// The replication members of a participant are a fact about a replication rather than
/// about the join that recorded the participant, so they are read from the replications
/// themselves on every report: a participant is a [`REPLICATION_TYPE_REPLICA`] while a
/// replication runs INTO this session — then also naming the stage and session it is
/// replicated from, which the model reports "if `replicationType` is `REPLICA`" — a
/// [`REPLICATION_TYPE_SOURCE`] while one runs FROM this session, and
/// [`REPLICATION_TYPE_NONE`] when no replication names it, which is the default the model
/// documents and which a participant absent from this map takes
/// ([`ParticipantReplication::default`]).
///
/// ONE walk over the replications, for the whole session: a reporting operation reads them
/// once per request and then looks each participant of its page up here, so the work a
/// page costs is the session's participants plus this store's replications rather than the
/// product of the two.
pub fn session_participant_replications(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
) -> HashMap<String, ParticipantReplication> {
    let mut replications: HashMap<String, ParticipantReplication> = HashMap::new();
    for replica in state.participant_replicas.values().flatten() {
        let candidate = if replica.destination_stage_arn == stage_arn
            && replica.destination_session_id == session_id
        {
            ParticipantReplication {
                replication_type: REPLICATION_TYPE_REPLICA.to_string(),
                replication_state: Some(replica.replication_state.clone()),
                source_stage_arn: Some(replica.source_stage_arn.clone()),
                source_session_id: Some(replica.source_session_id.clone()),
            }
        } else if replica.source_stage_arn == stage_arn && replica.source_session_id == session_id {
            ParticipantReplication {
                replication_type: REPLICATION_TYPE_SOURCE.to_string(),
                replication_state: Some(replica.replication_state.clone()),
                // Both reported only "if `replicationType` is `REPLICA`", as the two
                // members' docs state: a participant replicated INTO another stage is not
                // replicated FROM one.
                source_stage_arn: None,
                source_session_id: None,
            }
        } else {
            continue;
        };
        let held = replications.get(&replica.participant_id);
        if held.is_none_or(|held| supersedes(&candidate, held)) {
            replications.insert(replica.participant_id.clone(), candidate);
        }
    }
    replications
}

/// Whether the participant `participant_id` of the session `session_id` of `stage_arn` is
/// a [`REPLICATION_TYPE_REPLICA`] there — a participant a replication runs INTO this
/// session.
///
/// Read off [`session_participant_replications`], the crate's one answer to what a
/// participant of a session reports about replication, so "is this participant a replica"
/// and the `replicationType` that same participant reports cannot disagree.
pub fn session_participant_is_replica(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
) -> bool {
    session_participant_replications(state, stage_arn, session_id)
        .get(participant_id)
        .is_some_and(|replication| replication.replication_type == REPLICATION_TYPE_REPLICA)
}

/// Which of two replications a participant of one session reports, when more than one
/// names it.
///
/// Two orderings, so the order this store happens to hold its replications in cannot
/// decide what a read answers: being a replica IN the session is the stronger statement
/// about that participant's record than being replicated FROM it, and between two of the
/// same kind the RUNNING replication is the one reported — a publisher replicated into
/// several stages is replicating while ANY of those replications is.
fn supersedes(candidate: &ParticipantReplication, held: &ParticipantReplication) -> bool {
    let rank = |replication: &ParticipantReplication| {
        (
            replication.replication_type == REPLICATION_TYPE_REPLICA,
            replication.replication_state.as_deref() == Some(REPLICATION_STATE_ACTIVE),
        )
    };
    rank(candidate) > rank(held)
}

/// Per-service state for com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime. A handler reaches it through [`service_state`].
///
/// ONE store per engine, holding the resources of every region a caller has addressed,
/// each keyed by its own ARN — and an ARN names the region its resource was created in.
/// A request is answered from the view of the region IT was processed in, so the region
/// is a term of every lookup over these maps rather than a partition of them: the
/// accessors below take the region and the listings page in it ([`in_request_region`]),
/// and a request signed for one region finds nothing another region's endpoint created.
#[derive(Debug, Default)]
pub struct ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState {
    /// Every live stage, keyed by its ARN.
    pub stages: HashMap<String, StageRecord>,
    /// The tokens issued against a stage, keyed by that stage's ARN. Removed with
    /// the stage.
    pub participant_tokens: HashMap<String, Vec<ParticipantTokenRecord>>,
    /// The sessions a stage has had, keyed by that stage's ARN and held in start
    /// order. A session is opened by [`ensure_active_session`] and its participants
    /// written by [`join_stage_session`] / [`leave_stage_session`] — the stage's data
    /// plane, not this control plane — and the sessions go with the stage.
    pub stage_sessions: HashMap<String, Vec<StageSessionRecord>>,
    /// Every live ingest configuration, keyed by its ARN.
    pub ingest_configurations: HashMap<String, IngestConfigurationRecord>,
    /// Every live encoder configuration, keyed by its ARN.
    pub encoder_configurations: HashMap<String, EncoderConfigurationRecord>,
    /// Every live storage configuration, keyed by its ARN.
    pub storage_configurations: HashMap<String, StorageConfigurationRecord>,
    /// Every imported public key, keyed by its ARN.
    pub public_keys: HashMap<String, PublicKeyRecord>,
    /// Every composition that has been started and not yet reclaimed, keyed by its ARN.
    /// Ephemeral: the clock drops the record once the stop `StopComposition` began has
    /// finished (see [`ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::materialize_due_compositions`]).
    pub compositions: HashMap<String, CompositionRecord>,
    /// The started replications of a stage's participants, keyed by that SOURCE stage's
    /// ARN and held in start order — the order `ListParticipantReplicas` pages over.
    /// Removed when either stage a replica names is deleted.
    pub participant_replicas: HashMap<String, Vec<ParticipantReplicaRecord>>,
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

    /// The highest sequence this store has handed out: every page position any listing
    /// of this service can have issued lies in `1..=issued_through()`, because a
    /// position is the sequence of a record the store drew from [`Self::next_sequence`],
    /// which counts up from one. A token naming any other position was issued by
    /// nothing, so the page-token codec reads none from it.
    pub fn issued_through(&self) -> u64 {
        self.next_id
    }

    // -----------------------------------------------------------------------
    // The region's view of this store
    //
    // Every accessor below takes the region the calling request was processed in and
    // answers about that region's resources only, through the one region term
    // [`in_request_region`] states: a resource belongs to the region its ARN names, and
    // an operation answers from the view of its own request's region. The maps
    // themselves are reached directly only by the writers that already know which
    // region's resource they hold (a create, and the data-plane helpers that follow a
    // stored reference).
    // -----------------------------------------------------------------------

    /// The stage `arn` names in `region`'s view, or `None` when that view holds none.
    pub fn stage_in(&self, region: &str, arn: &str) -> Option<&StageRecord> {
        self.stages.get(request_scoped(arn, region)?)
    }

    /// [`Self::stage_in`], to be changed in place.
    pub fn stage_in_mut(&mut self, region: &str, arn: &str) -> Option<&mut StageRecord> {
        self.stages.get_mut(request_scoped(arn, region)?)
    }

    /// The ingest configuration `arn` names in `region`'s view, or `None` when that view
    /// holds none.
    pub fn ingest_configuration_in(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&IngestConfigurationRecord> {
        self.ingest_configurations
            .get(request_scoped(arn, region)?)
    }

    /// [`Self::ingest_configuration_in`], to be changed in place.
    pub fn ingest_configuration_in_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut IngestConfigurationRecord> {
        self.ingest_configurations
            .get_mut(request_scoped(arn, region)?)
    }

    /// The encoder configuration `arn` names in `region`'s view, or `None` when that
    /// view holds none.
    pub fn encoder_configuration_in(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&EncoderConfigurationRecord> {
        self.encoder_configurations
            .get(request_scoped(arn, region)?)
    }

    /// [`Self::encoder_configuration_in`], to be changed in place.
    pub fn encoder_configuration_in_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut EncoderConfigurationRecord> {
        self.encoder_configurations
            .get_mut(request_scoped(arn, region)?)
    }

    /// The storage configuration `arn` names in `region`'s view, or `None` when that
    /// view holds none.
    pub fn storage_configuration_in(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&StorageConfigurationRecord> {
        self.storage_configurations
            .get(request_scoped(arn, region)?)
    }

    /// [`Self::storage_configuration_in`], to be changed in place.
    pub fn storage_configuration_in_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut StorageConfigurationRecord> {
        self.storage_configurations
            .get_mut(request_scoped(arn, region)?)
    }

    /// The public key `arn` names in `region`'s view, or `None` when that view holds
    /// none.
    pub fn public_key_in(&self, region: &str, arn: &str) -> Option<&PublicKeyRecord> {
        self.public_keys.get(request_scoped(arn, region)?)
    }

    /// [`Self::public_key_in`], to be changed in place.
    pub fn public_key_in_mut(&mut self, region: &str, arn: &str) -> Option<&mut PublicKeyRecord> {
        self.public_keys.get_mut(request_scoped(arn, region)?)
    }

    /// The composition `arn` names in `region`'s view, or `None` when that view holds
    /// none. Call it after [`Self::materialize_due_compositions`], so a composition
    /// whose ephemeral resource the clock has reclaimed is not reported as live.
    pub fn composition_in(&self, region: &str, arn: &str) -> Option<&CompositionRecord> {
        self.compositions.get(request_scoped(arn, region)?)
    }

    /// [`Self::composition_in`], to be changed in place.
    pub fn composition_in_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut CompositionRecord> {
        self.compositions.get_mut(request_scoped(arn, region)?)
    }

    /// Every stage of `region` in creation order — the order `ListStages` pages over.
    pub fn stages_in_order(&self, region: &str) -> Vec<&StageRecord> {
        let mut stages: Vec<&StageRecord> = self
            .stages
            .values()
            .filter(|record| in_request_region(&record.arn, region))
            .collect();
        stages.sort_by_key(|record| record.sequence);
        stages
    }

    /// Every composition of `region` in start order — the order `ListCompositions` pages
    /// over, so a composition starting or being reclaimed mid-pagination cannot shift a
    /// later page onto an already-returned one.
    pub fn compositions_in_order(&self, region: &str) -> Vec<&CompositionRecord> {
        let mut compositions: Vec<&CompositionRecord> = self
            .compositions
            .values()
            .filter(|record| in_request_region(&record.arn, region))
            .collect();
        compositions.sort_by_key(|record| record.sequence);
        compositions
    }

    /// Every ingest configuration of `region` in creation order — the order
    /// `ListIngestConfigurations` pages over, so a configuration created or deleted
    /// mid-pagination cannot shift a later page onto an already-returned one.
    pub fn ingest_configurations_in_order(&self, region: &str) -> Vec<&IngestConfigurationRecord> {
        let mut configurations: Vec<&IngestConfigurationRecord> = self
            .ingest_configurations
            .values()
            .filter(|record| in_request_region(&record.arn, region))
            .collect();
        configurations.sort_by_key(|record| record.sequence);
        configurations
    }

    /// Every encoder configuration of `region` in creation order — the order
    /// `ListEncoderConfigurations` pages over, so a configuration created or deleted
    /// mid-pagination cannot shift a later page onto an already-returned one.
    pub fn encoder_configurations_in_order(&self, region: &str) -> Vec<&EncoderConfigurationRecord> {
        let mut configurations: Vec<&EncoderConfigurationRecord> = self
            .encoder_configurations
            .values()
            .filter(|record| in_request_region(&record.arn, region))
            .collect();
        configurations.sort_by_key(|record| record.sequence);
        configurations
    }

    /// Every storage configuration of `region` in creation order — the order
    /// `ListStorageConfigurations` pages over.
    pub fn storage_configurations_in_order(
        &self,
        region: &str,
    ) -> Vec<&StorageConfigurationRecord> {
        let mut configurations: Vec<&StorageConfigurationRecord> = self
            .storage_configurations
            .values()
            .filter(|record| in_request_region(&record.arn, region))
            .collect();
        configurations.sort_by_key(|record| record.sequence);
        configurations
    }

    /// Every public key of `region` in import order — the order `ListPublicKeys` pages
    /// over, so a key imported or deleted mid-pagination cannot shift a later page onto
    /// an already-returned one.
    pub fn public_keys_in_order(&self, region: &str) -> Vec<&PublicKeyRecord> {
        let mut public_keys: Vec<&PublicKeyRecord> = self
            .public_keys
            .values()
            .filter(|record| in_request_region(&record.arn, region))
            .collect();
        public_keys.sort_by_key(|record| record.sequence);
        public_keys
    }

    /// Every session the stage `stage_arn` has had, in start order — the order
    /// `ListStageSessions` pages over, so a session opening or ending mid-pagination
    /// cannot shift a later page onto an already-returned one. A stage `region`'s view
    /// holds no session for (one that never had a participant, one it does not hold at
    /// all, or one of another region) has none.
    pub fn stage_sessions_in_start_order(
        &self,
        region: &str,
        stage_arn: &str,
    ) -> Vec<&StageSessionRecord> {
        let mut sessions: Vec<&StageSessionRecord> = request_scoped(stage_arn, region)
            .and_then(|stage_arn| self.stage_sessions.get(stage_arn))
            .map(|sessions| sessions.iter().collect())
            .unwrap_or_default();
        sessions.sort_by_key(|record| record.sequence);
        sessions
    }

    /// The session `session_id` of the stage `stage_arn` in `region`'s view, or `None`
    /// when that view holds no such session for that stage.
    ///
    /// The lookup is by BOTH keys: a session id names a session of one stage, so a
    /// session of another stage is not found here even though every id this store
    /// mints is unique.
    pub fn stage_session_in(
        &self,
        region: &str,
        stage_arn: &str,
        session_id: &str,
    ) -> Option<&StageSessionRecord> {
        self.stage_session(request_scoped(stage_arn, region)?, session_id)
    }

    /// The replications sourced from the stage `stage_arn` in `region`'s view, in the
    /// order they were started. Empty for a stage that view holds no replication for.
    pub fn participant_replicas_in(
        &self,
        region: &str,
        stage_arn: &str,
    ) -> &[ParticipantReplicaRecord] {
        request_scoped(stage_arn, region)
            .and_then(|stage_arn| self.participant_replicas.get(stage_arn))
            .map_or(&[], Vec::as_slice)
    }

    /// The session `session_id` of the stage `stage_arn`, whatever region that stage is
    /// in, or `None` when this store holds no such session for it.
    ///
    /// The region-less lookup, for the writers and the data-plane helpers that already
    /// hold the stage: a handler resolving an ARN a REQUEST supplied uses
    /// [`Self::stage_session_in`] instead, so it answers only about its own region.
    pub fn stage_session(&self, stage_arn: &str, session_id: &str) -> Option<&StageSessionRecord> {
        self.stage_sessions
            .get(stage_arn)?
            .iter()
            .find(|session| session.session_id == session_id)
    }

    /// Write down every composition transition the clock has already made due, and drop
    /// the compositions whose ephemeral resource it has reclaimed.
    ///
    /// Each one was owed at [`CompositionRecord::active_at`], at
    /// [`CompositionRecord::empty_stage_stop_at`], or one
    /// [`COMPOSITION_STOP_SPAN`] after an end began, whether or not anybody called, so this
    /// changes no composition that a caller reading at the same instant would have seen
    /// differently — which is why a reporting operation may call it. A stopped
    /// composition's record goes the same way: the model says the "Composition stops and
    /// the resource is deleted … When StopComposition is called", and after the same
    /// published timeout when the stage it composites holds no participant, so the record
    /// is reclaimed rather than kept in a terminal state forever — but only one whole
    /// [`COMPOSITION_RETENTION_SPAN`] after it reached that terminal state, so the stop a
    /// caller is waiting for is readable long enough to be read.
    pub fn materialize_due_compositions(&mut self, now: SystemTime) {
        for composition in self.compositions.values_mut() {
            composition.materialize_due(now);
        }
        self.compositions
            .retain(|_, composition| !composition.is_reclaimed(now));
    }

    /// Whether the stage `stage_arn` holds any participant right now: a session of it
    /// with a [`PARTICIPANT_STATE_CONNECTED`] record in it.
    ///
    /// The ONE answer to "are there participants in this stage", so what a composition's
    /// empty-stage timeout turns on is exactly what `ListParticipants` reports of the same
    /// stage. `false` for a stage this store holds no session of, and for one whose every
    /// record has left.
    pub fn stage_has_participants(&self, stage_arn: &str) -> bool {
        self.stage_sessions.get(stage_arn).is_some_and(|sessions| {
            sessions.iter().any(|session| {
                session
                    .participants
                    .iter()
                    .any(|participant| participant.state == PARTICIPANT_STATE_CONNECTED)
            })
        })
    }

    /// Tell every composition of `stage_arn` what that stage holds at `now`
    /// ([`CompositionRecord::observe_stage_occupancy`]).
    ///
    /// Called from each of the three places that answer can change — a participant joining
    /// ([`join_stage_session`]), one leaving ([`leave_stage_session`]) and a composition
    /// starting against a stage — so the deadline the clock owes a composition's stop at is
    /// decided in one place for all of them, and a composition of a stage nobody is in
    /// cannot go on composing forever.
    pub fn observe_stage_occupancy(&mut self, stage_arn: &str, now: SystemTime) {
        // What the clock already owed is written down FIRST, so a participant arriving
        // after a composition's timeout has run out cannot take back a stop that was
        // already due: the compositions this reaches are the ones a caller reading at
        // this instant would have seen.
        self.materialize_due_compositions(now);
        let occupied = self.stage_has_participants(stage_arn);
        for composition in self
            .compositions
            .values_mut()
            .filter(|composition| composition.stage_arn == stage_arn)
        {
            composition.observe_stage_occupancy(occupied, now);
        }
    }

    /// The composition of `region` started under the `@idempotencyToken` `token`, if that
    /// region's view holds one.
    ///
    /// A composition is keyed by its own ARN, so the token is searched for rather than
    /// indexed: the store is bounded by [`MAX_COMPOSITIONS`], and a repeat of a start
    /// request is what this answers. A token is a token of the region it was first used
    /// in — the composition it names exists only there — so the same token offered
    /// through another region's endpoint names nothing and starts a composition of its
    /// own.
    pub fn composition_by_idempotency_token(
        &self,
        region: &str,
        token: &str,
    ) -> Option<&CompositionRecord> {
        self.compositions.values().find(|composition| {
            composition.idempotency_token.as_deref() == Some(token)
                && in_request_region(&composition.arn, region)
        })
    }

    /// The composition using the resource `arn` right now, if this store holds one: the
    /// first one started among those whose destinations refer to it
    /// ([`CompositionRecord::references`]) and whose stop has not completed
    /// ([`CompositionRecord::is_using_resources`]).
    ///
    /// The ONE lookup for "is a composition using this resource", so what a delete of an
    /// encoder or storage configuration refuses over is exactly what the compositions
    /// `GetComposition` reports say. Call it after
    /// [`Self::materialize_due_compositions`], so a composition whose stop the clock has
    /// already finished holds nothing back.
    pub fn composition_using(&self, arn: &str) -> Option<&CompositionRecord> {
        self.compositions
            .values()
            .filter(|composition| composition.is_using_resources() && composition.references(arn))
            .min_by_key(|composition| composition.sequence)
    }
}

// The seven CORS and security response headers this model binds as output members are a
// property of the ENDPOINT, not of the resource any one response reports, and AWS answers
// every call of this service with the same fixed value for each. They are stated once
// below, from the recorded responses of the operations whose output shapes bind them, so
// one answer of this service cannot publish a different policy from another.
//
// Two of the seven are BROWSER-FACING in a way the rest are not — the cross-origin read
// grant, and the content policy that travels with it — so those two are not this
// service's to assert by default: see [`BrowserPolicy`].

/// Which web origin may read an answer of this endpoint, and what such an answer may load.
///
/// `CreateIngestConfigurationResponse$accessControlAllowOrigin` and `$contentSecurityPolicy`
/// bind `@httpHeader("Access-Control-Allow-Origin")` and
/// `@httpHeader("Content-Security-Policy")`
/// (`smithy_query(query="shape",shape="CreateIngestConfigurationResponse")`), and the
/// recorded AWS responses carry the any-origin grant together with a same-origin content
/// policy. Those two are the pair that decides what a web page served by SOMEONE ELSE'S
/// origin may do with an answer of this emulator, and this emulator is reached by whoever
/// can reach its listener — nothing about a request identifies its caller. So the recorded
/// grant is NOT the default here: by default this service asserts no origin at all, which
/// leaves the answer unreadable to a cross-origin page, and the content policy allows no
/// source of any kind, which is right for a JSON answer no browser renders.
///
/// An operator who is emulating a browser client, and therefore needs the answers readable
/// cross-origin the way AWS's are, selects the recorded pair with [`BROWSER_POLICY_VAR`];
/// selecting it logs a warning naming what it opens. Every other header below is the
/// recorded value unconditionally: none of them grants anything to any origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserPolicy {
    /// The origin this endpoint grants a read to, or `None` to grant none — in which case
    /// the modelled member is left absent and no such header is written.
    pub allow_origin: Option<&'static str>,
    /// What an answer of this endpoint may load.
    pub content_security_policy: &'static str,
}

impl BrowserPolicy {
    /// The default: no cross-origin read granted, and an answer that may load nothing.
    #[must_use]
    pub const fn confined() -> Self {
        Self {
            allow_origin: None,
            content_security_policy: CONFINED_CONTENT_SECURITY_POLICY,
        }
    }

    /// The pair the recorded AWS responses carry: the any-origin read grant, and a
    /// same-origin content policy whose subresource references are upgraded to HTTPS
    /// before they are fetched.
    #[must_use]
    pub const fn recorded() -> Self {
        Self {
            allow_origin: Some(RECORDED_ACCESS_CONTROL_ALLOW_ORIGIN),
            content_security_policy: RECORDED_CONTENT_SECURITY_POLICY,
        }
    }
}

/// The variable an operator selects the recorded browser-facing pair with.
///
/// Read ONCE per process ([`browser_policy`]). Absent — the default — is
/// [`BrowserPolicy::confined`].
pub const BROWSER_POLICY_VAR: &str = "EMULATOR_IVSREALTIME_RECORDED_BROWSER_POLICY";

/// The settings of [`BROWSER_POLICY_VAR`] that select [`BrowserPolicy::recorded`], compared
/// case-insensitively after trimming. Anything else — including an unset variable and an
/// empty one — keeps the confined default, so the widening happens only where it was asked
/// for in as many words.
pub const BROWSER_POLICY_ENABLED_SETTINGS: [&str; 3] = ["1", "true", "yes"];

/// The policy `setting` selects: [`BrowserPolicy::recorded`] only for a setting
/// [`BROWSER_POLICY_ENABLED_SETTINGS`] names, [`BrowserPolicy::confined`] for every other
/// setting and for no setting at all.
#[must_use]
pub fn browser_policy_from_setting(setting: Option<&str>) -> BrowserPolicy {
    let selected = setting.is_some_and(|value| {
        let value = value.trim();
        BROWSER_POLICY_ENABLED_SETTINGS
            .iter()
            .any(|enabled| value.eq_ignore_ascii_case(enabled))
    });
    if selected {
        BrowserPolicy::recorded()
    } else {
        BrowserPolicy::confined()
    }
}

/// The policy this process answers with, from [`BROWSER_POLICY_VAR`].
///
/// Read once, so every answer of one run publishes the same policy, and the warning the
/// widening owes is logged once rather than per response.
pub fn browser_policy() -> BrowserPolicy {
    static POLICY: LazyLock<BrowserPolicy> = LazyLock::new(|| {
        let policy = browser_policy_from_setting(std::env::var(BROWSER_POLICY_VAR).ok().as_deref());
        if policy.allow_origin.is_some() {
            tracing::warn!(
                setting = BROWSER_POLICY_VAR,
                "ivs-realtime answers are now readable by ANY web origin: a page served by \
                 any site the developer's browser visits can read this emulator's responses, \
                 including the ingest keys an ingest configuration reports. Unset the \
                 variable to grant no origin."
            );
        }
        policy
    });

    *POLICY
}

/// Response headers a browser client may read off an answer of this endpoint, where a
/// cross-origin read is granted at all.
///
/// The service's own fixed list, as the recorded responses state it: every header its
/// answers carry — the two CORS headers, the five caching and security policies, the two
/// content headers, and the six the model's `@cors` trait also names in
/// `additionalExposedHeaders` (`smithy_query(query="service_info")`: `x-amzn-errortype`,
/// `x-amzn-requestid`, `x-amzn-errormessage`, `x-amzn-trace-id`, `x-amz-apigw-id` and
/// `date`). Stated in the order and spelling the recorded responses use, since the value a
/// client reads is the header's text. It names which headers are readable once a grant
/// exists and grants nothing itself, so it is the recorded value whatever
/// [`browser_policy`] answers.
pub const ACCESS_CONTROL_EXPOSE_HEADERS: &str = "Access-Control-Allow-Origin,\
     Access-Control-Expose-Headers,Cache-Control,Content-Length,Content-Security-Policy,\
     Content-Type,date,Strict-Transport-Security,x-amz-apigw-id,x-amzn-errormessage,\
     x-amzn-errortype,x-amzn-requestid,x-amzn-trace-id,X-Content-Type-Options,\
     X-Frame-Options";

/// How long an answer of this endpoint may be reused: not at all, by either directive the
/// recorded responses state — no store of it, and no cached copy served without
/// revalidation.
///
/// Every operation of this service reports state a later call can change — a stage's
/// `activeSessionId`, an ingest configuration's `state` — so no answer is a representation
/// a client or a proxy may serve again.
pub const CACHE_CONTROL: &str = "no-store, no-cache";

/// The cross-origin read grant the recorded responses carry; served only where
/// [`BrowserPolicy::recorded`] is selected.
pub const RECORDED_ACCESS_CONTROL_ALLOW_ORIGIN: &str = "*";

/// What an answer may load under [`BrowserPolicy::recorded`]: the policy the recorded
/// responses state — same-origin sources only, and any subresource reference upgraded to
/// HTTPS before it is fetched.
pub const RECORDED_CONTENT_SECURITY_POLICY: &str = "default-src 'self'; upgrade-insecure-requests;";

/// What an answer may load under [`BrowserPolicy::confined`]: nothing. The answer is a JSON
/// document, never one a browser renders, so no source of any kind is allowed.
pub const CONFINED_CONTENT_SECURITY_POLICY: &str = "default-src 'none'";

/// How long a client holds this endpoint to TLS, stated as the header's `max-age` in
/// seconds and applied to its subdomains too. The span is this emulator's own choice; what
/// the policy says is that a client which once reached the endpoint never tries cleartext
/// to it again.
pub const STRICT_TRANSPORT_SECURITY: &str = "max-age=47304000; includeSubDomains";

/// Whether a client may second-guess the content type of an answer: it may not. The one
/// value this header defines.
pub const X_CONTENT_TYPE_OPTIONS: &str = "nosniff";

/// Whether a browser may frame an answer of this endpoint: it may not. The header defines
/// `DENY` and `SAMEORIGIN`, and an API answer belongs in no frame at all.
pub const X_FRAME_OPTIONS: &str = "DENY";

/// The headers above, each paired with the name the model's `@httpHeader` trait gives it:
/// the five every answer carries, plus the cross-origin grant where [`browser_policy`]
/// selects one.
///
/// A SUCCESS does not go through this table: there they are modelled output members
/// ([`ResponseHeaderPolicy`]), so the protocol serializer writes each under the name its
/// own `@httpHeader` trait carries. The table is for the responses whose modelled members
/// never reach the wire — every failure of this service renders the one-member body
/// `crate::scaffolded::STATED_ERROR_SCHEMA` describes rather than the modelled error
/// shape, though the same members are on each of those shapes too — so `crate::service`
/// writes them onto the response itself, at the one place every response passes through.
/// The set is the SAME one a success carries, so the policy a client reads off this
/// endpoint does not depend on whether the call succeeded.
#[must_use]
pub fn response_header_policy() -> Vec<(&'static str, &'static str)> {
    let browser = browser_policy();
    let mut policy = Vec::with_capacity(7);
    if let Some(origin) = browser.allow_origin {
        policy.push(("Access-Control-Allow-Origin", origin));
    }
    policy.extend([
        (
            "Access-Control-Expose-Headers",
            ACCESS_CONTROL_EXPOSE_HEADERS,
        ),
        ("Cache-Control", CACHE_CONTROL),
        ("Content-Security-Policy", browser.content_security_policy),
        ("Strict-Transport-Security", STRICT_TRANSPORT_SECURITY),
        ("X-Content-Type-Options", X_CONTENT_TYPE_OPTIONS),
        ("X-Frame-Options", X_FRAME_OPTIONS),
    ]);
    policy
}

/// An output shape of this service that binds the CORS and security response headers.
///
/// Four of this model's output shapes bind those members —
/// `CreateIngestConfigurationResponse`, `StartParticipantReplicationResponse`,
/// `StopParticipantReplicationResponse` and `UpdateIngestConfigurationResponse`
/// (`smithy_query(query="shape",shape="CreateIngestConfigurationResponse")`) — and the
/// values are a property of the ENDPOINT rather than of the resource any one of them
/// reports, so they are filled in here, in one place, instead of in each handler.
pub trait ResponseHeaderPolicy {
    /// This output with each member the shape binds carrying the endpoint's value for it,
    /// so the protocol serializer writes them on the response. The cross-origin grant is
    /// the one member that may be left absent — [`browser_policy`] decides, and an absent
    /// member writes no header, which grants no origin.
    #[must_use]
    fn with_response_header_policy(self) -> Self;
}

/// Implement [`ResponseHeaderPolicy`] for each output shape that binds those members, so
/// one shape cannot report a different policy from another.
macro_rules! impl_response_header_policy {
    ($($output:ty),+ $(,)?) => {
        $(impl ResponseHeaderPolicy for $output {
            fn with_response_header_policy(mut self) -> Self {
                let browser = browser_policy();
                self.access_control_allow_origin =
                    browser.allow_origin.map(::std::string::ToString::to_string);
                self.access_control_expose_headers =
                    Some(ACCESS_CONTROL_EXPOSE_HEADERS.to_string());
                self.cache_control = Some(CACHE_CONTROL.to_string());
                self.content_security_policy =
                    Some(browser.content_security_policy.to_string());
                self.strict_transport_security = Some(STRICT_TRANSPORT_SECURITY.to_string());
                self.x_content_type_options = Some(X_CONTENT_TYPE_OPTIONS.to_string());
                self.x_frame_options = Some(X_FRAME_OPTIONS.to_string());
                self
            }
        })+
    };
}

// The still-scaffolded operation of the four is implemented here as well: the policy is
// the shape's, not the handler's, so implementing that operation is a call to this trait
// rather than seven more assignments.
impl_response_header_policy!(
    CreateIngestConfigurationOutput,
    StartParticipantReplicationOutput,
    StopParticipantReplicationOutput,
    UpdateIngestConfigurationOutput,
);

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
    internal_server(variant, STORE_UNAVAILABLE_MESSAGE)
}

/// The modelled 500 `variant` names, stating `message`. The one construction behind
/// every `InternalServerException` this service answers, so two of them cannot be
/// assembled two ways.
fn internal_server<E>(variant: fn(InternalServerException) -> E, message: &str) -> Scaffolded<E> {
    Scaffolded::stated(
        variant(InternalServerException {
            exception_message: Some(message.to_string()),
            ..Default::default()
        }),
        message.to_string(),
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
    internal_server(variant, TOKEN_MINTING_FAILED_MESSAGE)
}

/// The text [`token_minting_failed`] states.
pub const TOKEN_MINTING_FAILED_MESSAGE: &str =
    "the participant token could not be minted, so no token was issued";

/// The modelled 500 a handler answers when the emulator's credential store minted no
/// secret for an ingest key, so no ingest configuration is created rather than one
/// carrying a stream key this service made up itself.
pub fn stream_key_minting_failed<E>(variant: fn(InternalServerException) -> E) -> Scaffolded<E> {
    internal_server(variant, STREAM_KEY_MINTING_FAILED_MESSAGE)
}

/// The text [`stream_key_minting_failed`] states.
pub const STREAM_KEY_MINTING_FAILED_MESSAGE: &str =
    "the ingest stream key could not be minted, so no ingest configuration was created";

/// Characters in a minted stage id. The model's `StageArn` pattern allows
/// `[a-zA-Z0-9-]+` after `stage/`, and caps the whole ARN at 128 characters.
pub const STAGE_ID_LEN: usize = 12;

/// Characters in a minted participant id. The model's `ParticipantTokenId` allows
/// `[a-zA-Z0-9-_]{0,64}`.
pub const PARTICIPANT_ID_LEN: usize = 12;

/// Characters in a minted ingest-configuration id. The model's
/// `IngestConfigurationArn` pattern allows `[a-zA-Z0-9-]+` after
/// `ingest-configuration/`, and caps the whole ARN at 128 characters.
pub const INGEST_CONFIGURATION_ID_LEN: usize = 12;

/// Characters in a minted encoder-configuration id. The model's
/// `EncoderConfigurationArn` pattern allows `[a-zA-Z0-9-]+` after
/// `encoder-configuration/`, and caps the whole ARN at 128 characters.
pub const ENCODER_CONFIGURATION_ID_LEN: usize = 12;

/// Characters in a minted storage-configuration id. The model's
/// `StorageConfigurationArn` pattern allows `[a-zA-Z0-9-]+` after
/// `storage-configuration/`, and caps the whole ARN at 128 characters.
pub const STORAGE_CONFIGURATION_ID_LEN: usize = 12;

/// Characters in a minted public-key id. The model's `PublicKeyArn` pattern allows
/// `[a-zA-Z0-9-]+` after `public-key/`, and caps the whole ARN at 128 characters.
pub const PUBLIC_KEY_ID_LEN: usize = 12;

/// Characters a minted stage session id carries after its `st-` prefix.
///
/// The model's `StageSessionId` is `@length 16..16` with `@pattern ^st-[a-zA-Z0-9]+$`,
/// so a session id is `st-` followed by exactly thirteen id characters.
pub const STAGE_SESSION_ID_LEN: usize = 13;

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

/// The ingest-configuration ARN for `ingest_configuration_id`, in the form the model's
/// `IngestConfigurationArn` pattern requires:
/// `arn:aws:ivs:<region>:<account>:ingest-configuration/<id>`.
pub fn ingest_configuration_arn(region: &str, ingest_configuration_id: &str) -> String {
    ::engine::emulator_arn(
        "ivs",
        region,
        &format!("ingest-configuration/{ingest_configuration_id}"),
    )
}

/// The encoder-configuration ARN for `encoder_configuration_id`, in the form the model's
/// `EncoderConfigurationArn` pattern requires:
/// `arn:aws:ivs:<region>:<account>:encoder-configuration/<id>`.
pub fn encoder_configuration_arn(region: &str, encoder_configuration_id: &str) -> String {
    ::engine::emulator_arn(
        "ivs",
        region,
        &format!("encoder-configuration/{encoder_configuration_id}"),
    )
}

/// The storage-configuration ARN for `storage_configuration_id`, in the form the
/// model's `StorageConfigurationArn` pattern requires:
/// `arn:aws:ivs:<region>:<account>:storage-configuration/<id>`.
pub fn storage_configuration_arn(region: &str, storage_configuration_id: &str) -> String {
    ::engine::emulator_arn(
        "ivs",
        region,
        &format!("storage-configuration/{storage_configuration_id}"),
    )
}

/// The public-key ARN for `public_key_id`, in the form the model's `PublicKeyArn`
/// pattern requires: `arn:aws:ivs:<region>:<account>:public-key/<id>`.
pub fn public_key_arn(region: &str, public_key_id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("public-key/{public_key_id}"))
}

/// An identity this store has just minted for a resource it is about to hold.
#[derive(Debug)]
pub struct MintedResource {
    /// The store sequence the identity was drawn at, which the record keeps as its page
    /// position so a list pages the resources in creation order.
    pub sequence: u64,
    /// The id itself: [`mint_id`] over the drawn sequence, at the length the resource's
    /// modelled id allows.
    pub id: String,
    /// The ARN naming [`Self::id`], built by the resource's own ARN constructor.
    pub arn: String,
}

/// A fresh id of `id_len` characters that no resource `taken` reports as already named,
/// re-minted from a fresh sequence while it is, with the sequence first drawn.
///
/// The ONE place this service's "mint a fresh id, re-mint on collision" rule lives, so
/// every identity it issues is unique by construction and not by probability. The id and
/// the drawn sequence are independent: the sequence is the record's page position, kept
/// whatever re-minting the collision check needed.
fn mint_unique_id<T>(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    id_len: usize,
    taken: T,
) -> (u64, String)
where
    T: Fn(&ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState, &str) -> bool,
{
    let sequence = state.next_sequence();
    let mut id = mint_id(sequence, id_len);
    while taken(state, &id) {
        let retry = state.next_sequence();
        id = mint_id(retry, id_len);
    }
    (sequence, id)
}

/// A fresh identity for a resource keyed by its ARN: an id of `id_len` characters, the
/// ARN `arn_of` builds around it, and the sequence the record keeps.
///
/// `holds` answers whether this store already holds a resource under a candidate ARN —
/// the lookup of the map the caller is about to insert into — and the identity is
/// re-minted while it does, through the one rule [`mint_unique_id`] states. Every
/// creating operation of this service mints through here, so one ARN can never denote
/// two resources and the six creates cannot drift apart on how they ensure it.
pub fn mint_unique_arn<A, H>(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    id_len: usize,
    arn_of: A,
    holds: H,
) -> MintedResource
where
    A: Fn(&str) -> String,
    H: Fn(&ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState, &str) -> bool,
{
    let (sequence, id) = mint_unique_id(state, id_len, |state, candidate| {
        holds(state, &arn_of(candidate))
    });
    let arn = arn_of(&id);
    MintedResource { sequence, id, arn }
}

/// When a token issued now for `duration_minutes` expires.
pub fn expiration_time(duration_minutes: i32) -> SystemTime {
    let minutes = u64::try_from(duration_minutes).unwrap_or(0);
    SystemTime::now() + Duration::from_secs(minutes.saturating_mul(60))
}

/// A fresh stage session id, in the form the model's `StageSessionId` requires:
/// `st-` plus [`STAGE_SESSION_ID_LEN`] id characters, sixteen in total.
pub fn stage_session_id(sequence: u64) -> String {
    format!("st-{}", mint_id(sequence, STAGE_SESSION_ID_LEN))
}

/// The id of the session active in the stage `stage_arn`, opening one if the stage has
/// none, or `None` when this store holds no such stage (or when it is at
/// [`MAX_STAGE_SESSIONS_PER_STAGE`] sessions, which no emulated use reaches).
///
/// A stage session is the span a stage has participants in: the model's `StageSession`
/// doc describes it as beginning "when the first participant joins" and the stage
/// reporting it as `activeSessionId` until it ends. Nothing joins an emulated stage over
/// the API, so the two things that put a participant into a stage open the session it
/// needs through here — `StartParticipantReplication`, which puts a publisher's replica
/// into the destination stage and names the session its publisher is in on the source
/// side, and [`join_stage_session`], the stage data plane's own entry point. A stage that
/// already has one keeps it, so every replica and every participant of one stage names
/// the same session and a caller that read `activeSessionId` off the stage reads the same
/// value again.
///
/// A stage session "begins when the first participant joins a stage", so it begins at `at`
/// — the instant of the join that needed it.
///
/// The ONE place a session begins, so the id a stage reports and the
/// [`StageSessionRecord`] the reporting operations read are one session and not two.
pub fn ensure_active_session(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    at: SystemTime,
) -> Option<String> {
    let active = state
        .stages
        .get(stage_arn)?
        .active_session_id
        .clone()
        // The empty value is the "no session" reading of the member
        // ([`NO_ACTIVE_SESSION_ID`]), not a session id.
        .filter(|session_id| !session_id.is_empty());
    if let Some(session_id) = active {
        return Some(session_id);
    }
    // Bound the one store a session opening grows; the records are reclaimed with the
    // stage.
    if state.stage_sessions.get(stage_arn).map_or(0, Vec::len) >= MAX_STAGE_SESSIONS_PER_STAGE {
        return None;
    }
    let sequence = state.next_sequence();
    let session_id = stage_session_id(sequence);
    state
        .stage_sessions
        .entry(stage_arn.to_string())
        .or_default()
        .push(StageSessionRecord {
            session_id: session_id.clone(),
            start_time: at,
            end_time: None,
            participants: Vec::new(),
            events: Vec::new(),
            sequence,
        });
    // The stage now reports the session a caller passes to `ListParticipants`, which is
    // what `Stage$activeSessionId` is for.
    state.stages.get_mut(stage_arn)?.active_session_id = Some(session_id.clone());
    Some(session_id)
}

/// What this service holds about one participant of one stage.
#[derive(Debug, Clone)]
pub struct StageParticipant {
    /// Whether the participant may publish to the stage. Only a publisher can be
    /// replicated, and only an identity that carries [`PUBLISH_CAPABILITY`] — or an
    /// ingest credential, which exists to broadcast — may publish.
    pub publishes: bool,
    /// The customer-assigned name the participant's identity carries, when it carries
    /// one: the `userId` a record of this participant reports.
    pub user_id: Option<String>,
    /// The attributes the participant's identity carries, which a replica's attributes
    /// are merged over.
    pub attributes: HashMap<String, String>,
}

/// The identity this service ISSUED for `participant_id` against the stage `stage_arn`,
/// or `None` when it issued none.
///
/// A participant id is "assigned by IVS and returned by `CreateParticipantToken`" (the
/// model's doc for the member, on every operation that takes one) or minted with an
/// ingest configuration's credentials, so the identities this store holds for a stage are
/// exactly its token records and the credentials of the ingest configurations associated
/// with it. The ONE lookup for "what did this service issue this participant for this
/// stage, and may it publish there" — the capability a request to publish on its behalf
/// turns on, and the name and attributes such a request carries over.
///
/// This is NOT the question "is this participant in this stage": a stage holds whatever
/// participant records its sessions hold, including one whose identity was issued against
/// another stage, which is [`participant_attached_to_stage`].
pub fn stage_participant(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    participant_id: &str,
) -> Option<StageParticipant> {
    if let Some(record) = state
        .participant_tokens
        .get(stage_arn)
        .and_then(|records| {
            records
                .iter()
                .find(|record| record.participant_id == participant_id)
        })
    {
        return Some(StageParticipant {
            publishes: record
                .capabilities
                .iter()
                .any(|capability| capability == PUBLISH_CAPABILITY),
            user_id: record.user_id.clone(),
            attributes: record.attributes.clone(),
        });
    }
    state
        .ingest_configurations
        .values()
        .find(|configuration| {
            configuration.stage_arn == stage_arn
                && configuration.mints_participant(participant_id)
        })
        .map(|configuration| StageParticipant {
            // An ingest configuration is a publisher's credential: it carries the ingest
            // key a broadcast is published with, so its participant publishes.
            publishes: true,
            // The user id this credential was created WITH, or none when it was created
            // without one: the configuration materialises that member as the empty string
            // for its own projections ([`UNNAMED_PARTICIPANT_USER_ID`]), and a participant
            // carries a user id only where one was really named.
            user_id: (!configuration.user_id.is_empty()).then(|| configuration.user_id.clone()),
            attributes: configuration.attributes.clone(),
        })
}

/// Whether the participant `participant_id` is attached to the stage `stage_arn` — the
/// relation `DisconnectParticipant$stageArn` names ("ARN of the stage to which the
/// participant is attached").
///
/// A stage has the participants its own records hold: every participant record in a
/// session of that stage, whatever opened it and whatever state it is now in — a join over
/// the stage's data plane, or a replication putting a replica of another stage's publisher
/// into it ([`record_replicated_participant`]) — together with the identities this service
/// issued against the stage ([`stage_participant`]), which a holder can present to the
/// stage whether or not it has joined yet.
///
/// The ONE answer to "does this stage have this participant", so an operation that acts on
/// a participant of a stage and the readers that report that stage's participants
/// (`ListParticipants`, `GetParticipant`) cannot disagree about which participants the
/// stage has. `false` for a stage this store holds nothing of, and for a participant whose
/// records and identities all belong to other stages.
pub fn participant_attached_to_stage(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    participant_id: &str,
) -> bool {
    let recorded = state.stage_sessions.get(stage_arn).is_some_and(|sessions| {
        sessions.iter().any(|session| {
            session
                .participants
                .iter()
                .any(|participant| participant.participant_id == participant_id)
        })
    });
    recorded || stage_participant(state, stage_arn, participant_id).is_some()
}

/// One publisher broadcasting with an ingest configuration right now: the session of
/// the configuration's stage it is in, and the participant it publishes as.
#[derive(Debug, Clone)]
pub struct IngestPublisher {
    /// The session of the configuration's stage the publisher joined.
    pub session_id: String,
    /// The participant the publisher appears as in that session.
    pub participant_id: String,
}

/// The publishers broadcasting with `configuration` right now, in the order the
/// sessions of its stage hold them.
///
/// `IngestConfiguration$state` is `ACTIVE` "if a publisher currently is publishing to
/// the stage associated with the ingest configuration", so a publisher of a
/// configuration is a participant of a session of THAT stage which is still
/// [`PARTICIPANT_STATE_CONNECTED`], which is publishing MEDIA to that session right now
/// ([`StageSessionRecord::is_publishing`]), and which reached the stage with this
/// configuration's credentials — recorded with the configuration's ARN, or publishing
/// as one of the participants its ingest credentials were minted for. Those are the
/// records the stage's data plane writes ([`join_stage_session`]) and
/// `ListParticipants` reports; a configuration associated with no stage
/// ([`NO_ASSOCIATED_STAGE_ARN`]) has nothing to publish to, so it has no publisher.
///
/// Connection alone is NOT enough: a participant that joined to subscribe only, and one
/// whose media has stopped, is connected to the stage without publishing to it, and the
/// member's condition is "currently is publishing". The same wording governs what hangs
/// off this lookup — a delete refuses only while "the publisher is actively streaming to
/// a stage", and `force` disconnects the participant only "when a participant is actively
/// publishing" — so publishing is required here, in the one place, rather than at each
/// caller.
///
/// The ONE lookup for "is a publisher broadcasting with this configuration", so what a
/// read reports about a configuration and what a delete refuses over cannot disagree.
pub fn ingest_configuration_publishers(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    configuration: &IngestConfigurationRecord,
) -> Vec<IngestPublisher> {
    let mut publishers = Vec::new();
    if configuration.stage_arn == NO_ASSOCIATED_STAGE_ARN {
        return publishers;
    }
    let Some(sessions) = state.stage_sessions.get(&configuration.stage_arn) else {
        return publishers;
    };
    for session in sessions {
        for participant in &session.participants {
            if participant.state != PARTICIPANT_STATE_CONNECTED {
                continue;
            }
            // Publishing right now, read off the session's own occurrences through the
            // crate's single answer to that question — not `ParticipantRecord::published`,
            // which is "whether the participant EVER published" and stays set once set.
            if !session.is_publishing(&participant.participant_id) {
                continue;
            }
            let publishes_with_configuration = configuration.publishes_participant(participant);
            if publishes_with_configuration {
                publishers.push(IngestPublisher {
                    session_id: session.session_id.clone(),
                    participant_id: participant.participant_id.clone(),
                });
            }
        }
    }
    publishers
}

/// Leave every ingest configuration associated with `stage_arn` associated with no
/// stage.
///
/// A non-empty `IngestConfiguration$stageArn` is a REFERENCE to a stage of this service
/// ([`resolved_stage_association`] refuses one this store holds no stage for), so a
/// stage that goes away takes the references to it with it: once the stage is gone, no
/// configuration reports its ARN. The configurations themselves stay — they keep their
/// ingest keys and can be associated with another stage — so only the association is
/// dropped, through the one writer of the "no stage" reading
/// ([`IngestConfigurationRecord::clear_stage_association`]).
pub fn disassociate_stage_from_ingest_configurations(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
) {
    // A configuration associated with no stage is already where this leaves it, so the
    // "no stage" value is never treated as a stage to disassociate from.
    if stage_arn == NO_ASSOCIATED_STAGE_ARN {
        return;
    }
    for configuration in state.ingest_configurations.values_mut() {
        if configuration.stage_arn == stage_arn {
            configuration.clear_stage_association();
        }
    }
}

/// The region the request was processed in: the one its SigV4 credential scope names, so
/// a caller reads back the region it addressed and is answered from that region's view of
/// the account ([`in_request_region`]).
///
/// Falls back to [`DEFAULT_REGION`] for an unsigned request, and for a credential scope
/// whose region is not a token the model's ARN patterns accept (`[a-z0-9-]+`). It lives
/// here, beside that constant, because it is the rule for every ARN this service mints
/// and for every resource it reports, not just the stage ARNs of this slice.
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

/// The region element of an IVS ARN: the fourth `:`-field of the form every ARN shape in
/// this model pins (`arn:aws:ivs:<region>:<account>:<type>/<id>`), or `None` when the
/// string carries no such field.
pub fn arn_region(arn: &str) -> Option<&str> {
    arn.split(':').nth(3)
}

/// Whether the resource `arn` names is one a request processed in `region` is answered
/// about.
///
/// Each resource of this service is created through one region's endpoint and carries
/// that region in its ARN ([`request_region`] is what every mint fills the element
/// from), and an operation answers from the account view of the region ITS OWN request
/// was processed in, so an ARN naming another region names nothing it can report, change
/// or delete. Every record is keyed by its own ARN, so this predicate over the ARN a
/// request supplied is the region term of the lookup — the one place that term is
/// stated, used by the accessors below and by the listings' page order.
pub fn in_request_region(arn: &str, region: &str) -> bool {
    arn_region(arn) == Some(region)
}

/// `arn` itself when [`in_request_region`] holds for it, so a region-scoped lookup reads
/// `self.stages.get(request_scoped(arn, region)?)`.
fn request_scoped<'a>(arn: &'a str, region: &str) -> Option<&'a str> {
    in_request_region(arn, region).then_some(arn)
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
    // names a participant of this stage — stored or minted earlier in this same request.
    // Minted through the one place this service applies that rule, so the id a token
    // carries is unique by construction and not by probability.
    let (_, participant_id) = mint_unique_id(state, PARTICIPANT_ID_LEN, |state, candidate| {
        in_flight
            .iter()
            .any(|record| record.participant_id == candidate)
            || state
                .participant_tokens
                .get(stage_arn)
                .is_some_and(|records| {
                    records
                        .iter()
                        .any(|record| record.participant_id == candidate)
                })
    });

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

/// Total size of an `attributes` map, against [`MAX_ATTRIBUTES_BYTES`]: the sum of
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

/// One ingest credential: a fresh participant id, and the ingest key minted for it.
///
/// The ONE place an ingest credential is built, so the primary credential an ingest
/// configuration carries and each of its redundant ones are minted alike (the model
/// gives both the same two members, `participantId` and `streamKey`).
///
/// The participant id comes off the store's monotonic counter, so it is distinct from
/// every other id this store mints — including the participant ids of the stage's
/// tokens — by construction rather than by probability.
///
/// `None` only when the credential store minted no secret, which the caller answers
/// with [`stream_key_minting_failed`] rather than by reporting a key it made up.
pub fn mint_ingest_credential(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    credentials: &::emulator_server::CredentialStore,
    region: &str,
) -> Option<IngestCredential> {
    let participant_id = mint_id(state.next_sequence(), PARTICIPANT_ID_LEN);
    let stream_key = mint_stream_key(credentials, region, &participant_id)?;
    Some(IngestCredential {
        participant_id,
        stream_key,
    })
}

/// The ingest key a publisher broadcasts with, for the participant `participant_id` of
/// a configuration created in `region`.
///
/// The FORM is the model's: `StreamKey`'s `@pattern`
/// `^rt_[0-9]+_[a-z0-9-]+_[a-zA-Z0-9-]+_.+$` reads as
/// `rt_<account>_<region>_<participant id>_<secret>`, so the account this emulator
/// presents, the region the creating request was signed for and the participant the key
/// publishes as are its first three fields, and the secret is the last.
///
/// The secret is a credential handed to the caller, and nothing in a service may
/// construct a secret itself, so it is minted by the one component of this emulator that
/// generates secrets — the credential store — and then removed from it again, exactly as
/// [`mint_participant_token`] does: an ingest key authenticates a broadcast to a stage,
/// not a request to the emulator's API, so the emulator keeps no credential it would
/// accept as SigV4 and nothing accumulates per call.
///
/// `None` when the store minted no secret; see [`mint_ingest_credential`].
pub fn mint_stream_key(
    credentials: &::emulator_server::CredentialStore,
    region: &str,
    participant_id: &str,
) -> Option<String> {
    let minted = credentials.create(&::emulator_server::aws_credentials::AWS_ACCESS_KEY);
    let secret = minted
        .secret(::emulator_server::aws_credentials::SECRET_ACCESS_KEY)
        .map(str::to_string);
    // The ingest key is not a credential of this emulator; see above.
    let _ = credentials.delete(&minted.id);
    Some(format!(
        "rt_{}_{region}_{participant_id}_{}",
        ::engine::emulator_account_id(),
        secret?,
    ))
}

/// The opaque `nextToken` a `ListStages` page ends with: the creation sequence the
/// next page resumes after, inside the view that page reported.
///
/// The codec is `aws_smithy_types::base64`, the standard-alphabet encoder the wire
/// serializer already uses for every blob, and its output stays inside the model's
/// `PaginationToken` pattern `^[a-zA-Z0-9+/=_-]*$`.
///
/// `region` is the scope this listing pages within — "all stages in your account, in the
/// AWS region where the API request is processed" — so the position means nothing in
/// another region's page and is refused there ([`encode_after`]).
pub fn encode_page_token(region: &str, after_sequence: u64) -> String {
    encode_after(PAGE_TOKEN_PREFIX, &[region], NO_FILTERS, after_sequence)
}

/// The creation sequence a `nextToken` resumes after in `region`'s page, or `None` when
/// the token was not issued by [`encode_page_token`] for that same scope, or names a
/// position no page of this store ever ended at (`issued_through`, see [`decode_after`]).
pub fn decode_page_token(region: &str, issued_through: u64, token: &str) -> Option<u64> {
    decode_after(PAGE_TOKEN_PREFIX, &[region], NO_FILTERS, issued_through, token)
}

/// The opaque `nextToken` a `ListParticipants` page ends with: the join sequence of
/// the last participant it reported, which the next page resumes after.
///
/// Encoded exactly as [`encode_page_token`] is, under its own prefix
/// ([`PARTICIPANT_PAGE_TOKEN_PREFIX`]) and over the scope this listing pages within —
/// the region, the stage and the session whose participants the page held, and the
/// filter members the request carried, which narrow those participants further — so a
/// token one listing, one session or one filter issued names no position in another.
pub fn encode_participant_page_token(
    region: &str,
    stage_arn: &str,
    session_id: &str,
    filters: &[FilterTerm<'_>],
    after_sequence: u64,
) -> String {
    encode_after(
        PARTICIPANT_PAGE_TOKEN_PREFIX,
        &[region, stage_arn, session_id],
        filters,
        after_sequence,
    )
}

/// The join sequence a `ListParticipants` `nextToken` resumes after among the
/// participants of this stage session, or `None` when the token was not issued by
/// [`encode_participant_page_token`] for that same scope — a token of another listing, of
/// another session or of another filter selection included.
pub fn decode_participant_page_token(
    region: &str,
    stage_arn: &str,
    session_id: &str,
    filters: &[FilterTerm<'_>],
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        PARTICIPANT_PAGE_TOKEN_PREFIX,
        &[region, stage_arn, session_id],
        filters,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListParticipantEvents` page: the occurrence position of
/// the last event it reported, which the next page resumes after, under that listing's
/// own marker and over the scope it pages within — the region, the stage, the session and
/// the participant whose events the page held.
pub fn encode_participant_event_page_token(
    region: &str,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
    after_sequence: u64,
) -> String {
    encode_after(
        PARTICIPANT_EVENT_PAGE_TOKEN_PREFIX,
        &[region, stage_arn, session_id, participant_id],
        NO_FILTERS,
        after_sequence,
    )
}

/// The occurrence position a `ListParticipantEvents` `nextToken` resumes after among this
/// participant's events, or `None` when the token was not issued by
/// [`encode_participant_event_page_token`] for that same scope — a token of another
/// listing, another session or another participant included.
pub fn decode_participant_event_page_token(
    region: &str,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        PARTICIPANT_EVENT_PAGE_TOKEN_PREFIX,
        &[region, stage_arn, session_id, participant_id],
        NO_FILTERS,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListStageSessions` page: the start sequence the next
/// page of stage sessions resumes after, under that listing's own marker and over the
/// scope it pages within — the region and the stage whose sessions the page held.
pub fn encode_stage_session_page_token(
    region: &str,
    stage_arn: &str,
    after_sequence: u64,
) -> String {
    encode_after(
        STAGE_SESSION_PAGE_TOKEN_PREFIX,
        &[region, stage_arn],
        NO_FILTERS,
        after_sequence,
    )
}

/// The start sequence a stage-session `nextToken` resumes after among this stage's
/// sessions, or `None` when the token was not issued by
/// [`encode_stage_session_page_token`] for that same scope — a token of another listing or
/// of another stage included.
pub fn decode_stage_session_page_token(
    region: &str,
    stage_arn: &str,
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        STAGE_SESSION_PAGE_TOKEN_PREFIX,
        &[region, stage_arn],
        NO_FILTERS,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListParticipantReplicas` page: the start sequence the
/// next page of replicas resumes after, under that listing's own marker and over the
/// scope it pages within — the region, the source stage and the participant whose
/// replicas the page held.
pub fn encode_replica_page_token(
    region: &str,
    source_stage_arn: &str,
    participant_id: &str,
    after_sequence: u64,
) -> String {
    encode_after(
        REPLICA_PAGE_TOKEN_PREFIX,
        &[region, source_stage_arn, participant_id],
        NO_FILTERS,
        after_sequence,
    )
}

/// The start sequence a replica `nextToken` resumes after among this participant's
/// replicas, or `None` when the token was not issued by [`encode_replica_page_token`] for
/// that same scope — a token of another listing, another source stage or another
/// participant included.
pub fn decode_replica_page_token(
    region: &str,
    source_stage_arn: &str,
    participant_id: &str,
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        REPLICA_PAGE_TOKEN_PREFIX,
        &[region, source_stage_arn, participant_id],
        NO_FILTERS,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListStorageConfigurations` page: the creation sequence
/// the next page of storage configurations resumes after, under that listing's own
/// marker and over the region whose view the page reported.
pub fn encode_storage_configuration_page_token(region: &str, after_sequence: u64) -> String {
    encode_after(
        STORAGE_CONFIGURATION_PAGE_TOKEN_PREFIX,
        &[region],
        NO_FILTERS,
        after_sequence,
    )
}

/// The creation sequence a storage-configuration `nextToken` resumes after, or `None`
/// when the token was not issued by [`encode_storage_configuration_page_token`] for that
/// same scope — a token of another listing or of another region included.
pub fn decode_storage_configuration_page_token(
    region: &str,
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        STORAGE_CONFIGURATION_PAGE_TOKEN_PREFIX,
        &[region],
        NO_FILTERS,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListPublicKeys` page: the import sequence the next page
/// of public keys resumes after, under that listing's own marker and over the region
/// whose view the page reported.
pub fn encode_public_key_page_token(region: &str, after_sequence: u64) -> String {
    encode_after(
        PUBLIC_KEY_PAGE_TOKEN_PREFIX,
        &[region],
        NO_FILTERS,
        after_sequence,
    )
}

/// The import sequence a public-key `nextToken` resumes after, or `None` when the token
/// was not issued by [`encode_public_key_page_token`] for that same scope — a token of
/// another listing or of another region included.
pub fn decode_public_key_page_token(
    region: &str,
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        PUBLIC_KEY_PAGE_TOKEN_PREFIX,
        &[region],
        NO_FILTERS,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListEncoderConfigurations` page: the creation sequence
/// the next page of encoder configurations resumes after, under that listing's own
/// marker and over the region whose view the page reported.
pub fn encode_encoder_configuration_page_token(region: &str, after_sequence: u64) -> String {
    encode_after(
        ENCODER_CONFIGURATION_PAGE_TOKEN_PREFIX,
        &[region],
        NO_FILTERS,
        after_sequence,
    )
}

/// The creation sequence an encoder-configuration `nextToken` resumes after, or `None`
/// when the token was not issued by [`encode_encoder_configuration_page_token`] for that
/// same scope — a token of another listing or of another region included.
pub fn decode_encoder_configuration_page_token(
    region: &str,
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        ENCODER_CONFIGURATION_PAGE_TOKEN_PREFIX,
        &[region],
        NO_FILTERS,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListIngestConfigurations` page: the creation sequence the
/// next page of ingest configurations resumes after, under that listing's own marker and
/// over the scope the page reported — the region whose view it is, and the filter members
/// the request carried, which select which of that region's configurations the page
/// holds.
pub fn encode_ingest_configuration_page_token(
    region: &str,
    filters: &[FilterTerm<'_>],
    after_sequence: u64,
) -> String {
    encode_after(
        INGEST_CONFIGURATION_PAGE_TOKEN_PREFIX,
        &[region],
        filters,
        after_sequence,
    )
}

/// The creation sequence an ingest-configuration `nextToken` resumes after, or `None`
/// when the token was not issued by [`encode_ingest_configuration_page_token`] for that
/// same scope — a token of another listing, of another region or of another filter
/// selection included.
pub fn decode_ingest_configuration_page_token(
    region: &str,
    filters: &[FilterTerm<'_>],
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        INGEST_CONFIGURATION_PAGE_TOKEN_PREFIX,
        &[region],
        filters,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListCompositions` page: the start sequence the next page
/// of compositions resumes after, under that listing's own marker and over the scope the
/// page reported — the region whose view it is, and the filter members the request
/// carried, which select which of that region's compositions the page holds.
pub fn encode_composition_page_token(
    region: &str,
    filters: &[FilterTerm<'_>],
    after_sequence: u64,
) -> String {
    encode_after(
        COMPOSITION_PAGE_TOKEN_PREFIX,
        &[region],
        filters,
        after_sequence,
    )
}

/// The start sequence a composition `nextToken` resumes after, or `None` when the token
/// was not issued by [`encode_composition_page_token`] for that same scope — a token of
/// another listing, of another region or of another filter selection included.
pub fn decode_composition_page_token(
    region: &str,
    filters: &[FilterTerm<'_>],
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        COMPOSITION_PAGE_TOKEN_PREFIX,
        &[region],
        filters,
        issued_through,
        token,
    )
}

/// One filter member of a listing request: the member's name as the model declares it,
/// and the value this request gave it — `None` when the request carries that filter not
/// at all.
///
/// A filter member SELECTS which records a page holds, so it is a term of that page's
/// scope beside the identity terms, and every listing that has filter members hands its
/// whole set (in the order the model declares them) to the token codec below.
pub type FilterTerm<'a> = (&'a str, Option<&'a str>);

/// The filter set of a listing whose request has no filter member at all: its scope is
/// its identity terms alone.
const NO_FILTERS: &[FilterTerm<'static>] = &[];

/// The one page-token codec, so the listings cannot encode a position two ways.
///
/// A page position is an ordinal in ONE list of records, and which records a listing
/// pages over is decided by the terms of the request that select them: the identity terms
/// `scope` names, and the `filters` the request narrows them by. The token therefore
/// carries both beside the position, and [`decode_after`] reads a position out of it only
/// for the scope the resuming request itself names — so a token offered to any other
/// scope names no position in the records that page holds, whatever store-wide sequence it
/// carries.
fn encode_after(
    prefix: &str,
    scope: &[&str],
    filters: &[FilterTerm<'_>],
    after_sequence: u64,
) -> String {
    ::aws_smithy_types::base64::encode(format!(
        "{}{after_sequence}",
        scope_marker(prefix, scope, filters)
    ))
}

/// The position `token` names under `prefix` within the scope `scope` and `filters` name,
/// or `None` when it names none there.
///
/// A token names a position only when it is one [`encode_after`] really could have
/// issued, which is two requirements beyond the framing:
///
/// * the position is one this store has handed out — inside `issued_through`, the
///   sequences [`ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::issued_through`]
///   counts — since the encoder only ever writes the sequence of a record the store holds;
/// * the token is BYTE-IDENTICAL to what the encoder writes for that position, so a
///   payload spelt any other way than the encoder spells it (a sign, a leading zero, any
///   other base64 rendering of the same bytes) is a token this listing never issued.
///
/// Both are the same rule as the scope marker's: a `nextToken` resumes a listing only
/// when that listing issued it, and anything else is refused rather than read as a
/// position.
fn decode_after(
    prefix: &str,
    scope: &[&str],
    filters: &[FilterTerm<'_>],
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    let after_sequence: u64 = decoded
        .strip_prefix(&scope_marker(prefix, scope, filters))?
        .parse()
        .ok()?;
    ((1..=issued_through).contains(&after_sequence)
        && encode_after(prefix, scope, filters, after_sequence) == token)
        .then_some(after_sequence)
}

/// `prefix` followed by every term of `scope` and then every filter member, each written
/// as its byte length, a `/` and the term itself, so two different scopes cannot build the
/// same marker however the terms are punctuated — an ARN carries both `:` and `/`, which a
/// plain separator would let one term imitate.
///
/// A filter the request carries is written as its member name and then its value; one it
/// carries not at all is written as its member name alone, so "no filter" and "this
/// filter, with the empty value" are two scopes — the empty string is a value a filter
/// member can carry (`ListParticipants`' `filterByUserId` is `@length 0..128`).
fn scope_marker(prefix: &str, scope: &[&str], filters: &[FilterTerm<'_>]) -> String {
    let mut marker = String::from(prefix);
    for term in scope {
        push_scope_term(&mut marker, term);
    }
    for (member, value) in filters {
        push_scope_term(&mut marker, member);
        if let Some(value) = value {
            marker.push('=');
            push_scope_term(&mut marker, value);
        }
    }
    marker
}

/// Write one scope term onto `marker`: its byte length, a `/`, the term and a `:`.
fn push_scope_term(marker: &mut String, term: &str) {
    marker.push_str(&term.len().to_string());
    marker.push('/');
    marker.push_str(term);
    marker.push(':');
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

/// How this service states an import of public key material it already holds
/// (`ConflictException`, HTTP 409): one fixed sentence naming the member whose value is
/// already present, with nothing of the request in it.
///
/// It is the sentence the sibling IVS endpoint was recorded answering for the same
/// refusal on its own key import, with the resource named as this model's ARN spells
/// this one (`public-key`) — the same family rule [`resource_not_found_message`] follows.
pub const DUPLICATE_PUBLIC_KEY_MESSAGE: &str =
    "A public-key resource already exists with the given publicKeyMaterial";

/// How this service states a rejection of one member's VALUE for a reason the model
/// cannot express: it names the member and the constraint the value failed.
///
/// The form the sibling IVS endpoint was recorded answering for material it cannot
/// decode, so a refusal of the same member is worded the same across the IVS family
/// rather than in this endpoint's operation-prefixed form.
pub fn member_constraint_message(member: &str, constraint: &str) -> String {
    format!("Request member: {member} failed to satisfy the constraints: {constraint}")
}

/// How this service states a request whose member names a resource of this service that
/// this account holds none of: the operation, the dotted member path, the kind of
/// resource the member must name and the identifier the request sent, in the construction
/// every other member refusal of this service uses ([`violation_clause`]).
///
/// One rule for every member that carries a REFERENCE to another resource of this
/// service, so a dangling reference is refused in the same words whichever operation and
/// member carried it.
pub fn unresolved_reference_message(
    operation: &str,
    member: &str,
    resource: &str,
    identifier: &str,
) -> String {
    format!(
        "{operation}.{member} is required to name a {resource} this account holds, \
         found {identifier}"
    )
}

/// How this service states a `nextToken` it did not issue: the operation, the member
/// and the requirement, in the construction every other member refusal of this
/// service uses (`crate::state::violation_clause`).
///
/// One rule for every listing of this service — each pages over its own records under
/// its own token prefix — so a token from elsewhere is refused in the same words
/// whichever page it was offered to.
pub fn foreign_page_token_message(operation: &str) -> String {
    format!("{operation}.nextToken is required to be a pagination token issued by {operation}")
}

/// How this service states a request that supplied more than one of a set of members
/// only one of which may be given: it names the members the request actually carried,
/// then the whole set, in the construction every other member refusal of this service
/// uses (`<Operation>.<member> …`).
///
/// One rule for every operation whose model documents such a set — the four
/// `ListParticipants` filters say it in those words ("Only one of filterByUserId,
/// filterByPublished, filterByState, or filterByRecordingState can be provided per
/// request") — so two operations cannot word the same refusal two ways. `supplied`
/// holds the members the request carried, in the order `exclusive` lists them, and
/// `exclusive` the whole set.
pub fn mutually_exclusive_members_message(
    operation: &str,
    supplied: &[&str],
    exclusive: &[&str],
) -> String {
    format!(
        "{operation} is required to have at most one of {}, found {}",
        exclusive.join(", "),
        supplied.join(", "),
    )
}

/// How this service states a request that supplied the wrong number of a set of members
/// exactly one of which must be given: it names the members the request actually carried
/// (none, when it carried none) and then the whole set, in the construction every other
/// member refusal of this service uses (`<Operation>.<member> …`).
///
/// The companion of [`mutually_exclusive_members_message`], for a set whose members are
/// not merely exclusive but required as a choice — the place a destination publishes to,
/// say, where naming none describes no destination at all.
pub fn exactly_one_member_message(
    operation: &str,
    supplied: &[&str],
    exclusive: &[&str],
) -> String {
    format!(
        "{operation} is required to have exactly one of {}, found {}",
        exclusive.join(", "),
        if supplied.is_empty() {
            "none".to_string()
        } else {
            supplied.join(", ")
        },
    )
}

/// How this service states a LIST whose documented shape admits at most one element
/// naming each of a set of alternatives, but whose request named one alternative more
/// than once: it names the list member, the alternative that repeated and how many
/// elements named it, in the construction every other member refusal of this service
/// uses (`<Operation>.<member> …`).
///
/// The element-level companion of [`exactly_one_member_message`]: that one refuses an
/// element that names the wrong NUMBER of alternatives, this one refuses a list that
/// names one alternative too MANY times. `choice` is the repeated alternative, named as
/// the model names the member that selects it, and `found` the number of elements that
/// selected it.
pub fn at_most_one_element_per_choice_message(
    operation: &str,
    member: &str,
    choice: &str,
    found: usize,
) -> String {
    format!(
        "{operation}.{member} is required to have at most one element naming {choice}, \
         found {found}"
    )
}

/// What one start request asked for, as one comparable value.
///
/// `@idempotencyToken` makes a retry of ONE request answer with what the first call
/// created; it does not make one token stand for two different requests. So the token is
/// stored beside a fingerprint of the rest of the request — the stage, the effective
/// layout, the effective destination configurations in the order they were given, and the
/// tag set —
/// and a token offered again with a different fingerprint is refused instead of answered
/// with the wrong composition.
///
/// The tag pairs are sorted first: a `Tags` map has no request order, so two spellings of
/// one tag set must fingerprint alike. Nothing else here is a map, so the rendering is
/// stable.
pub fn composition_fingerprint(
    stage_arn: &str,
    layout: &LayoutConfiguration,
    destinations: &[DestinationConfiguration],
    tags: &HashMap<String, String>,
) -> String {
    let mut pairs: Vec<(&str, &str)> = tags
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    pairs.sort_unstable();
    format!("{stage_arn}|{layout:?}|{destinations:?}|{pairs:?}")
}

/// How this service states an `@idempotencyToken` offered again for a DIFFERENT request:
/// it names the operation, the member and the token, so the caller can see which token
/// already stands for something else.
///
/// AWS documents no text for `ConflictException` beyond what it means ("Updating or
/// deleting a resource can cause an inconsistent state"), so the sentence is built from
/// the request, the way every other refusal of this service is.
pub fn idempotency_token_reused_message(operation: &str, token: &str) -> String {
    format!(
        "{operation}.idempotencyToken is required to name the request it was first used \
         for, found {token} which already names a different request"
    )
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

/// The `ConflictException` (HTTP 409) a handler answers when the request would put the
/// store in a state contradicting one it already holds. The caller supplies the
/// sentence, built from the request by the rule beside the failure it states.
pub fn conflict<E>(
    variant: fn(ConflictException) -> E,
    message: impl Into<String>,
) -> Scaffolded<E> {
    let message = message.into();
    Scaffolded::stated(
        variant(ConflictException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// How this service states a stop of a composition that is not composing: fixed text
/// around the composition ARN the request named, and nothing else.
///
/// The model documents no text for `ConflictException` beyond what it means ("Updating or
/// deleting a resource can cause an inconsistent state"), so the wording comes from what
/// the service was recorded answering: one fixed sentence whose only variable part is the
/// ARN the request sent. The composition's own lifecycle state is NOT part of it — the
/// sentence a caller matches on cannot vary with a value this store keeps, so this takes
/// no state to interpolate.
pub fn composition_not_running_message(arn: &str) -> String {
    format!("Resource: {arn} is in a transitioning state")
}

/// How this service states a request to replicate a participant that is already
/// replicated into the stage it names: it names the participant and both stages, so the
/// caller can see which replication already holds.
///
/// AWS documents no text for `ConflictException` beyond what it means ("Updating or
/// deleting a resource can cause an inconsistent state"), so the sentence is built from
/// the request, the way every other refusal of this service is.
pub fn replication_already_active_message(
    participant_id: &str,
    source_stage_arn: &str,
    destination_stage_arn: &str,
) -> String {
    format!(
        "Resource: participant {participant_id} of {source_stage_arn} is already \
         replicated to {destination_stage_arn}"
    )
}

/// How this service states a request whose two ends of a replication are one stage: it
/// names the operation, the member that must differ and the stage the request sent, in the
/// construction every other member refusal of this service uses ([`violation_clause`]).
///
/// One rule for both operations that take the two ends of a replication: replication is
/// INTO another stage, and the model cannot say "these two members differ", so each
/// operation refuses it here in the same words.
pub fn same_stage_replication_message(operation: &str, destination_stage_arn: &str) -> String {
    format!(
        "{operation}.destinationStageArn is required to name a stage other than \
         sourceStageArn, found {destination_stage_arn}"
    )
}

/// How this service identifies ONE replication in a failure about it: the participant
/// being replicated and the two stages it is replicated between, which is what a request
/// names a replication by (this service stores no id for one).
///
/// Fed to [`resource_not_found`], so a stop of a replication this store does not hold
/// answers the one not-found sentence of this service ([`resource_not_found_message`])
/// rather than wording of its own.
pub fn replication_identifier(
    participant_id: &str,
    source_stage_arn: &str,
    destination_stage_arn: &str,
) -> String {
    format!(
        "participant {participant_id} of {source_stage_arn} replicated to \
         {destination_stage_arn}"
    )
}

/// How this service states a delete of a resource a composition is still using: it names
/// the resource, the composition using it and the state that composition is in, so the
/// caller can see what has to stop before the delete can succeed.
///
/// AWS documents no text for `ConflictException` beyond what it means ("Updating or
/// deleting a resource can cause an inconsistent state"), so the sentence is built from
/// the request and the store, the way every other refusal of this service is.
pub fn resource_in_use_by_composition_message(
    arn: &str,
    composition_arn: &str,
    composition_state: &str,
) -> String {
    format!("Resource: {arn} is in use: composition {composition_arn} is {composition_state}")
}

/// How this service states an update of a member that cannot be changed while a
/// recording is running: it names the stage, the participant whose recording is running
/// and the state that recording is in, so the caller can see what has to finish before
/// the update can succeed.
///
/// AWS documents no text for `ConflictException` beyond what it means ("Updating or
/// deleting a resource can cause an inconsistent state"), so the sentence is built from
/// the request and the store, the way every other refusal of this service is.
pub fn stage_recording_active_message(
    arn: &str,
    participant_id: &str,
    recording_state: &str,
) -> String {
    format!(
        "Resource: {arn} is in use: the recording of participant {participant_id} is \
         {recording_state}"
    )
}

/// How this service states a request to delete an ingest configuration a publisher is
/// broadcasting with: it names the configuration, the participant publishing with it
/// and the stage it publishes to, so the caller can see what is in use.
///
/// AWS documents no text for `ConflictException` beyond what it means ("Updating or
/// deleting a resource can cause an inconsistent state"), so the sentence is built from
/// the request and the store, the way every other refusal of this service is.
pub fn ingest_configuration_publishing_message(
    arn: &str,
    participant_id: &str,
    stage_arn: &str,
) -> String {
    format!(
        "Resource: {arn} is in use: participant {participant_id} is publishing with it to \
         {stage_arn}"
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

/// The tag-key prefix Amazon Web Services reserves for its OWN tags, on every resource
/// of every service.
///
/// Every `Tags`/`TagKeyList` member of this model defers to the AWS-wide tagging rules
/// ("See Best practices and strategies in Tagging AWS Resources and Tag Editor for
/// details, including restrictions that apply to tags and 'Tag naming limits and
/// requirements'; Amazon IVS has no constraints on tags beyond what is documented
/// there." — `smithy_query(query="doc", shape="TagResourceRequest", member="tags")`, and
/// the same sentence on `UntagResourceRequest$tagKeys`, `Stage$tags` and the create
/// requests). One of those restrictions is this prefix: it is reserved for AWS use, and a
/// caller can neither create, edit nor delete a tag key that claims it. `TagKey` carries
/// only `@length(1..128)`, so the rule is documented-but-unmodelled and belongs to the
/// handlers, exactly like the 1 KB `attributes` limit ([`MAX_ATTRIBUTES_BYTES`]).
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` claims the reserved namespace.
///
/// Compared case-INSENSITIVELY: what AWS reserves is the namespace, not one spelling of
/// it, so `AWS:` and `Aws:` claim it as much as `aws:` does.
///
/// The comparison is over BYTES, because a tag key is modelled `@length(1..128)` with no
/// `@pattern` and so may be any UTF-8 text: slicing it at a byte offset could fall inside
/// a multi-byte character and panic, while taking the bytes answers `false` for such a
/// key — which is correct, since a key whose first four bytes are not `aws:` does not
/// claim the namespace.
fn claims_reserved_tag_namespace(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}

/// Refuse a request that names a tag key in the reserved [`RESERVED_TAG_KEY_PREFIX`]
/// namespace. `Err` is the sentence to put on the `ValidationException`.
///
/// `member` is the dotted path of the member the keys arrived in (`tags` for a tagging or
/// create call, `tagKeys` for a removal), so the refusal reads in the construction every
/// other refusal of this service uses (`<Operation>.<member> …`, see
/// [`violation_clause`]). Called before anything is minted or written, so a refused call
/// leaves the store exactly as it was.
///
/// Every offending key is named, in sorted order: the keys of a `Tags` map have no request
/// order to report them in, so one refusal is worded one way whichever member carried them.
pub fn check_tag_keys_unreserved<'a>(
    operation: &str,
    member: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let mut reserved: Vec<&str> = keys
        .into_iter()
        .filter(|key| claims_reserved_tag_namespace(key))
        .collect();
    if reserved.is_empty() {
        return Ok(());
    }
    reserved.sort_unstable();
    reserved.dedup();
    Err(format!(
        "{operation}.{member} is required to name no key in the reserved \
         {RESERVED_TAG_KEY_PREFIX} namespace, found {}",
        reserved.join(", "),
    ))
}

/// Refuse a request whose `attributes` map is larger than the documented
/// [`MAX_ATTRIBUTES_BYTES`] total. `Err` is the sentence to put on the
/// `ValidationException`.
///
/// The limit is documented and carried by no modelled trait, so — like this service's
/// other documented-but-unmodelled rules ([`check_tag_count`],
/// [`check_tag_keys_unreserved`]) — it lives here once and every operation taking an
/// `attributes` map maps the sentence onto its own error. The measure is the one
/// [`attributes_size`] computes.
///
/// `member` is the dotted path of the member the map arrived in (`attributes` for a
/// request carrying one directly, `participantTokenConfigurations.<n>.member.attributes`
/// for one inside a list), so the refusal reads in the construction this service states
/// every constraint refusal with ([`violation_clause`]): the operation, the member path
/// and the bound it violated, with what the request actually sent.
///
/// The bound is a property of the request alone, so a caller runs this before any lookup
/// and before anything is minted or persisted: a request that is both oversized and names
/// no resource is answered as malformed, and a refused request leaves the store exactly as
/// it was.
pub fn check_attributes_size(
    operation: &str,
    member: &str,
    attributes: &HashMap<String, String>,
) -> Result<(), String> {
    let size = attributes_size(attributes);
    if size <= MAX_ATTRIBUTES_BYTES {
        return Ok(());
    }
    Err(format!(
        "{operation}.{member} is required to have a maximum size of {MAX_ATTRIBUTES_BYTES} \
         bytes, found {size}"
    ))
}

/// Merge an `attributes` map a request carries over the one a stored identity already
/// holds, and refuse the composition when it passes the documented
/// [`MAX_ATTRIBUTES_BYTES`] total. `Ok` is the composed map, ready to persist; `Err` is
/// the sentence to put on the `ValidationException`.
///
/// The documented bound belongs to the map this service STORES and REPORTS — every shape
/// of this service carrying an `attributes` map documents it, including the ones a reader
/// answers with — not only to the half a caller filled in. So a map this service composes
/// from more than one source is measured AFTER it is composed, against the same measure
/// [`check_attributes_size`] uses: no operation persists, and so no reader reports, an
/// `attributes` map past the bound its own field documents. An overlapping key takes the
/// overriding map's value, so a composition can total less than the sum of its halves.
///
/// Like [`check_attributes_size`], this neither opens nor writes anything: a caller runs
/// it before it persists, so a refused request leaves the store exactly as it was.
pub fn merge_attributes_within_bound(
    operation: &str,
    member: &str,
    held: &HashMap<String, String>,
    overriding: HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    let mut merged = held.clone();
    merged.extend(overriding);
    check_attributes_size(operation, member, &merged)?;
    Ok(merged)
}

/// The resource type an IVS ARN names (`stage`, `encoder-configuration`, …), or `None`
/// when the string carries no sixth `:`-field at all.
///
/// The type is that field up to its `/`, from the ARN form every ARN shape in this model
/// pins: `arn:aws:ivs:<region>:<account>:<type>/<id>`.
pub fn arn_resource_type(arn: &str) -> Option<&str> {
    arn.split(':').nth(5)?.split('/').next()
}

/// A kind of resource this store holds — the taggable-resource table of this service,
/// stated here once for both the read ([`resource_tags`]) and the write
/// ([`resource_tags_mut`]) side of the `/tags/{resourceArn}` surface.
///
/// A stage, an ingest configuration, an encoder configuration, a storage configuration,
/// a composition and a public key are the resource types this store can hold:
/// `CreateStage`, `CreateIngestConfiguration`, `CreateEncoderConfiguration`,
/// `CreateStorageConfiguration`, `StartComposition` and `ImportPublicKey` are the
/// operations of this
/// service that mint one, so every other resource type in its ARN shapes names nothing,
/// and a well-formed ARN naming one is answered exactly as an ARN naming a deleted stage
/// is — not found. Each remaining create is its own gap issue, and when one lands its
/// kind joins this enum: both functions below dispatch on it exhaustively, so the
/// compiler then asks each of them where that record lives and the two sides of the
/// surface cannot disagree about which resources are taggable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResourceKind {
    /// A stage, from `CreateStage`.
    Stage,
    /// A composition, from `StartComposition`.
    Composition,
    /// An ingest configuration, from `CreateIngestConfiguration`.
    IngestConfiguration,
    /// An encoder configuration, from `CreateEncoderConfiguration`.
    EncoderConfiguration,
    /// A storage configuration, from `CreateStorageConfiguration`.
    StorageConfiguration,
    /// A public key, from `ImportPublicKey`.
    PublicKey,
}

impl TaggedResourceKind {
    /// The kind `arn`'s resource-type field names ([`arn_resource_type`]), or `None`
    /// when that field names a resource type this store never holds — or the string
    /// carries no such field at all.
    pub fn from_arn(arn: &str) -> Option<Self> {
        match arn_resource_type(arn)? {
            "stage" => Some(Self::Stage),
            "composition" => Some(Self::Composition),
            "ingest-configuration" => Some(Self::IngestConfiguration),
            "encoder-configuration" => Some(Self::EncoderConfiguration),
            "storage-configuration" => Some(Self::StorageConfiguration),
            "public-key" => Some(Self::PublicKey),
            _ => None,
        }
    }
}

/// The tags attached to the resource `arn` names in `region`'s view, or `None` when that
/// view holds no such resource.
///
/// Which resource types can be named at all is [`TaggedResourceKind`]'s table, so this
/// reader and [`resource_tags_mut`] answer about exactly the same set of resources.
///
/// `region` is the region the tagging request was processed in, and the lookups go
/// through the region-scoped accessors, so a resource of another region is not found
/// here either: the tagging surface answers about exactly the resources that region's
/// own readers report ([`in_request_region`]).
pub fn resource_tags(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    arn: &str,
    region: &str,
) -> Option<HashMap<String, String>> {
    match TaggedResourceKind::from_arn(arn)? {
        TaggedResourceKind::Stage => state.stage_in(region, arn).map(|stage| stage.tags.clone()),
        TaggedResourceKind::Composition => state
            .composition_in(region, arn)
            // This reader holds the store immutably, so a composition the clock has
            // already reclaimed is skipped rather than dropped: the reclaim deadline is
            // computed from the instant the stop recorded, so a tag listing answers for
            // exactly the compositions `GetComposition` answers for at the same instant,
            // and the next call that can write clears the record.
            .filter(|composition| !composition.is_reclaimed(SystemTime::now()))
            .map(|composition| composition.tags.clone()),
        TaggedResourceKind::IngestConfiguration => state
            .ingest_configuration_in(region, arn)
            .map(|configuration| configuration.tags.clone()),
        TaggedResourceKind::EncoderConfiguration => state
            .encoder_configuration_in(region, arn)
            .map(|configuration| configuration.tags.clone()),
        TaggedResourceKind::StorageConfiguration => state
            .storage_configuration_in(region, arn)
            .map(|configuration| configuration.tags.clone()),
        TaggedResourceKind::PublicKey => state
            .public_key_in(region, arn)
            .map(|public_key| public_key.tags.clone()),
    }
}

/// The `tags` of the resource `arn` names in `region`'s view, to be changed in place.
/// `None` when that view holds no such resource; the resource types it can name are
/// [`TaggedResourceKind`]'s, the same table [`resource_tags`] reads.
pub fn resource_tags_mut<'a>(
    state: &'a mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    arn: &str,
    region: &str,
) -> Option<&'a mut HashMap<String, String>> {
    // A composition's own lifetime is the clock's: materialise what it already owed, so
    // a tag cannot be written onto a composition that no longer exists — and what the
    // clock owes a replication first of the two, because a replication ending can be what
    // left the composited stage empty and so earned the composition its stop.
    let now = SystemTime::now();
    materialize_due_replications(state, now);
    state.materialize_due_compositions(now);
    match TaggedResourceKind::from_arn(arn)? {
        TaggedResourceKind::Stage => state.stage_in_mut(region, arn).map(|stage| &mut stage.tags),
        TaggedResourceKind::Composition => state
            .composition_in_mut(region, arn)
            .map(|composition| &mut composition.tags),
        TaggedResourceKind::IngestConfiguration => state
            .ingest_configuration_in_mut(region, arn)
            .map(|configuration| &mut configuration.tags),
        TaggedResourceKind::EncoderConfiguration => state
            .encoder_configuration_in_mut(region, arn)
            .map(|configuration| &mut configuration.tags),
        TaggedResourceKind::StorageConfiguration => state
            .storage_configuration_in_mut(region, arn)
            .map(|configuration| &mut configuration.tags),
        TaggedResourceKind::PublicKey => state
            .public_key_in_mut(region, arn)
            .map(|public_key| &mut public_key.tags),
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

    /// The reserved namespace is the NAMESPACE, not one spelling of it, and a key that
    /// merely starts with the same letters is not in it. A key whose bytes straddle the
    /// four the comparison reads is not a special case: it simply does not claim the
    /// namespace.
    #[test]
    fn only_a_key_claiming_the_reserved_namespace_is_recognised() {
        let cases = [
            ("the reserved prefix in lower case", "aws:owner", true),
            (
                "a reserved key with a qualifier, as AWS's own tags carry",
                "aws:cloudformation:stack-name",
                true,
            ),
            ("the reserved prefix in upper case", "AWS:owner", true),
            ("the reserved prefix in mixed case", "Aws:owner", true),
            ("the prefix and nothing after it", "aws:", true),
            ("the same letters without the colon", "awsome", false),
            ("the letters as a whole key", "aws", false),
            ("the prefix in the middle of a key", "team-aws:owner", false),
            ("an ordinary key", "team", false),
            ("a key whose first bytes are multi-byte", "ä-aws:owner", false),
            ("a reserved key with a multi-byte tail", "aws:ö", true),
        ];
        for (case, key, expected) in cases {
            let actual = claims_reserved_tag_namespace(key);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A refusal names the operation, the member the keys arrived in and every offending
    /// key, in sorted order; a request naming none is accepted.
    #[test]
    fn a_reserved_key_refusal_names_the_operation_the_member_and_every_offending_key() {
        let accepted = check_tag_keys_unreserved("TagResource", "tags", ["team", "env"]);
        assert!(
            accepted.is_ok(),
            "case \"no reserved key\": expected acceptance, got {accepted:?}"
        );

        let cases: [(&str, &str, &str, Vec<&str>, &str); 3] = [
            (
                "one reserved key on TagResource",
                "TagResource",
                "tags",
                vec!["aws:owner"],
                "TagResource.tags is required to name no key in the reserved aws: namespace, \
                 found aws:owner",
            ),
            (
                "two reserved keys beside an ordinary one",
                "CreateStage",
                "tags",
                vec!["AWS:b", "team", "aws:a"],
                "CreateStage.tags is required to name no key in the reserved aws: namespace, \
                 found AWS:b, aws:a",
            ),
            (
                "a reserved key on UntagResource",
                "UntagResource",
                "tagKeys",
                vec!["aws:owner", "aws:owner"],
                "UntagResource.tagKeys is required to name no key in the reserved aws: \
                 namespace, found aws:owner",
            ),
        ];
        for (case, operation, member, keys, expected) in cases {
            let actual = check_tag_keys_unreserved(operation, member, keys);
            assert_eq!(
                actual.as_ref().err().map(String::as_str),
                Some(expected),
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}


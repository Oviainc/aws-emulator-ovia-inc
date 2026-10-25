// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime service.
//!
//! The stage lifecycle (`CreateStage`, `GetStage`, `ListStages`, `UpdateStage`,
//! `DeleteStage`), the token issuance that makes a stage usable
//! (`CreateParticipantToken`), the stage sessions a stage's participants are
//! grouped into (`ListParticipants`), the occurrences during one of those sessions
//! (`ListParticipantEvents`) and the compositions a stage is composed into
//! (`StartComposition`, `GetComposition`, `StopComposition`) all read and write the
//! records below, so the records, the id minting they share and the modelled defaults
//! they apply live here rather than in any one operation's file.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{validate, LengthTrait, PatternTrait};
use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    AutoParticipantRecordingConfiguration, Composition, ConflictException,
    CreateIngestConfigurationOutput, Destination, DestinationConfiguration, DestinationDetail,
    EncoderConfiguration, Event, GridConfiguration, IngestConfiguration, InternalServerException,
    LayoutConfiguration, ParticipantRecordingHlsConfiguration, ParticipantReplica,
    ParticipantSummary, ParticipantThumbnailConfiguration, ParticipantToken, PipConfiguration,
    RedundantIngestCredential, ResourceNotFoundException, S3StorageConfiguration,
    ServiceQuotaExceededException, Stage, StageEndpoints, StageSessionSummary, StageSummary,
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
pub fn resolved_recording_configuration(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    operation: &str,
    requested: Option<AutoParticipantRecordingConfiguration>,
) -> Result<AutoParticipantRecordingConfiguration, String> {
    let configuration = effective_recording_configuration(requested);
    let arn = configuration.storage_configuration_arn.as_str();
    if !arn.is_empty() && !state.storage_configurations.contains_key(arn) {
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
pub fn resolved_stage_association(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    operation: &str,
    requested: Option<String>,
) -> Result<String, String> {
    let stage_arn = requested.unwrap_or_else(|| NO_ASSOCIATED_STAGE_ARN.to_string());
    if stage_arn != NO_ASSOCIATED_STAGE_ARN && !state.stages.contains_key(&stage_arn) {
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
/// so those listings page by it.
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

/// Compositions this emulator keeps for one engine. Not an AWS quota — this emulator's
/// own resource guard on the one store `StartComposition` grows, set the same way
/// [`MAX_STAGES`] is and answered with the same modelled
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_COMPOSITIONS: usize = 1_000;

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
/// it left out.
///
/// The members the model carries a `@default` trait for (`omitStoppedVideo`, `gridGap`)
/// arrive already filled in by the framework's deserializer — they are not optional on
/// the Rust shape — so only the two the model documents a default for in prose are
/// applied here.
fn effective_grid(requested: GridConfiguration) -> GridConfiguration {
    GridConfiguration {
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
        ..requested
    }
}

/// The picture-in-picture layout `requested` describes, with the documented default for
/// every member it left out.
///
/// `pipWidth` and `pipHeight` are deliberately left absent when the request omits them:
/// the model documents that each is then "based on the size of the composition and the
/// aspect ratio of the participant's video" rather than taking a fixed value.
fn effective_pip(requested: PipConfiguration) -> PipConfiguration {
    PipConfiguration {
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
    pub user_id: Option<String>,
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
    /// fact ([`ingest_configuration_publishers`]) on every read.
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
            // `ACTIVE` exactly while a publisher is broadcasting with this
            // configuration, as the member's own doc states it.
            state: if ingest_configuration_publishers(store, self).is_empty() {
                NO_PUBLISHER_INGEST_CONFIGURATION_STATE.to_string()
            } else {
                PUBLISHING_INGEST_CONFIGURATION_STATE.to_string()
            },
            user_id: self.user_id.clone(),
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
}

/// One encoder configuration, as `CreateEncoderConfiguration` persisted it and
/// `GetEncoderConfiguration` reports it.
#[derive(Debug, Clone)]
pub struct EncoderConfigurationRecord {
    /// The encoder-configuration ARN, and the single key every lookup and delete uses.
    pub arn: String,
    pub name: Option<String>,
    /// The transcoding settings. Not optional: every encoder configuration has them,
    /// with the modelled defaults where the creating request supplied none
    /// ([`effective_video`]).
    pub video: Video,
    pub tags: HashMap<String, String>,
    /// Creation order, the stable key a listing of encoder configurations pages over
    /// (`ListEncoderConfigurations`, its own gap issue), set the way a stage's is.
    pub sequence: u64,
}

impl EncoderConfigurationRecord {
    /// The modelled `EncoderConfiguration` this record reports.
    pub fn to_encoder_configuration(&self) -> EncoderConfiguration {
        EncoderConfiguration {
            arn: self.arn.clone(),
            name: self.name.clone(),
            video: Some(self.video.clone()),
            // Always reported, empty map included: `tags` is how a caller reads the tags
            // back, exactly as on a stage.
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
    /// `Destination$configuration`, "configuration used to create this destination".
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
    /// already owed at `now`, so a reporting operation may call it. The last step a
    /// stopped composition owes — the deletion of the ephemeral resource — is not a state
    /// to write but a record to drop, which is [`Self::is_reclaimed`]'s question and
    /// `ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::materialize_due_compositions`'s
    /// to act on.
    pub fn materialize_due(&mut self, now: SystemTime) {
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
    /// and the stop recorded — [`Self::active_at`] for `STARTING` -> `ACTIVE`, one
    /// [`COMPOSITION_STOP_SPAN`] after [`Self::ended_at`] for `STOPPING` -> `STOPPED` —
    /// so this is what every reader asks, and the stored `state` is only where
    /// [`Self::materialize_due`] writes the answer down. A reader holding the store
    /// immutably therefore reaches the same answer as one that can write, instead of
    /// seeing a transition that is due but not yet recorded. A state the clock owes
    /// nothing from is reported as stored.
    pub fn state_at(&self, now: SystemTime) -> &str {
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
}

// ---------------------------------------------------------------------------
// Stopping a composition
//
// `StopComposition` ends a composition `StartComposition` started. A composition is
// EPHEMERAL: the model's `StartComposition` doc says it "is an ephemeral resource that
// exists after this operation returns successfully" and that the "Composition stops and
// the resource is deleted … When StopComposition is called". So a stop is the start of
// the composition's end — the clock owes it `STOPPED` one span later and the deletion of
// the resource one span after that, both materialised by
// [`CompositionRecord::materialize_due`] beside the `STARTING` -> `ACTIVE` transition a
// start owes, and neither ever slept on.
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
/// the `STOPPING` -> `STOPPED` transition, and one span after that the deletion of the
/// ephemeral resource.
///
/// AWS publishes no span for either step, so this is the emulator's own, inside the band
/// this tree fixes for a transition the clock owes and set the same way
/// [`COMPOSITION_ACTIVATION_SPAN`] is. It is deliberately NOT zero: a caller that reads
/// the composition back straight after stopping it must still observe `STOPPING`, the way
/// it would against AWS.
pub const COMPOSITION_STOP_SPAN: Duration = Duration::from_millis(500);

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
    /// Everything after that is the clock's ([`Self::materialize_due`]): `STOPPED` one
    /// [`COMPOSITION_STOP_SPAN`] later, and the deletion of the ephemeral resource one
    /// span after that.
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
    /// Due one [`COMPOSITION_STOP_SPAN`] after the stop finished, which is two spans after
    /// the stop began — computed from the recorded end time and [`Self::state_at`] rather
    /// than from the stored state, so a reader that holds the record immutably reaches the
    /// same answer as one that can write ([`Self::materialize_due`] is what removes it).
    pub fn is_reclaimed(&self, now: SystemTime) -> bool {
        self.state_at(now) == COMPOSITION_STATE_STOPPED
            && self
                .ended_at
                .is_some_and(|ended_at| now >= ended_at + COMPOSITION_STOP_SPAN * 2)
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
// applied — over the session [`ensure_active_session`] opens, and the reporting
// operations (`ListParticipants` and `ListStageSessions` today; `GetParticipant` and
// `GetStageSession`, each its own gap issue) answer from exactly what those
// recorded, never from a value invented for a join that never happened.
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

/// `ReplicationType` of a participant that is neither replicated to another stage nor
/// a replica from one.
///
/// The model documents it on `ParticipantSummary$replicationType`: "Indicates if the
/// participant has been replicated to another stage or is a replica from another
/// stage. Default: `NONE`." `StartParticipantReplication` (its own gap issue) is what
/// moves a participant off it.
pub const REPLICATION_TYPE_NONE: &str = "NONE";

/// One participant of one stage session, as the stage's data plane recorded it
/// joining ([`join_stage_session`]) and `ListParticipants` reports it.
#[derive(Debug, Clone)]
pub struct ParticipantRecord {
    /// The participant id it joined under: the one its participant token carries, or
    /// the one its ingest credential publishes as.
    pub participant_id: String,
    /// The customer-assigned name the token carried, when it carried one.
    pub user_id: Option<String>,
    /// The attributes the token carried. No member of `ParticipantSummary` reports
    /// them — `Participant$attributes` does, which `GetParticipant` reports (its own
    /// gap issue) — so they are recorded here where the join supplies them.
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
    /// [`REPLICATION_TYPE_NONE`] unless replication made it a `SOURCE` or a `REPLICA`.
    pub replication_type: String,
    /// The replication's own state (`ACTIVE` / `STOPPED`), absent while
    /// [`Self::replication_type`] is [`REPLICATION_TYPE_NONE`].
    pub replication_state: Option<String>,
    /// Where a replica was replicated from: both members are reported only "if
    /// `replicationType` is `REPLICA`", as their docs state.
    pub source_stage_arn: Option<String>,
    pub source_session_id: Option<String>,
    /// Whether the publisher reached the stage over redundant ingest.
    pub redundant_ingest: bool,
    /// The ingest configuration the publisher broadcast with, when it used one.
    pub ingest_configuration_arn: Option<String>,
    /// Join order within the session, and the stable key `ListParticipants` pages
    /// over — the same role [`StageRecord::sequence`] plays for `ListStages`.
    pub sequence: u64,
}

impl ParticipantRecord {
    /// The modelled `ParticipantSummary` this record reports in a `ListParticipants`
    /// page.
    pub fn to_summary(&self) -> ParticipantSummary {
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
            replication_type: Some(self.replication_type.clone()),
            replication_state: self.replication_state.clone(),
            source_stage_arn: self.source_stage_arn.clone(),
            source_session_id: self.source_session_id.clone(),
            redundant_ingest: self.redundant_ingest,
            ingest_configuration_arn: self.ingest_configuration_arn.clone(),
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

/// Record `event` as having just occurred in the session `session_id` of `stage_arn`.
///
/// The ONE place an event record is created, so every seam that observes an occurrence
/// writes the same representation of it: the instant is the one it occurred at and the
/// position is the session's next occurrence position, which is what keeps a page of
/// events in occurrence order.
///
/// `false` — nothing recorded — when this store holds no such session, or when the
/// session is at [`MAX_EVENTS_PER_SESSION`] events.
pub fn record_participant_event(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    event: ParticipantEvent,
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
        event_time: SystemTime::now(),
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
    /// Nothing disconnects an emulated publisher, so no clock reads it yet; it is
    /// recorded because it is what the caller asked replication to do.
    pub reconnect_window_seconds: i32,
    /// The attributes the replica participant carries in the destination stage: the
    /// source participant's own, with the starting request's merged over them — the rule
    /// `StartParticipantReplicationRequest$attributes` states ("These attributes are
    /// merged with any attributes set for this participant when creating the token. If
    /// there is overlap in keys, the values in these attributes are replaced."). No
    /// member of `ParticipantReplica` reports them; the participant operations
    /// (`GetParticipant` / `ListParticipants`, each its own gap issue) are what report a
    /// replica participant's attributes.
    pub attributes: HashMap<String, String>,
    /// When replication started. Reported by no member of `ParticipantReplica`, and
    /// recorded for the same reason [`StageRecord::created_at`] is.
    pub started_at: SystemTime,
    /// Start order, and the stable key `ListParticipantReplicas` pages over — the same
    /// role [`StageRecord::sequence`] plays for `ListStages`.
    pub sequence: u64,
}

impl ParticipantReplicaRecord {
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
}

/// Record the participant `joined` describes joining `stage_arn`, opening a session
/// for the stage when no session is active, and return the record the session now
/// holds for it.
///
/// The ONE place a participant record is created, so every operation that reports one
/// answers the same representation: a joining participant is
/// [`PARTICIPANT_STATE_CONNECTED`], its `firstJoinTime` is the instant it joined, it
/// is recorded with the recording state the recorder reports (or
/// [`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`]), and it is neither a replication
/// source nor a replica ([`REPLICATION_TYPE_NONE`]).
///
/// A participant the session already holds REJOINS it: the model documents
/// `firstJoinTime` as the first join and `published` as whether the participant ever
/// published, so the record keeps its id, its first join instant and its page
/// position, comes back to `CONNECTED`, and keeps `published` once set.
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
    // The session the stage is in, or a fresh one: "a stage session begins when the
    // first participant joins a stage".
    let session_id = ensure_active_session(state, stage_arn)?;
    // Read off before the join is applied, because applying it consumes `joined`.
    let participant_id = joined.participant_id.clone();
    let publishes = joined.published;

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
            if let Some(recording_state) = joined.recording_state.clone() {
                rejoined.recording_state = recording_state;
            }
            rejoined.redundant_ingest = joined.redundant_ingest;
            if joined.ingest_configuration_arn.is_some() {
                rejoined.ingest_configuration_arn = joined.ingest_configuration_arn.clone();
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
        None => new_participant_record(state, stage_arn, &session_id, joined)?,
    };

    // Both a first join and a rejoin are a participant joining the session, so each is
    // recorded; a publish is recorded only where the media actually begins, which a
    // rejoin of a participant already publishing is not.
    record_participant_event(
        state,
        stage_arn,
        &session_id,
        ParticipantEvent::new(EVENT_JOINED, &participant_id),
    );
    if publishes
        && !state
            .stage_session(stage_arn, &session_id)
            .is_some_and(|session| session.is_publishing(&participant_id))
    {
        record_participant_event(
            state,
            stage_arn,
            &session_id,
            ParticipantEvent::new(EVENT_PUBLISH_STARTED, &participant_id),
        );
    }
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
) -> Option<ParticipantRecord> {
    // Only a participant that is NEW to the session takes a page position, so the join
    // order of a session's participants stays contiguous.
    let sequence = state.next_sequence();
    let record = ParticipantRecord {
        participant_id: joined.participant_id,
        user_id: joined.user_id,
        attributes: joined.attributes,
        state: PARTICIPANT_STATE_CONNECTED.to_string(),
        first_join_time: SystemTime::now(),
        published: joined.published,
        recording_state: joined
            .recording_state
            .unwrap_or_else(|| NOT_RECORDED_PARTICIPANT_RECORDING_STATE.to_string()),
        replication_type: REPLICATION_TYPE_NONE.to_string(),
        replication_state: None,
        source_stage_arn: None,
        source_session_id: None,
        redundant_ingest: joined.redundant_ingest,
        ingest_configuration_arn: joined.ingest_configuration_arn,
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
/// `stage_arn`, and close that session when it was the last one in it.
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
/// `false` — nothing recorded — when this store holds no such stage, session or
/// participant, or when that participant has already left.
pub fn leave_stage_session(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
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
            session.end_time = Some(SystemTime::now());
        }
        emptied
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
            ParticipantEvent::new(EVENT_PUBLISH_STOPPED, participant_id),
        );
    }
    record_participant_event(
        state,
        stage_arn,
        session_id,
        ParticipantEvent::new(EVENT_LEFT, participant_id),
    );
    if session_ended {
        if let Some(stage) = state.stages.get_mut(stage_arn) {
            if stage.active_session_id.as_deref() == Some(session_id) {
                stage.active_session_id = None;
            }
        }
    }
    true
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

    /// Every stage in creation order — the order `ListStages` pages over.
    pub fn stages_in_order(&self) -> Vec<&StageRecord> {
        let mut stages: Vec<&StageRecord> = self.stages.values().collect();
        stages.sort_by_key(|record| record.sequence);
        stages
    }

    /// Every storage configuration in creation order — the order
    /// `ListStorageConfigurations` pages over.
    pub fn storage_configurations_in_order(&self) -> Vec<&StorageConfigurationRecord> {
        let mut configurations: Vec<&StorageConfigurationRecord> =
            self.storage_configurations.values().collect();
        configurations.sort_by_key(|record| record.sequence);
        configurations
    }

    /// Every session the stage `stage_arn` has had, in start order — the order
    /// `ListStageSessions` pages over, so a session opening or ending mid-pagination
    /// cannot shift a later page onto an already-returned one. A stage this store
    /// holds no session for (one that never had a participant, or one it does not hold
    /// at all) has none.
    pub fn stage_sessions_in_start_order(&self, stage_arn: &str) -> Vec<&StageSessionRecord> {
        let mut sessions: Vec<&StageSessionRecord> = self
            .stage_sessions
            .get(stage_arn)
            .map(|sessions| sessions.iter().collect())
            .unwrap_or_default();
        sessions.sort_by_key(|record| record.sequence);
        sessions
    }

    /// The session `session_id` of the stage `stage_arn`, or `None` when this store
    /// holds no such session for that stage.
    ///
    /// The lookup is by BOTH keys: a session id names a session of one stage, so a
    /// session of another stage is not found here even though every id this store
    /// mints is unique.
    pub fn stage_session(&self, stage_arn: &str, session_id: &str) -> Option<&StageSessionRecord> {
        self.stage_sessions
            .get(stage_arn)?
            .iter()
            .find(|session| session.session_id == session_id)
    }

    /// Write down every composition transition the clock has already made due, and drop
    /// the compositions whose ephemeral resource it has reclaimed.
    ///
    /// Each one was owed at [`CompositionRecord::active_at`], or one
    /// [`COMPOSITION_STOP_SPAN`] after a stop, whether or not anybody called, so this
    /// changes no composition that a caller reading at the same instant would have seen
    /// differently — which is why a reporting operation may call it. A stopped
    /// composition's record goes the same way: the model says the "Composition stops and
    /// the resource is deleted … When StopComposition is called", so the record is
    /// reclaimed rather than kept in a terminal state forever.
    pub fn materialize_due_compositions(&mut self, now: SystemTime) {
        for composition in self.compositions.values_mut() {
            composition.materialize_due(now);
        }
        self.compositions
            .retain(|_, composition| !composition.is_reclaimed(now));
    }

    /// The composition started under the `@idempotencyToken` `token`, if this store
    /// holds one.
    ///
    /// A composition is keyed by its own ARN, so the token is searched for rather than
    /// indexed: the store is bounded by [`MAX_COMPOSITIONS`], and a repeat of a start
    /// request is what this answers.
    pub fn composition_by_idempotency_token(&self, token: &str) -> Option<&CompositionRecord> {
        self.compositions
            .values()
            .find(|composition| composition.idempotency_token.as_deref() == Some(token))
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

// `CreateIngestConfigurationResponse$accessControlAllowOrigin` and its three siblings are
// deliberately left UNSET, and no value for them appears below: which web origin may read
// an answer off this listener is the emulator operator's decision, not this service's, and
// the framework that owns the listener is what answers it (it reflects an origin the
// operator allowed and grants no origin otherwise). A service that asserted an origin of
// its own would hand that grant to every caller of this emulator with nothing to turn it
// off. The model's `@cors` trait names no origin either
// (`smithy_query(query="service_info")` reports the trait carrying
// `additionalAllowedHeaders` and `additionalExposedHeaders` only), so there is no modelled
// origin to report. The six headers below are restrictions rather than grants, and are
// this service's to state.

/// Response headers a browser client may read off an answer of this endpoint.
///
/// The model's `@cors` trait lists them as `additionalExposedHeaders`
/// (`smithy_query(query="service_info")`): `x-amzn-errortype`, `x-amzn-requestid`,
/// `x-amzn-errormessage`, `x-amzn-trace-id`, `x-amz-apigw-id` and `date` — the trait's own
/// list, with the one name it repeats stated once. The first three are the three
/// `crate::service` writes on a failure, so what this header publishes is exactly what the
/// responses carry.
pub const ACCESS_CONTROL_EXPOSE_HEADERS: &str = "x-amzn-errortype, x-amzn-requestid, \
     x-amzn-errormessage, x-amzn-trace-id, x-amz-apigw-id, date";

/// How long an answer of this endpoint may be reused: not at all.
///
/// Every operation of this service reports state a later call can change — a stage's
/// `activeSessionId`, an ingest configuration's `state` — so no answer is a representation
/// a client or a proxy may serve again.
pub const CACHE_CONTROL: &str = "no-store";

/// What an answer of this endpoint may load: nothing. The answer is a JSON document, never
/// one a browser renders, so no source of any kind is allowed.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'";

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

/// The six headers above, each paired with the name the model's `@httpHeader` trait gives
/// it.
///
/// A SUCCESS does not go through this table: there they are modelled output members
/// ([`ResponseHeaderPolicy`]), so the protocol serializer writes each under the name its
/// own `@httpHeader` trait carries. The table is for the responses whose modelled members
/// never reach the wire — every failure of this service renders the one-member body
/// `crate::scaffolded::STATED_ERROR_SCHEMA` describes rather than the modelled error
/// shape, though the same members are on each of those shapes too — so `crate::service`
/// writes them onto the response itself, at the one place every response passes through.
pub const RESPONSE_HEADER_POLICY: [(&str, &str); 6] = [
    (
        "Access-Control-Expose-Headers",
        ACCESS_CONTROL_EXPOSE_HEADERS,
    ),
    ("Cache-Control", CACHE_CONTROL),
    ("Content-Security-Policy", CONTENT_SECURITY_POLICY),
    ("Strict-Transport-Security", STRICT_TRANSPORT_SECURITY),
    ("X-Content-Type-Options", X_CONTENT_TYPE_OPTIONS),
    ("X-Frame-Options", X_FRAME_OPTIONS),
];

/// An output shape of this service that binds the CORS and security response headers.
///
/// Four of this model's output shapes bind those members —
/// `CreateIngestConfigurationResponse`, `StartParticipantReplicationResponse`,
/// `StopParticipantReplicationResponse` and `UpdateIngestConfigurationResponse`
/// (`smithy_query(query="shape",shape="CreateIngestConfigurationResponse")`) — and the
/// values are a property of the ENDPOINT rather than of the resource any one of them
/// reports, so they are filled in here, in one place, instead of in each handler.
pub trait ResponseHeaderPolicy {
    /// This output with each restriction member the shape binds carrying the endpoint's
    /// value for it, so the protocol serializer writes all six on the response. The
    /// cross-origin grant member is left absent; see the note above
    /// [`ACCESS_CONTROL_EXPOSE_HEADERS`].
    #[must_use]
    fn with_response_header_policy(self) -> Self;
}

/// Implement [`ResponseHeaderPolicy`] for each output shape that binds those members, so
/// one shape cannot report a different policy from another.
macro_rules! impl_response_header_policy {
    ($($output:ty),+ $(,)?) => {
        $(impl ResponseHeaderPolicy for $output {
            fn with_response_header_policy(mut self) -> Self {
                self.access_control_expose_headers =
                    Some(ACCESS_CONTROL_EXPOSE_HEADERS.to_string());
                self.cache_control = Some(CACHE_CONTROL.to_string());
                self.content_security_policy = Some(CONTENT_SECURITY_POLICY.to_string());
                self.strict_transport_security = Some(STRICT_TRANSPORT_SECURITY.to_string());
                self.x_content_type_options = Some(X_CONTENT_TYPE_OPTIONS.to_string());
                self.x_frame_options = Some(X_FRAME_OPTIONS.to_string());
                self
            }
        })+
    };
}

// The two still-scaffolded operations of the four are implemented here as well: the policy
// is the shape's, not the handler's, so implementing one of those operations is a call to
// this trait rather than six more assignments.
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
/// The ONE place a session begins, so the id a stage reports and the
/// [`StageSessionRecord`] the reporting operations read are one session and not two.
pub fn ensure_active_session(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
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
            start_time: SystemTime::now(),
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
    /// The attributes the participant's identity carries, which a replica's attributes
    /// are merged over.
    pub attributes: HashMap<String, String>,
}

/// The participant `participant_id` of the stage `stage_arn`, or `None` when this
/// service issued no identity for that participant in that stage.
///
/// A participant id is "assigned by IVS and returned by `CreateParticipantToken`" (the
/// model's doc for the member, on every operation that takes one) or minted with an
/// ingest configuration's credentials, so the identities this store holds for a stage are
/// exactly its token records and the credentials of the ingest configurations associated
/// with it. The ONE lookup for "does this stage have this participant, and may it
/// publish", so no two operations can disagree about it.
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
            attributes: configuration.attributes.clone(),
        })
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
/// [`PARTICIPANT_STATE_CONNECTED`] and which reached the stage with this
/// configuration's credentials — recorded with the configuration's ARN, or publishing
/// as one of the participants its ingest credentials were minted for. Those are the
/// records the stage's data plane writes ([`join_stage_session`]) and
/// `ListParticipants` reports; a configuration associated with no stage
/// ([`NO_ASSOCIATED_STAGE_ARN`]) has nothing to publish to, so it has no publisher.
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
            let publishes_with_configuration = participant.ingest_configuration_arn.as_deref()
                == Some(configuration.arn.as_str())
                || configuration.mints_participant(&participant.participant_id);
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
/// next page resumes after.
///
/// The codec is `aws_smithy_types::base64`, the standard-alphabet encoder the wire
/// serializer already uses for every blob, and its output stays inside the model's
/// `PaginationToken` pattern `^[a-zA-Z0-9+/=_-]*$`.
pub fn encode_page_token(after_sequence: u64) -> String {
    encode_after(PAGE_TOKEN_PREFIX, after_sequence)
}

/// The creation sequence a `nextToken` resumes after, or `None` when the token was
/// not issued by [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Option<u64> {
    decode_after(PAGE_TOKEN_PREFIX, token)
}

/// The opaque `nextToken` a `ListParticipants` page ends with: the join sequence of
/// the last participant it reported, which the next page resumes after.
///
/// Encoded exactly as [`encode_page_token`] is, under its own prefix, so a token one
/// listing issued names no position in another
/// ([`PARTICIPANT_PAGE_TOKEN_PREFIX`]).
pub fn encode_participant_page_token(after_sequence: u64) -> String {
    encode_after(PARTICIPANT_PAGE_TOKEN_PREFIX, after_sequence)
}

/// The join sequence a `ListParticipants` `nextToken` resumes after, or `None` when
/// the token was not issued by [`encode_participant_page_token`] — a token from
/// another listing included.
pub fn decode_participant_page_token(token: &str) -> Option<u64> {
    decode_after(PARTICIPANT_PAGE_TOKEN_PREFIX, token)
}

/// [`encode_page_token`] for a `ListParticipantEvents` page: the occurrence position of
/// the last event it reported, which the next page resumes after, under that listing's
/// own marker.
pub fn encode_participant_event_page_token(after_sequence: u64) -> String {
    encode_after(PARTICIPANT_EVENT_PAGE_TOKEN_PREFIX, after_sequence)
}

/// The occurrence position a `ListParticipantEvents` `nextToken` resumes after, or
/// `None` when the token was not issued by [`encode_participant_event_page_token`] — a
/// token from another listing included.
pub fn decode_participant_event_page_token(token: &str) -> Option<u64> {
    decode_after(PARTICIPANT_EVENT_PAGE_TOKEN_PREFIX, token)
}

/// [`encode_page_token`] for a `ListStageSessions` page: the start sequence the next
/// page of stage sessions resumes after, under that listing's own marker.
pub fn encode_stage_session_page_token(after_sequence: u64) -> String {
    encode_after(STAGE_SESSION_PAGE_TOKEN_PREFIX, after_sequence)
}

/// The start sequence a stage-session `nextToken` resumes after, or `None` when the
/// token was not issued by [`encode_stage_session_page_token`] — a token from another
/// listing included.
pub fn decode_stage_session_page_token(token: &str) -> Option<u64> {
    decode_after(STAGE_SESSION_PAGE_TOKEN_PREFIX, token)
}

/// [`encode_page_token`] for a `ListParticipantReplicas` page: the start sequence the
/// next page of replicas resumes after, under that listing's own marker.
pub fn encode_replica_page_token(after_sequence: u64) -> String {
    encode_after(REPLICA_PAGE_TOKEN_PREFIX, after_sequence)
}

/// The start sequence a replica `nextToken` resumes after, or `None` when the token was
/// not issued by [`encode_replica_page_token`] — a token from another listing
/// included.
pub fn decode_replica_page_token(token: &str) -> Option<u64> {
    decode_after(REPLICA_PAGE_TOKEN_PREFIX, token)
}

/// [`encode_page_token`] for a `ListStorageConfigurations` page: the creation sequence
/// the next page of storage configurations resumes after, under that listing's own
/// marker.
pub fn encode_storage_configuration_page_token(after_sequence: u64) -> String {
    encode_after(STORAGE_CONFIGURATION_PAGE_TOKEN_PREFIX, after_sequence)
}

/// The creation sequence a storage-configuration `nextToken` resumes after, or `None`
/// when the token was not issued by [`encode_storage_configuration_page_token`] — a
/// token from another listing included.
pub fn decode_storage_configuration_page_token(token: &str) -> Option<u64> {
    decode_after(STORAGE_CONFIGURATION_PAGE_TOKEN_PREFIX, token)
}

/// The one page-token codec, so the listings cannot encode a position two ways.
fn encode_after(prefix: &str, after_sequence: u64) -> String {
    ::aws_smithy_types::base64::encode(format!("{prefix}{after_sequence}"))
}

/// The position `token` names under `prefix`, or `None` when it names none.
fn decode_after(prefix: &str, token: &str) -> Option<u64> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded.strip_prefix(prefix)?.parse().ok()
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

/// What one start request asked for, as one comparable value.
///
/// `@idempotencyToken` makes a retry of ONE request answer with what the first call
/// created; it does not make one token stand for two different requests. So the token is
/// stored beside a fingerprint of the rest of the request — the stage, the effective
/// layout, the destination configurations in the order they were given, and the tag set —
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

/// How this service states a stop of a composition that is not composing: it names the
/// composition and the state it is already in, so the caller can see why the stop is not
/// the next thing that can happen to it.
///
/// AWS documents no text for `ConflictException` beyond what it means ("Updating or
/// deleting a resource can cause an inconsistent state"), so the sentence is built from
/// the request and the stored state, the way every other refusal of this service is.
pub fn composition_not_running_message(arn: &str, state: &str) -> String {
    format!("Resource: {arn} is already {state}")
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
/// A stage, an ingest configuration, an encoder configuration, a storage configuration
/// and a composition are the resource types this store can hold: `CreateStage`,
/// `CreateIngestConfiguration`, `CreateEncoderConfiguration`,
/// `CreateStorageConfiguration` and `StartComposition` are the operations of this
/// service that mint one, so every other resource type in its ARN shapes names nothing,
/// and a well-formed ARN naming one is answered exactly as an ARN naming a deleted stage
/// is — not found. Each remaining create is its own gap issue, and when one lands its
/// record joins the match below.
pub fn resource_tags(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    arn: &str,
) -> Option<HashMap<String, String>> {
    match arn_resource_type(arn)? {
        "stage" => state.stages.get(arn).map(|stage| stage.tags.clone()),
        "composition" => state
            .compositions
            .get(arn)
            // This reader holds the store immutably, so a composition the clock has
            // already reclaimed is skipped rather than dropped: the reclaim deadline is
            // computed from the instant the stop recorded, so a tag listing answers for
            // exactly the compositions `GetComposition` answers for at the same instant,
            // and the next call that can write clears the record.
            .filter(|composition| !composition.is_reclaimed(SystemTime::now()))
            .map(|composition| composition.tags.clone()),
        "ingest-configuration" => state
            .ingest_configurations
            .get(arn)
            .map(|configuration| configuration.tags.clone()),
        "encoder-configuration" => state
            .encoder_configurations
            .get(arn)
            .map(|configuration| configuration.tags.clone()),
        "storage-configuration" => state
            .storage_configurations
            .get(arn)
            .map(|configuration| configuration.tags.clone()),
        _ => None,
    }
}

/// The `tags` of the resource `arn` names, to be changed in place. `None` when this
/// store holds no such resource; see [`resource_tags`].
pub fn resource_tags_mut<'a>(
    state: &'a mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    arn: &str,
) -> Option<&'a mut HashMap<String, String>> {
    // A composition's own lifetime is the clock's: materialise what it already owed, so
    // a tag cannot be written onto a composition that no longer exists.
    state.materialize_due_compositions(SystemTime::now());
    match arn_resource_type(arn)? {
        "stage" => state.stages.get_mut(arn).map(|stage| &mut stage.tags),
        "composition" => state
            .compositions
            .get_mut(arn)
            .map(|composition| &mut composition.tags),
        "ingest-configuration" => state
            .ingest_configurations
            .get_mut(arn)
            .map(|configuration| &mut configuration.tags),
        "encoder-configuration" => state
            .encoder_configurations
            .get_mut(arn)
            .map(|configuration| &mut configuration.tags),
        "storage-configuration" => state
            .storage_configurations
            .get_mut(arn)
            .map(|configuration| &mut configuration.tags),
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


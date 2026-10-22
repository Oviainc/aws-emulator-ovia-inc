// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime service.
//!
//! The stage lifecycle (`CreateStage`, `GetStage`, `ListStages`, `UpdateStage`,
//! `DeleteStage`) and the token issuance that makes a stage usable
//! (`CreateParticipantToken`) all read and write the records below, so the records,
//! the id minting they share and the modelled defaults they apply live here rather
//! than in any one operation's file.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AutoParticipantRecordingConfiguration, InternalServerException, ParticipantToken, Stage,
    StageEndpoints, StageSummary,
};

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
    /// starts one.
    pub active_session_id: Option<String>,
    pub tags: HashMap<String, String>,
    pub auto_participant_recording_configuration: Option<AutoParticipantRecordingConfiguration>,
    /// Summary of the stage's data-plane endpoints. Left `None`: this emulator serves
    /// no events/WHIP/RTMP endpoint for a stage, and reporting a host nothing listens
    /// on would be a fabricated value.
    pub endpoints: Option<StageEndpoints>,
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
            active_session_id: self.active_session_id.clone(),
            // Always reported, empty map included: `tags` is how a caller reads the
            // tags back, and an absent member is not the same answer as "none".
            tags: Some(self.tags.clone()),
            auto_participant_recording_configuration: self
                .auto_participant_recording_configuration
                .clone(),
            endpoints: self.endpoints.clone(),
        }
    }

    /// The modelled `StageSummary` this record reports in a `ListStages` page.
    pub fn to_summary(&self) -> StageSummary {
        StageSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            active_session_id: self.active_session_id.clone(),
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
            capabilities: Some(self.capabilities.clone()),
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
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        exception_message: Some(
            "the IVS RealTime store is not reachable from the engine serving this request"
                .to_string(),
        ),
        ..Default::default()
    }
}

/// The modelled 500 a handler answers with when the emulator's credential store minted
/// no secret for a participant token, so no token is issued rather than one this service
/// made up itself.
pub fn token_minting_failed() -> InternalServerException {
    InternalServerException {
        exception_message: Some(
            "the participant token could not be minted, so no token was issued".to_string(),
        ),
        ..Default::default()
    }
}

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

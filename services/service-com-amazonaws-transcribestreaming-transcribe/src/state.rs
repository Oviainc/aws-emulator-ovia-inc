// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-transcribestreaming-transcribe service.
//!
//! Every streaming session this service opens lives here, keyed by its 36-char
//! `SessionId`, and survives the request that created it: a
//! `StartMedicalScribeStream` is what a later `GetMedicalScribeStream` reports
//! from, and the response headers of a `Start*` are read back off the stored
//! session rather than echoed from the request headers.
//!
//! What a session records is only what the request actually carried: the settings
//! it negotiated, when it started and stopped, and how much audio arrived. NOTHING
//! here describes what was SAID — this emulator runs no speech recogniser, so it
//! holds no transcript, no word, no confidence and no entity, and the `Start*`
//! operations answer the recognition itself with the modeled
//! `ServiceUnavailableException` on the result stream rather than inventing one.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ClinicalNoteGenerationResult, MedicalScribeChannelDefinition, MedicalScribeEncryptionSettings,
    MedicalScribePostStreamAnalyticsResult, MedicalScribePostStreamAnalyticsSettings,
};

/// Bytes per audio sample for the 16-bit linear encodings this service accepts.
///
/// From the model's own size calculation for `AudioEvent$AudioChunk`:
/// `Duration (s) * Sample Rate (Hz) * Number of Channels * 2 (Bytes per Sample)`
/// (smithy_query(query="doc", shape="AudioEvent", member="AudioChunk")).
pub const BYTES_PER_SAMPLE: f64 = 2.0;

/// The most event-stream frames one request body may carry.
///
/// A CHOSEN capacity bound, not an AWS rule: the emulator buffers a whole request,
/// so the frame count is caller-chosen work and must be bounded before it is done.
/// Past it a `Start*` answers the modeled `LimitExceededException`, which the model
/// documents for a client that "has exceeded one of the Amazon Transcribe limits".
pub const MAX_EVENTS_PER_STREAM: usize = 1024;

/// The most audio one session may accumulate. A CHOSEN capacity bound, answered
/// with `LimitExceededException` — the error the model names for a stream that
/// "exceeds the maximum session duration".
pub const MAX_AUDIO_BYTES_PER_SESSION: u64 = 8 * 1024 * 1024;

/// The most streams that may be running at once.
///
/// A CHOSEN bound on concurrent work, which is what the modeled
/// `LimitExceededException` describes: "typically the concurrent stream service
/// quota".
pub const MAX_CONCURRENT_STREAMS: usize = 64;

/// The most sessions the store holds at once. Past it the least recently ended
/// session is evicted, so a long-lived process's memory cannot grow without bound;
/// when nothing can be evicted (every session is still running) the concurrency
/// bound above has already refused the request.
pub const MAX_STORED_SESSIONS: usize = 512;

/// Which operation opened a session. The result stream, and the settings a session
/// carries, differ per kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// `StartStreamTranscription`.
    Standard,
    /// `StartMedicalStreamTranscription`.
    Medical,
    /// `StartMedicalScribeStream`.
    MedicalScribe,
}

/// The real-time status of a streaming session, as
/// `MedicalScribeStreamStatus` declares it (IN_PROGRESS | PAUSED | FAILED |
/// COMPLETED).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStatus {
    InProgress,
    /// Declared because `MedicalScribeStreamStatus` declares it. No path in this
    /// slice produces it: the model documents no event that pauses a stream, so it
    /// exists for wire-value completeness and for whoever implements one.
    Paused,
    Failed,
    Completed,
}

impl StreamStatus {
    /// The wire value, one of `MedicalScribeStreamStatus`'s members.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Paused => "PAUSED",
            Self::Failed => "FAILED",
            Self::Completed => "COMPLETED",
        }
    }

    /// Whether the stream has stopped: a terminal session no longer conflicts with
    /// a new stream that reuses its session id.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Failed | Self::Completed)
    }
}

/// One audio chunk the session received, in arrival order. Measurements of the
/// request only: how long the chunk was and where in the stream it sat.
#[derive(Debug, Clone)]
pub struct AudioChunkRecord {
    /// 0-based arrival index.
    pub seq: u64,
    /// Length of the chunk in bytes.
    pub len: usize,
    /// Offset of the chunk's first sample from the start of the stream, in seconds.
    pub offset_secs: f64,
}

/// How long after the end of a HealthScribe session its post-stream analytics take
/// to reach a terminal status.
///
/// A CHOSEN span: the model documents that the analytics are `IN_PROGRESS` as soon
/// as the session ends and terminal once they finish
/// (smithy_query(query="doc", shape="ClinicalNoteGenerationResult", member="Status")),
/// but not how long that takes.
pub const POST_STREAM_ANALYTICS_DELAY: Duration = Duration::from_millis(500);

/// The HealthScribe configuration a `MedicalScribeConfigurationEvent` carried,
/// persisted because `GetMedicalScribeStream` has to report it.
#[derive(Debug, Clone, Default)]
pub struct MedicalScribeConfig {
    pub resource_access_role_arn: Option<String>,
    pub channel_definitions: Vec<MedicalScribeChannelDefinition>,
    pub encryption_settings: Option<MedicalScribeEncryptionSettings>,
    pub post_stream_analytics_settings: Option<MedicalScribePostStreamAnalyticsSettings>,
    pub post_stream_analytics_result: Option<MedicalScribePostStreamAnalyticsResult>,
    pub context_provided: bool,
    /// When the post-stream analytics this session started become due. `None` once
    /// they have been materialised, or while the session is still streaming.
    pub analytics_due_at: Option<SystemTime>,
}

/// One streaming session: the negotiated settings, the lifecycle, and the audio
/// accounting.
#[derive(Debug, Clone)]
pub struct StreamSession {
    pub session_id: String,
    pub kind: SessionKind,
    pub request_id: String,
    // Negotiated settings, stored at start and read back for the response headers.
    pub language_code: Option<String>,
    pub media_sample_rate_hertz: i32,
    pub media_encoding: String,
    pub vocabulary_name: Option<String>,
    pub vocabulary_filter_name: Option<String>,
    pub vocabulary_filter_method: Option<String>,
    pub language_model_name: Option<String>,
    pub identify_language: bool,
    pub identify_multiple_languages: bool,
    pub language_options: Option<String>,
    pub preferred_language: Option<String>,
    pub vocabulary_names: Option<String>,
    pub vocabulary_filter_names: Option<String>,
    pub show_speaker_label: bool,
    pub enable_channel_identification: bool,
    pub number_of_channels: Option<i32>,
    pub enable_partial_results_stabilization: bool,
    pub partial_results_stability: Option<String>,
    pub content_identification_type: Option<String>,
    pub content_redaction_type: Option<String>,
    pub pii_entity_types: Option<String>,
    pub transcript_format: Option<String>,
    pub session_resume_window: Option<i32>,
    /// Medical only: `Specialty` and `Type`.
    pub specialty: Option<String>,
    pub stream_type: Option<String>,
    // Lifecycle.
    pub status: StreamStatus,
    pub created_at: SystemTime,
    pub ended_at: Option<SystemTime>,
    // What the request carried, measured: how much audio arrived and how it was
    // sliced. Not what was said — nothing here recognises speech.
    pub bytes_received: u64,
    pub chunks: Vec<AudioChunkRecord>,
    /// HealthScribe only, from the `ConfigurationEvent`.
    pub scribe: Option<MedicalScribeConfig>,
}

impl StreamSession {
    /// A fresh session with the lifecycle fields set and no audio yet.
    pub fn new(
        session_id: String,
        kind: SessionKind,
        request_id: String,
        media_sample_rate_hertz: i32,
        media_encoding: String,
        created_at: SystemTime,
    ) -> Self {
        Self {
            session_id,
            kind,
            request_id,
            language_code: None,
            media_sample_rate_hertz,
            media_encoding,
            vocabulary_name: None,
            vocabulary_filter_name: None,
            vocabulary_filter_method: None,
            language_model_name: None,
            identify_language: false,
            identify_multiple_languages: false,
            language_options: None,
            preferred_language: None,
            vocabulary_names: None,
            vocabulary_filter_names: None,
            show_speaker_label: false,
            enable_channel_identification: false,
            number_of_channels: None,
            enable_partial_results_stabilization: false,
            partial_results_stability: None,
            content_identification_type: None,
            content_redaction_type: None,
            pii_entity_types: None,
            transcript_format: None,
            session_resume_window: None,
            specialty: None,
            stream_type: None,
            status: StreamStatus::InProgress,
            created_at,
            ended_at: None,
            bytes_received: 0,
            chunks: Vec::new(),
            scribe: None,
        }
    }

    /// How many channels the stored settings say the audio carries. Channel
    /// identification is what makes `NumberOfChannels` meaningful; without it the
    /// model documents a single channel.
    pub fn channels(&self) -> i32 {
        match (self.enable_channel_identification, self.number_of_channels) {
            (true, Some(n)) if n >= 1 => n,
            _ => 1,
        }
    }

    /// Bytes of audio per second of wall time at the negotiated settings.
    ///
    /// Never zero: the sample rate is refused outside the model's range before a
    /// session is built, and this is the divisor of every timing below.
    pub fn bytes_per_second(&self) -> f64 {
        let rate = f64::from(self.media_sample_rate_hertz) * f64::from(self.channels());
        (rate * BYTES_PER_SAMPLE).max(1.0)
    }

    /// Record one received audio chunk. The chunk's offset is computed from the
    /// bytes that arrived before it, so timings stay monotonic however the client
    /// slices its audio.
    ///
    /// `false` when the session has already taken [`MAX_AUDIO_BYTES_PER_SESSION`],
    /// in which case nothing is recorded: the caller answers
    /// `LimitExceededException` rather than letting a session grow without bound.
    pub fn record_chunk(&mut self, chunk: &[u8]) -> bool {
        let total = self.bytes_received.saturating_add(chunk.len() as u64);
        if total > MAX_AUDIO_BYTES_PER_SESSION {
            return false;
        }
        let seq = self.chunks.len() as u64;
        let offset_secs = self.bytes_received as f64 / self.bytes_per_second();
        self.chunks.push(AudioChunkRecord {
            seq,
            len: chunk.len(),
            offset_secs,
        });
        self.bytes_received = total;
        true
    }

    /// The end of the stored audio, in seconds from the start of the stream.
    pub fn duration_secs(&self) -> f64 {
        self.bytes_received as f64 / self.bytes_per_second()
    }

    /// Mark the stream stopped, if it has not stopped already.
    pub fn end(&mut self, status: StreamStatus, at: SystemTime) {
        if self.ended_at.is_none() {
            self.ended_at = Some(at);
        }
        self.status = status;
    }

    /// Start the post-stream analytics a HealthScribe session owes once its stream
    /// has ended: `IN_PROGRESS` immediately, per the model's own description of
    /// `ClinicalNoteGenerationResult$Status`, and due to reach a terminal status
    /// later.
    pub fn begin_post_stream_analytics(&mut self, now: SystemTime) {
        let Some(scribe) = self.scribe.as_mut() else {
            return;
        };
        if scribe.post_stream_analytics_settings.is_none() {
            return;
        }
        scribe.post_stream_analytics_result = Some(MedicalScribePostStreamAnalyticsResult {
            clinical_note_generation_result: Some(ClinicalNoteGenerationResult {
                clinical_note_output_location: None,
                transcript_output_location: None,
                status: Some("IN_PROGRESS".to_string()),
                failure_reason: None,
            }),
        });
        scribe.analytics_due_at = Some(now + POST_STREAM_ANALYTICS_DELAY);
    }

    /// Materialise the post-stream analytics the clock has already made due.
    ///
    /// Called from the reporting path as well, because the outcome was owed at the
    /// due instant whether or not anybody asked: nothing here depends on the read
    /// happening.
    ///
    /// The outcome is the model's own FAILED status with a `FailureReason` that says
    /// why: clinical-note generation summarises a transcript, and this emulator
    /// produces no transcript, so no note and no output location exists. Reporting
    /// `COMPLETED` with an S3 location would claim an object nothing wrote.
    pub fn settle_post_stream_analytics(&mut self, now: SystemTime) {
        let Some(scribe) = self.scribe.as_mut() else {
            return;
        };
        let Some(due) = scribe.analytics_due_at else {
            return;
        };
        if now < due {
            return;
        }
        scribe.post_stream_analytics_result = Some(MedicalScribePostStreamAnalyticsResult {
            clinical_note_generation_result: Some(ClinicalNoteGenerationResult {
                clinical_note_output_location: None,
                transcript_output_location: None,
                status: Some("FAILED".to_string()),
                failure_reason: Some(
                    "This emulator performs no speech recognition and no clinical note \
                     generation, so no note or transcript was written for this session."
                        .to_string(),
                ),
            }),
        });
        scribe.analytics_due_at = None;
    }
}

/// A freshly minted v4 UUID: the 36-char hyphenated form `SessionId`'s `@pattern`
/// requires, and the shape every identifier this service mints takes.
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// What claiming a session id for a new stream did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStart {
    /// The id was free (or named a stream that had already stopped), and the new
    /// session is stored.
    Started,
    /// The id named a stream of the SAME kind that was still running. That stream is
    /// terminated and the new session is NOT stored — the caller answers
    /// `ConflictException`, which the model documents as "A new stream started with
    /// the same session ID. The current stream has been terminated."
    TerminatedPrevious,
    /// The id named a still-running stream of ANOTHER kind, which is another
    /// operation's resource: it is left untouched and the new session is NOT stored.
    /// The caller answers `ConflictException` too — the id is taken — but nothing
    /// about the other stream changes.
    HeldByAnotherStream,
    /// The store is at capacity: [`MAX_CONCURRENT_STREAMS`] streams are already
    /// running, or [`MAX_STORED_SESSIONS`] sessions are held and none can be
    /// evicted. The caller answers `LimitExceededException`.
    AtCapacity,
}

/// Per-service state for com-amazonaws-transcribestreaming-transcribe. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTranscribestreamingTranscribeState {
    /// Every streaming session, keyed by its 36-char `SessionId`.
    pub sessions: HashMap<String, StreamSession>,
}

impl ComAmazonawsTranscribestreamingTranscribeState {
    /// How many stored streams have not stopped.
    pub fn running_streams(&self) -> usize {
        self.sessions
            .values()
            .filter(|session| !session.status.is_terminal())
            .count()
    }

    /// Claim a session id for `session`.
    ///
    /// A session id whose stream has already stopped is free to be streamed again,
    /// so a new stream replaces the stored one. A session id whose stream is still
    /// running is a conflict, and WHICH operation opened that stream decides what
    /// happens to it: a new stream of the SAME kind terminates it (the transition
    /// the model's `ConflictException` describes, observable through
    /// `GetMedicalScribeStream`), while a stream of another kind only finds the id
    /// taken — one API must not advance another API's resource, so that session is
    /// left exactly as it was. Either way the new session is refused and NOT stored,
    /// so the caller's error and the stored state agree.
    ///
    /// The store is bounded in both directions a caller can grow it: the number of
    /// streams running at once, and the number of sessions kept. Past the second
    /// bound the session that ended longest ago is evicted, so the map cannot grow
    /// for the process's lifetime.
    pub fn start_session(&mut self, session: StreamSession, now: SystemTime) -> SessionStart {
        let session_id = session.session_id.clone();
        let replacing = match self.sessions.get_mut(&session_id) {
            Some(existing) if !existing.status.is_terminal() => {
                if existing.kind != session.kind {
                    return SessionStart::HeldByAnotherStream;
                }
                existing.end(StreamStatus::Failed, now);
                return SessionStart::TerminatedPrevious;
            }
            Some(_) => true,
            None => false,
        };
        if !session.status.is_terminal() && self.running_streams() >= MAX_CONCURRENT_STREAMS {
            return SessionStart::AtCapacity;
        }
        if !replacing && self.sessions.len() >= MAX_STORED_SESSIONS {
            let evictable = self
                .sessions
                .values()
                .filter(|held| held.status.is_terminal())
                .min_by_key(|held| held.ended_at.unwrap_or(held.created_at))
                .map(|held| held.session_id.clone());
            match evictable {
                Some(id) => {
                    self.sessions.remove(&id);
                }
                None => return SessionStart::AtCapacity,
            }
        }
        self.sessions.insert(session_id, session);
        SessionStart::Started
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTranscribestreamingTranscribeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTranscribestreamingTranscribeState>)
}

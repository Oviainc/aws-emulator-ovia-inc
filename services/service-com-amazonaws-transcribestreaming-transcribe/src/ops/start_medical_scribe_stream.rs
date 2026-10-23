// SPDX-License-Identifier: CC0-1.0

//! @op StartMedicalScribeStream
//!
//! `POST /medical-scribe-stream -> 200`. Opens an Amazon Web Services HealthScribe
//! streaming session, persists the configuration its first event carries, accounts
//! for the audio that follows, and answers with the modeled
//! `MedicalScribeResultStream`.
//!
//! What that stream carries: this emulator has NO speech recogniser, so it reports
//! no transcript segment and invents none — the stream carries the modeled
//! `ServiceUnavailableException` member of `MedicalScribeResultStream`, framed as an
//! in-band exception with [`NO_RECOGNISER`] as its message. The SESSION, though, is
//! real and is the point of this operation: the
//! `ConfigurationEvent`'s `ResourceAccessRoleArn`, `ChannelDefinitions`,
//! `EncryptionSettings`, vocabulary settings and `PostStreamAnalyticsSettings` are
//! all stored on it, and the stream's `StreamStatus` moves to `COMPLETED` when the
//! client sends a `SessionControlEvent` of type `END_OF_SESSION` (and stays
//! `IN_PROGRESS` until then, because the model lets a session be resumed).
//!
//! Hand-authored for the reason `src/ops/SKIPPED_STUBS.md` records: a `@streaming`
//! event-stream input member and a `@streaming` union output. The framing lives in
//! [`super::types::event_stream`].
//!
//! ## Input
//!
//! * `SessionId` (len 36..36, UUID pattern, `@httpHeader`) — generated when absent
//! * `LanguageCode` (MedicalScribeLanguageCode: en-US, `@httpHeader`) **required**
//! * `MediaSampleRateHertz` (integer, range 16000..48000, `@httpHeader`) **required**
//! * `MediaEncoding` (MedicalScribeMediaEncoding: flac | ogg-opus | pcm,
//!   `@httpHeader`) **required**
//! * `InputStream` (`@httpPayload`, `@streaming` union of `AudioEvent`,
//!   `ConfigurationEvent`, `SessionControlEvent`) **required** — the first event must
//!   be a `ConfigurationEvent`
//!
//! ## Output
//!
//! `SessionId`, `RequestId`, `LanguageCode`, `MediaSampleRateHertz` and
//! `MediaEncoding` as response headers, and `ResultStream`
//! (`MedicalScribeResultStream`, `@httpPayload`, `@streaming`) as the body —
//! carrying the in-band `ServiceUnavailableException` described above.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400), `ConflictException` (HTTP 409),
//!   `InternalFailureException` (HTTP 500), `LimitExceededException` (HTTP 429),
//!   `ServiceUnavailableException` (HTTP 503).

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    new_uuid, MedicalScribeConfig, SessionKind, SessionStart, StreamSession, StreamStatus,
};
use aws_smithy_runtime_api::http::{Request, Response};
use smithy_server_runtime::{BoxFuture, OperationContext, ServerHttpProtocol, Service};
use std::sync::Arc;
use std::time::SystemTime;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-transcribestreaming-transcribe.json");

/// The URI the model binds this operation to
/// (smithy_query(query="operation", operation="StartMedicalScribeStream"):
/// `POST /medical-scribe-stream`). AWS publishes no WebSocket transport for this
/// operation, so there is no `-websocket` spelling of this path to claim.
pub const MODELED_PATH: &str = "/medical-scribe-stream";

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the
/// model declares for it.
#[smithy_macros::operation_error("com.amazonaws.transcribestreaming#StartMedicalScribeStream")]
#[derive(Debug)]
pub enum StartMedicalScribeStreamErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    InternalFailureException(InternalFailureException),
    LimitExceededException(LimitExceededException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// The `StartMedicalScribeStream` operation. Owns the shared engine.
pub struct StartMedicalScribeStream<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> StartMedicalScribeStream<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

fn bad_request(message: impl Into<String>) -> StartMedicalScribeStreamErrorModeled {
    StartMedicalScribeStreamErrorModeled::BadRequestException(BadRequestException {
        message: Some(message.into()),
    })
}

fn internal_failure(message: impl Into<String>) -> StartMedicalScribeStreamErrorModeled {
    StartMedicalScribeStreamErrorModeled::InternalFailureException(InternalFailureException {
        message: Some(message.into()),
    })
}

/// A limit of this emulator the request went past: the error the model documents
/// for a client that "has exceeded one of the Amazon Transcribe limits".
fn limit_exceeded(message: impl Into<String>) -> StartMedicalScribeStreamErrorModeled {
    StartMedicalScribeStreamErrorModeled::LimitExceededException(LimitExceededException {
        message: Some(message.into()),
    })
}

/// Word an event-stream decode failure as the modeled error for its kind.
fn from_event_stream(error: EventStreamError) -> StartMedicalScribeStreamErrorModeled {
    match error {
        EventStreamError::Malformed(message) => bad_request(message),
        EventStreamError::TooManyEvents(message) => limit_exceeded(message),
    }
}

/// The conflict the model words as "A new stream started with the same session ID.
/// The current stream has been terminated."
fn conflict(session_id: &str) -> StartMedicalScribeStreamErrorModeled {
    StartMedicalScribeStreamErrorModeled::ConflictException(ConflictException {
        message: Some(format!(
            "A new stream started with the same session ID {session_id}. \
             The current stream has been terminated."
        )),
    })
}

impl<Eng: Send + Sync + 'static> StartMedicalScribeStream<Eng> {
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
    ) -> Result<Response, StartMedicalScribeStreamErrorModeled> {
        let settings: StartMedicalScribeStreamSettings =
            settings_from_headers(protocol, req, &StartMedicalScribeStreamSettings::SCHEMA)
                .map_err(bad_request)?;
        let body = req
            .body()
            .bytes()
            .ok_or_else(|| bad_request("the request body is not available as bytes"))?;
        if body.is_empty() {
            return Err(bad_request(
                "1 validation error detected: Value null at 'inputStream' failed to satisfy \
                 constraint: Member must not be null",
            ));
        }
        let events = decode_event_stream(body).map_err(from_event_stream)?;
        // "Whether you are starting a new session or resuming an existing session,
        // your first event must be a MedicalScribeConfigurationEvent."
        if events.first().map(|event| event.event_type.as_str()) != Some("ConfigurationEvent") {
            return Err(bad_request(
                "the first event of a MedicalScribeInputStream must be a ConfigurationEvent",
            ));
        }

        let now = SystemTime::now();
        let session_id = settings.session_id.clone().unwrap_or_else(new_uuid);

        let mut session = StreamSession::new(
            session_id.clone(),
            SessionKind::MedicalScribe,
            new_uuid(),
            settings.media_sample_rate_hertz,
            settings.media_encoding.clone(),
            now,
        );
        session.language_code = Some(settings.language_code.clone());

        for (position, event) in events.iter().enumerate() {
            match event.event_type.as_str() {
                "ConfigurationEvent" if position == 0 => {
                    let config: MedicalScribeConfigurationEvent = event_payload(
                        protocol,
                        &event.payload,
                        &MedicalScribeConfigurationEvent::SCHEMA,
                    )
                    .map_err(bad_request)?;
                    apply_configuration(&mut session, &config);
                }
                "ConfigurationEvent" => {
                    return Err(bad_request(
                        "only the first event of a MedicalScribeInputStream may be a \
                         ConfigurationEvent",
                    ))
                }
                "AudioEvent" => {
                    // `MedicalScribeAudioEvent$AudioChunk` is the event's
                    // `@eventPayload`, so the frame payload IS the chunk.
                    if !session.record_chunk(&event.payload) {
                        return Err(limit_exceeded(format!(
                            "the stream carries more than the {} bytes of audio this \
                             emulator accepts in one session",
                            crate::state::MAX_AUDIO_BYTES_PER_SESSION
                        )));
                    }
                }
                "SessionControlEvent" => {
                    // The only modeled type is END_OF_SESSION, and the payload is
                    // validated against that enum before it is applied.
                    let control: MedicalScribeSessionControlEvent = event_payload(
                        protocol,
                        &event.payload,
                        &MedicalScribeSessionControlEvent::SCHEMA,
                    )
                    .map_err(bad_request)?;
                    if control.r#type == END_OF_SESSION {
                        session.end(StreamStatus::Completed, now);
                        session.begin_post_stream_analytics(now);
                    }
                }
                other => {
                    return Err(bad_request(format!(
                        "'{other}' is not a member of MedicalScribeInputStream; send \
                         ConfigurationEvent, AudioEvent or SessionControlEvent frames"
                    )))
                }
            }
        }

        // The store lock is held only to claim the session id and take a copy of
        // what was stored; the response is built outside it.
        let stored = {
            let state = crate::state::service_state(&*self.engine)
                .ok_or_else(|| internal_failure("this service's session store is unavailable"))?;
            let mut store = state.lock();
            match store.start_session(session, now) {
                // Nothing was stored: `start_session` terminates the stream that was
                // running (when it is one of this operation's own) or leaves another
                // operation's stream untouched, and returns without inserting, so the
                // 409 this answers with and the state the store holds agree.
                SessionStart::TerminatedPrevious | SessionStart::HeldByAnotherStream => {
                    return Err(conflict(&session_id))
                }
                SessionStart::AtCapacity => {
                    return Err(limit_exceeded(format!(
                        "this emulator is already running the {} concurrent streams it accepts",
                        crate::state::MAX_CONCURRENT_STREAMS
                    )))
                }
                SessionStart::Started => {}
            }
            // Read the session back out of the store: the response reports what was
            // persisted, never what the request headers said.
            store
                .sessions
                .get(&session_id)
                .cloned()
                .ok_or_else(|| internal_failure("the session was not persisted"))?
        };

        // The ONE event this result stream carries: the modeled
        // `ServiceUnavailableException` member, framed as an in-band exception. No
        // `TranscriptEvent` is emitted, because no recogniser ran.
        let frames = frame_event(
            protocol,
            "ServiceUnavailableException",
            true,
            &ServiceUnavailableException {
                message: Some(NO_RECOGNISER.to_string()),
            },
            &SCHEMA_SERVICE_UNAVAILABLE_EXCEPTION,
        )
        .map_err(|e| internal_failure(format!("the result stream could not be framed: {e}")))?;

        let stored = &stored;
        let headers = StartMedicalScribeStreamHeaders {
            session_id: Some(stored.session_id.clone()),
            request_id: Some(stored.request_id.clone()),
            language_code: stored.language_code.clone(),
            media_sample_rate_hertz: Some(stored.media_sample_rate_hertz),
            media_encoding: Some(stored.media_encoding.clone()),
        };
        event_stream_response(
            protocol,
            "StartMedicalScribeStream",
            &headers,
            &StartMedicalScribeStreamHeaders::SCHEMA,
            frames,
        )
        .map_err(|e| internal_failure(format!("the response could not be serialized: {e}")))
    }
}

/// Persist everything a `MedicalScribeConfigurationEvent` carries onto the session,
/// because `GetMedicalScribeStream` reports all of it.
fn apply_configuration(session: &mut StreamSession, config: &MedicalScribeConfigurationEvent) {
    session.vocabulary_name = config.vocabulary_name.clone();
    session.vocabulary_filter_name = config.vocabulary_filter_name.clone();
    session.vocabulary_filter_method = config.vocabulary_filter_method.clone();
    session.scribe = Some(MedicalScribeConfig {
        resource_access_role_arn: Some(config.resource_access_role_arn.clone()),
        channel_definitions: config.channel_definitions.clone().unwrap_or_default(),
        encryption_settings: config.encryption_settings.clone(),
        post_stream_analytics_settings: Some(config.post_stream_analytics_settings.clone()),
        post_stream_analytics_result: None,
        context_provided: config.medical_scribe_context.is_some(),
        analytics_due_at: None,
    });
}

impl<Eng: Send + Sync + 'static> EventStreamOperation for StartMedicalScribeStream<Eng> {
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            let response = match self.respond(protocol, req) {
                Ok(response) => response,
                Err(error) => error_response(protocol, &error),
            };
            // One access record per request, so a refused stream is diagnosable.
            // No request payload and no credential reaches it.
            tracing::debug!(
                operation = "StartMedicalScribeStream",
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served an event-stream request"
            );
            response
        })
    }
}

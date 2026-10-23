// SPDX-License-Identifier: CC0-1.0

//! @op StartMedicalStreamTranscription
//!
//! `POST /medical-stream-transcription -> 200`. The medical sibling of
//! `StartStreamTranscription`: the same session store, the same modeled refusals and
//! the same answer to the recognition itself — this emulator has no speech
//! recogniser, so the `MedicalTranscriptResultStream` carries the modeled
//! `ServiceUnavailableException` member framed as an in-band exception
//! ([`NO_RECOGNISER`]) rather than an invented transcript. The medical-specific
//! required settings are stored and reported back from the session.
//!
//! Hand-authored for the reason `src/ops/SKIPPED_STUBS.md` records: a `@streaming`
//! event-stream input member and a `@streaming` union output. The framing lives in
//! [`super::types::event_stream`].
//!
//! ## Input (every member an `@httpHeader` except the payload)
//!
//! * `LanguageCode` (LanguageCode) **required**
//! * `MediaSampleRateHertz` (integer, range 8000..48000) **required**
//! * `MediaEncoding` (MediaEncoding: flac | g711-alaw | g711-ulaw | g729 | ogg-opus
//!   | pcm) **required**
//! * `Specialty` (CARDIOLOGY | NEUROLOGY | ONCOLOGY | PRIMARYCARE | RADIOLOGY |
//!   UROLOGY) **required**
//! * `Type` (CONVERSATION | DICTATION) **required**
//! * `ShowSpeakerLabel`, `EnableChannelIdentification`, `NumberOfChannels` (range 2..)
//! * `VocabularyName`, `ContentIdentificationType` (PHI)
//! * `SessionId` (len 36..36, UUID pattern) — generated when absent
//! * `AudioStream` (`@httpPayload`, `@streaming`) **required**
//!
//! ## Output
//!
//! Every input member above echoed as a response header, plus `RequestId`
//! (`x-amzn-request-id`), and `TranscriptResultStream`
//! (`MedicalTranscriptResultStream`, `@httpPayload`, `@streaming`) as the body —
//! carrying the in-band `ServiceUnavailableException` described above.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400), `ConflictException` (HTTP 409),
//!   `InternalFailureException` (HTTP 500), `LimitExceededException` (HTTP 429),
//!   `ServiceUnavailableException` (HTTP 503).
//!
//! ## The WebSocket transport
//!
//! Like `StartStreamTranscription`, this operation is also reachable at
//! [`WEBSOCKET_PATH`] — its modeled URI plus `-websocket` — with the input above
//! carried as query parameters (`sample-rate`, `media-encoding`, `specialty`,
//! `type`, …) instead of `x-amzn-transcribe-*` headers. The parameters are read back
//! onto the same modeled shape and validated against the same constraints, and the
//! stream itself is refused with the modeled `ServiceUnavailableException` — see
//! [`NO_WEBSOCKET_TRANSPORT`].

#[allow(unused_imports)]
use super::types::*;
use crate::state::{new_uuid, SessionKind, SessionStart, StreamSession, StreamStatus};
use aws_smithy_runtime_api::http::{Request, Response};
use smithy_server_runtime::{BoxFuture, OperationContext, ServerHttpProtocol, Service};
use std::sync::Arc;
use std::time::SystemTime;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-transcribestreaming-transcribe.json");

/// The URI the model binds this operation to
/// (smithy_query(query="operation", operation="StartMedicalStreamTranscription"):
/// `POST /medical-stream-transcription`). Its WebSocket transport is this path plus
/// [`WEBSOCKET_PATH_SUFFIX`].
pub const MODELED_PATH: &str = "/medical-stream-transcription";

/// Where the WebSocket transport of this operation is reached.
pub const WEBSOCKET_PATH: &str = "/medical-stream-transcription-websocket";

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the
/// model declares for it.
#[smithy_macros::operation_error(
    "com.amazonaws.transcribestreaming#StartMedicalStreamTranscription"
)]
#[derive(Debug)]
pub enum StartMedicalStreamTranscriptionErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    InternalFailureException(InternalFailureException),
    LimitExceededException(LimitExceededException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// The `StartMedicalStreamTranscription` operation. Owns the shared engine.
pub struct StartMedicalStreamTranscription<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> StartMedicalStreamTranscription<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

fn bad_request(message: impl Into<String>) -> StartMedicalStreamTranscriptionErrorModeled {
    StartMedicalStreamTranscriptionErrorModeled::BadRequestException(BadRequestException {
        message: Some(message.into()),
    })
}

fn internal_failure(message: impl Into<String>) -> StartMedicalStreamTranscriptionErrorModeled {
    StartMedicalStreamTranscriptionErrorModeled::InternalFailureException(
        InternalFailureException {
            message: Some(message.into()),
        },
    )
}

/// A limit of this emulator the request went past: the error the model documents
/// for a client that "has exceeded one of the Amazon Transcribe limits".
fn limit_exceeded(message: impl Into<String>) -> StartMedicalStreamTranscriptionErrorModeled {
    StartMedicalStreamTranscriptionErrorModeled::LimitExceededException(LimitExceededException {
        message: Some(message.into()),
    })
}

/// Word an event-stream decode failure as the modeled error for its kind.
fn from_event_stream(error: EventStreamError) -> StartMedicalStreamTranscriptionErrorModeled {
    match error {
        EventStreamError::Malformed(message) => bad_request(message),
        EventStreamError::TooManyEvents(message) => limit_exceeded(message),
    }
}

/// The conflict the model words as "A new stream started with the same session ID.
/// The current stream has been terminated."
fn conflict(session_id: &str) -> StartMedicalStreamTranscriptionErrorModeled {
    StartMedicalStreamTranscriptionErrorModeled::ConflictException(ConflictException {
        message: Some(format!(
            "A new stream started with the same session ID {session_id}. \
             The current stream has been terminated."
        )),
    })
}

/// A stream this service cannot carry: the modeled error the model declares for
/// exactly that, HTTP 503.
fn service_unavailable(message: impl Into<String>) -> StartMedicalStreamTranscriptionErrorModeled {
    StartMedicalStreamTranscriptionErrorModeled::ServiceUnavailableException(
        ServiceUnavailableException {
            message: Some(message.into()),
        },
    )
}

/// The rules this operation's input must satisfy ACROSS its members. This operation
/// binds the channel-identification pair that `StartStreamTranscription` binds, with
/// the same model-stated rule, so it applies the same check; it binds no
/// partial-results stabilization members, so there is nothing else to check here.
fn cross_member_refusal(settings: &StartMedicalStreamTranscriptionSettings) -> Option<String> {
    channel_identification_refusal(
        settings.enable_channel_identification,
        settings.number_of_channels,
    )
}

/// Answer the WebSocket transport of this operation: the input off the query string,
/// validated as the header transport's is, and the stream refused with the modeled
/// `ServiceUnavailableException` ([`NO_WEBSOCKET_TRANSPORT`] says why). Nothing is
/// stored: no stream was carried, so no session exists.
fn websocket_refusal(
    protocol: &dyn ServerHttpProtocol,
    req: &Request,
) -> StartMedicalStreamTranscriptionErrorModeled {
    let settings: StartMedicalStreamTranscriptionSettings = match settings_from_query(
        protocol,
        req,
        &StartMedicalStreamTranscriptionSettings::SCHEMA,
    ) {
        Ok(settings) => settings,
        Err(message) => return bad_request(message),
    };
    match cross_member_refusal(&settings) {
        Some(message) => bad_request(message),
        None => service_unavailable(NO_WEBSOCKET_TRANSPORT),
    }
}

impl<Eng: Send + Sync + 'static> StartMedicalStreamTranscription<Eng> {
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
    ) -> Result<Response, StartMedicalStreamTranscriptionErrorModeled> {
        let settings: StartMedicalStreamTranscriptionSettings = settings_from_headers(
            protocol,
            req,
            &StartMedicalStreamTranscriptionSettings::SCHEMA,
        )
        .map_err(bad_request)?;
        if let Some(message) = cross_member_refusal(&settings) {
            return Err(bad_request(message));
        }
        let body = req
            .body()
            .bytes()
            .ok_or_else(|| bad_request("the request body is not available as bytes"))?;
        if body.is_empty() {
            return Err(bad_request(
                "1 validation error detected: Value null at 'audioStream' failed to satisfy \
                 constraint: Member must not be null",
            ));
        }
        let events = decode_event_stream(body).map_err(from_event_stream)?;

        let now = SystemTime::now();
        let session_id = settings.session_id.clone().unwrap_or_else(new_uuid);

        let mut session = StreamSession::new(
            session_id.clone(),
            SessionKind::Medical,
            new_uuid(),
            settings.media_sample_rate_hertz,
            settings.media_encoding.clone(),
            now,
        );
        session.language_code = Some(settings.language_code.clone());
        session.vocabulary_name = settings.vocabulary_name.clone();
        session.specialty = Some(settings.specialty.clone());
        session.stream_type = Some(settings.r#type.clone());
        session.show_speaker_label = settings.show_speaker_label;
        session.enable_channel_identification = settings.enable_channel_identification;
        session.number_of_channels = settings.number_of_channels;
        session.content_identification_type = settings.content_identification_type.clone();

        for event in &events {
            match event.event_type.as_str() {
                "AudioEvent" => {
                    // `AudioEvent$AudioChunk` is the event's `@eventPayload`, so the
                    // frame payload IS the chunk.
                    if !session.record_chunk(&event.payload) {
                        return Err(limit_exceeded(format!(
                            "the stream carries more than the {} bytes of audio this \
                             emulator accepts in one session",
                            crate::state::MAX_AUDIO_BYTES_PER_SESSION
                        )));
                    }
                }
                // The other `AudioStream` member carries the channel definitions
                // and post-call analytics settings of a CALL ANALYTICS stream,
                // which this operation is not. It is a modeled member of the union
                // this operation binds, so it is accepted rather than refused;
                // nothing here consumes it.
                "ConfigurationEvent" => {}
                other => {
                    return Err(bad_request(format!(
                        "'{other}' is not a member of AudioStream; send AudioEvent frames"
                    )))
                }
            }
        }
        session.end(StreamStatus::Completed, now);

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
        let headers = StartMedicalStreamTranscriptionHeaders {
            request_id: Some(stored.request_id.clone()),
            language_code: stored.language_code.clone(),
            media_sample_rate_hertz: Some(stored.media_sample_rate_hertz),
            media_encoding: Some(stored.media_encoding.clone()),
            vocabulary_name: stored.vocabulary_name.clone(),
            specialty: stored.specialty.clone(),
            r#type: stored.stream_type.clone(),
            show_speaker_label: stored.show_speaker_label,
            session_id: Some(stored.session_id.clone()),
            enable_channel_identification: stored.enable_channel_identification,
            number_of_channels: stored.number_of_channels,
            content_identification_type: stored.content_identification_type.clone(),
        };
        event_stream_response(
            protocol,
            "StartMedicalStreamTranscription",
            &headers,
            &StartMedicalStreamTranscriptionHeaders::SCHEMA,
            frames,
        )
        .map_err(|e| internal_failure(format!("the response could not be serialized: {e}")))
    }
}

impl<Eng: Send + Sync + 'static> EventStreamOperation for StartMedicalStreamTranscription<Eng> {
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            // The transport decides where the input is read from: query parameters on
            // the WebSocket spelling, `x-amzn-transcribe-*` headers on the modeled
            // HTTP/2 one.
            let response = if is_websocket_transport_of(req.uri(), MODELED_PATH) {
                error_response(protocol, &websocket_refusal(protocol, req))
            } else {
                match self.respond(protocol, req) {
                    Ok(response) => response,
                    Err(error) => error_response(protocol, &error),
                }
            };
            // One access record per request, so a refused stream is diagnosable.
            // No request payload and no credential reaches it.
            tracing::debug!(
                operation = "StartMedicalStreamTranscription",
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served an event-stream request"
            );
            response
        })
    }
}

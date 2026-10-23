// SPDX-License-Identifier: CC0-1.0

//! @op StartStreamTranscription
//!
//! `POST /stream-transcription -> 200`. Opens a transcription session, stores the
//! settings it negotiated, accounts for the audio the client streams in, and
//! answers with the modeled `TranscriptResultStream` event stream.
//!
//! What that stream carries: this emulator has NO speech recogniser, so it reports
//! no transcript. It does not invent one either — the stream carries the modeled
//! `ServiceUnavailableException` member of `TranscriptResultStream`, framed as an
//! in-band exception with [`NO_RECOGNISER`] as its message, which is the model's own
//! channel for a stream that yields no result. Everything around the recognition is
//! real: the modeled refusals, the session, the negotiated settings read back off it
//! as response headers, the conflict rule and the capacity limits.
//!
//! This operation is hand-authored rather than generated: its input carries a
//! `@streaming` event-stream member and its output is a `@streaming` union, which
//! the op-stub generator declines (`src/ops/SKIPPED_STUBS.md`) and the generated
//! `ErasedOperation` shim refuses. The framing, and the seam that routes a request
//! here, live in [`super::types::event_stream`]; this file holds the behaviour.
//!
//! ## Input (every member an `@httpHeader` except the payload)
//!
//! * `MediaSampleRateHertz` (integer, range 8000..48000) **required**
//! * `MediaEncoding` (MediaEncoding: flac | g711-alaw | g711-ulaw | g729 | ogg-opus
//!   | pcm) **required**
//! * `LanguageCode` (LanguageCode), `PreferredLanguage`, `LanguageOptions`,
//!   `IdentifyLanguage`, `IdentifyMultipleLanguages`, `LanguageModelName`
//! * `SessionId` (len 36..36, UUID pattern) — generated when absent
//! * `VocabularyName`, `VocabularyNames`, `VocabularyFilterName`,
//!   `VocabularyFilterNames`, `VocabularyFilterMethod`
//! * `ShowSpeakerLabel`, `EnableChannelIdentification`, `NumberOfChannels` (range
//!   2..), `EnablePartialResultsStabilization`, `PartialResultsStability`
//! * `ContentIdentificationType`, `ContentRedactionType`, `PiiEntityTypes`
//! * `SessionResumeWindow` (range 1..300), `TranscriptFormat`
//! * `AudioStream` (`@httpPayload`, `@streaming` union of `AudioEvent`,
//!   `ConfigurationEvent`) **required**
//!
//! ## Output
//!
//! Every input member above echoed as a response header, plus `RequestId`
//! (`x-amzn-request-id`), and `TranscriptResultStream` (`@httpPayload`,
//! `@streaming`) as the body — carrying the in-band `ServiceUnavailableException`
//! described above.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — one or more arguments was not valid; for
//!   example `MediaEncoding` or `LanguageCode` used unsupported values.
//! * `ConflictException` (HTTP 409) — a new stream started with the same session ID;
//!   the current stream has been terminated.
//! * `InternalFailureException` (HTTP 500), `LimitExceededException` (HTTP 429),
//!   `ServiceUnavailableException` (HTTP 503).
//!
//! ## The WebSocket transport
//!
//! The same operation is also reachable at [`WEBSOCKET_PATH`] — the modeled URI plus
//! `-websocket` — where the input above is carried as query parameters instead of
//! `x-amzn-transcribe-*` headers (`sample-rate`, `media-encoding`, `language-code`,
//! …). Those parameters are read back onto the same modeled shape and validated
//! against the same constraints ([`settings_from_query`]), so this transport refuses
//! what the header transport refuses and with the same modeled
//! `BadRequestException`. The stream itself is then refused with the modeled
//! `ServiceUnavailableException`: an upgrade cannot be completed from inside a
//! service here, and [`NO_WEBSOCKET_TRANSPORT`] records why.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{new_uuid, SessionKind, SessionStart, StreamSession, StreamStatus};
use aws_smithy_runtime_api::http::{Request, Response};
use smithy_server_runtime::{BoxFuture, OperationContext, ServerHttpProtocol, Service};
use std::sync::Arc;
use std::time::SystemTime;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-transcribestreaming-transcribe.json");

/// The URI the model binds this operation to
/// (smithy_query(query="operation", operation="StartStreamTranscription"):
/// `POST /stream-transcription`). Its WebSocket transport is this path plus
/// [`WEBSOCKET_PATH_SUFFIX`].
pub const MODELED_PATH: &str = "/stream-transcription";

/// Where the WebSocket transport of this operation is reached.
pub const WEBSOCKET_PATH: &str = "/stream-transcription-websocket";

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the
/// model declares for it.
#[smithy_macros::operation_error("com.amazonaws.transcribestreaming#StartStreamTranscription")]
#[derive(Debug)]
pub enum StartStreamTranscriptionErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    InternalFailureException(InternalFailureException),
    LimitExceededException(LimitExceededException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// The `StartStreamTranscription` operation.
///
/// Owns the shared engine (the constructor seam). The generated routing table holds
/// one of these, and `service.rs` holds another wrapped in an
/// [`EventStreamRoute`] — both reach the same sessions, because the sessions live in
/// the shared engine, not on the operation.
pub struct StartStreamTranscription<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> StartStreamTranscription<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// Refuse the request the way the model documents: `BadRequestException`, HTTP 400.
fn bad_request(message: impl Into<String>) -> StartStreamTranscriptionErrorModeled {
    StartStreamTranscriptionErrorModeled::BadRequestException(BadRequestException {
        message: Some(message.into()),
    })
}

fn internal_failure(message: impl Into<String>) -> StartStreamTranscriptionErrorModeled {
    StartStreamTranscriptionErrorModeled::InternalFailureException(InternalFailureException {
        message: Some(message.into()),
    })
}

/// A limit of this emulator the request went past: the error the model documents
/// for a client that "has exceeded one of the Amazon Transcribe limits".
fn limit_exceeded(message: impl Into<String>) -> StartStreamTranscriptionErrorModeled {
    StartStreamTranscriptionErrorModeled::LimitExceededException(LimitExceededException {
        message: Some(message.into()),
    })
}

/// A stream this service cannot carry: the modeled error the model declares for
/// exactly that, HTTP 503.
fn service_unavailable(message: impl Into<String>) -> StartStreamTranscriptionErrorModeled {
    StartStreamTranscriptionErrorModeled::ServiceUnavailableException(ServiceUnavailableException {
        message: Some(message.into()),
    })
}

/// The rules this operation's input must satisfy ACROSS its members, which no
/// single-member constraint trait can express. Checked on both transports, from the
/// same settings shape, so neither transport accepts a combination the other
/// refuses.
fn cross_member_refusal(settings: &StartStreamTranscriptionSettings) -> Option<String> {
    channel_identification_refusal(
        settings.enable_channel_identification,
        settings.number_of_channels,
    )
    .or_else(|| {
        partial_results_stability_refusal(
            settings.enable_partial_results_stabilization,
            settings.partial_results_stability.as_deref(),
        )
    })
}

/// Answer the WebSocket transport of this operation: read the input off the query
/// string, validate it exactly as the header transport is validated, and refuse the
/// stream with the modeled `ServiceUnavailableException` ([`NO_WEBSOCKET_TRANSPORT`]
/// says why). Nothing is stored: no stream was carried, so no session exists.
fn websocket_refusal(
    protocol: &dyn ServerHttpProtocol,
    req: &Request,
) -> StartStreamTranscriptionErrorModeled {
    let settings: StartStreamTranscriptionSettings =
        match settings_from_query(protocol, req, &StartStreamTranscriptionSettings::SCHEMA) {
            Ok(settings) => settings,
            Err(message) => return bad_request(message),
        };
    match cross_member_refusal(&settings) {
        Some(message) => bad_request(message),
        None => service_unavailable(NO_WEBSOCKET_TRANSPORT),
    }
}

/// Word an event-stream decode failure as the modeled error for its kind.
fn from_event_stream(error: EventStreamError) -> StartStreamTranscriptionErrorModeled {
    match error {
        EventStreamError::Malformed(message) => bad_request(message),
        EventStreamError::TooManyEvents(message) => limit_exceeded(message),
    }
}

/// The conflict the model words as "A new stream started with the same session ID.
/// The current stream has been terminated."
fn conflict(session_id: &str) -> StartStreamTranscriptionErrorModeled {
    StartStreamTranscriptionErrorModeled::ConflictException(ConflictException {
        message: Some(format!(
            "A new stream started with the same session ID {session_id}. \
             The current stream has been terminated."
        )),
    })
}

impl<Eng: Send + Sync + 'static> StartStreamTranscription<Eng> {
    /// The whole request: read the settings, store a session, account for the audio,
    /// and answer with the result stream of the session that was stored.
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
    ) -> Result<Response, StartStreamTranscriptionErrorModeled> {
        let settings: StartStreamTranscriptionSettings =
            settings_from_headers(protocol, req, &StartStreamTranscriptionSettings::SCHEMA)
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
            SessionKind::Standard,
            new_uuid(),
            settings.media_sample_rate_hertz,
            settings.media_encoding.clone(),
            now,
        );
        session.language_code = settings.language_code.clone();
        session.vocabulary_name = settings.vocabulary_name.clone();
        session.vocabulary_filter_name = settings.vocabulary_filter_name.clone();
        session.vocabulary_filter_method = settings.vocabulary_filter_method.clone();
        session.language_model_name = settings.language_model_name.clone();
        session.identify_language = settings.identify_language;
        session.identify_multiple_languages = settings.identify_multiple_languages;
        session.language_options = settings.language_options.clone();
        session.preferred_language = settings.preferred_language.clone();
        session.vocabulary_names = settings.vocabulary_names.clone();
        session.vocabulary_filter_names = settings.vocabulary_filter_names.clone();
        session.show_speaker_label = settings.show_speaker_label;
        session.enable_channel_identification = settings.enable_channel_identification;
        session.number_of_channels = settings.number_of_channels;
        session.enable_partial_results_stabilization = settings.enable_partial_results_stabilization;
        session.partial_results_stability = settings.partial_results_stability.clone();
        session.content_identification_type = settings.content_identification_type.clone();
        session.content_redaction_type = settings.content_redaction_type.clone();
        session.pii_entity_types = settings.pii_entity_types.clone();
        session.session_resume_window = settings.session_resume_window;
        session.transcript_format = settings.transcript_format.clone();

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
                // and post-call analytics settings of a CALL ANALYTICS stream
                // (`StartCallAnalyticsStreamTranscription`), which this operation is
                // not. It is a modeled member of the union this operation binds, so
                // it is accepted rather than refused; nothing here consumes it, and
                // nothing it carries reaches the response.
                "ConfigurationEvent" => {}
                other => {
                    return Err(bad_request(format!(
                        "'{other}' is not a member of AudioStream; send AudioEvent frames"
                    )))
                }
            }
        }
        // The audio stream ends with the request body, so the session has run to
        // completion by the time the response is built.
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
            // Read the session back out of the store: the response reports what was
            // persisted, never what the request headers said.
            store
                .sessions
                .get(&session_id)
                .cloned()
                .ok_or_else(|| internal_failure("the session was not persisted"))?
        };

        // The ONE event this result stream carries: the modeled
        // `ServiceUnavailableException` member, framed as an in-band exception, which
        // is the model's own channel for "this stream produced no transcript". No
        // `TranscriptEvent` is emitted, because no recogniser ran — see
        // [`NO_RECOGNISER`].
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
        let headers = StartStreamTranscriptionHeaders {
            request_id: Some(stored.request_id.clone()),
            language_code: stored.language_code.clone(),
            media_sample_rate_hertz: Some(stored.media_sample_rate_hertz),
            media_encoding: Some(stored.media_encoding.clone()),
            vocabulary_name: stored.vocabulary_name.clone(),
            session_id: Some(stored.session_id.clone()),
            vocabulary_filter_name: stored.vocabulary_filter_name.clone(),
            vocabulary_filter_method: stored.vocabulary_filter_method.clone(),
            show_speaker_label: stored.show_speaker_label,
            enable_channel_identification: stored.enable_channel_identification,
            number_of_channels: stored.number_of_channels,
            enable_partial_results_stabilization: stored.enable_partial_results_stabilization,
            partial_results_stability: stored.partial_results_stability.clone(),
            content_identification_type: stored.content_identification_type.clone(),
            content_redaction_type: stored.content_redaction_type.clone(),
            pii_entity_types: stored.pii_entity_types.clone(),
            language_model_name: stored.language_model_name.clone(),
            identify_language: stored.identify_language,
            language_options: stored.language_options.clone(),
            preferred_language: stored.preferred_language.clone(),
            identify_multiple_languages: stored.identify_multiple_languages,
            vocabulary_names: stored.vocabulary_names.clone(),
            vocabulary_filter_names: stored.vocabulary_filter_names.clone(),
            session_resume_window: stored.session_resume_window,
            transcript_format: stored.transcript_format.clone(),
        };
        event_stream_response(
            protocol,
            "StartStreamTranscription",
            &headers,
            &StartStreamTranscriptionHeaders::SCHEMA,
            frames,
        )
        .map_err(|e| internal_failure(format!("the response could not be serialized: {e}")))
    }
}

impl<Eng: Send + Sync + 'static> EventStreamOperation for StartStreamTranscription<Eng> {
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            // Which transport the request arrived on decides where the input is read
            // from: the WebSocket spelling carries it in the query string, the
            // modeled HTTP/2 one in `x-amzn-transcribe-*` headers with the audio in
            // the body.
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
                operation = "StartStreamTranscription",
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served an event-stream request"
            );
            response
        })
    }
}

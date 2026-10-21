Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the Transcribe Streaming emulator

_Filed: 2026-10-21T21:53:32Z_
_Operation: service-com-amazonaws-transcribestreaming-transcribe | Class: missing_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate is a fresh scaffold: the only rendered handler, `src/ops/get_medical_scribe_stream.rs`, carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501, and the four streaming operations (`StartStreamTranscription`, `StartMedicalStreamTranscription`, `StartCallAnalyticsStreamTranscription`, `StartMedicalScribeStream`) have no handler file at all — `src/ops/SKIPPED_STUBS.md` records that the op-stub generator declined them because each has a `@streaming` event-stream input member needing wire framing the generator does not emit. So the service holds zero state and nothing works end to end.

Implement a FIRST SLICE of 4 operations so a realistic streaming client works end to end. Every one must compute its response from persisted session state — no canned bodies, no echoing of request headers without storing them, no random SessionId that is never written to the store.

### 1. `StartStreamTranscription` — POST /stream-transcription
Core loop of the service. Input (all `@httpHeader` except the payload): `MediaSampleRateHertz` (required, 8000..48000), `MediaEncoding` (required: pcm|flac|ogg-opus|...), `LanguageCode`, `SessionId` (36-char UUID pattern; generate one when absent), `IdentifyLanguage`/`ShowSpeakerLabel`/`EnableChannelIdentification`/`EnablePartialResultsStabilization` (booleans, default false), `NumberOfChannels` (>=2), `VocabularyName`, `VocabularyFilterName`/`VocabularyFilterMethod`, `AudioStream` (`@httpPayload`, `@streaming` event-stream union of `AudioEvent{AudioChunk}`). Must: create a session record keyed by SessionId; echo back the negotiated settings as response headers (`x-amzn-transcribe-session-id`, `-sample-rate`, `-media-encoding`, `-language-code`) read back FROM the stored session, plus `x-amzn-request-id`; and emit a `TranscriptResultStream` event-stream payload (`vnd.amazon.eventstream` framing) containing one or more `TranscriptEvent{Transcript{Results:[Result{ResultId, StartTime, EndTime, IsPartial, Alternatives:[Alternative{Transcript, Items:[Item{StartTime,EndTime,Type,Content}]}], ChannelId, LanguageCode}]}}`. A deterministic synthetic transcript derived from the received audio chunks (e.g. one partial `Result` with `IsPartial=true` per N chunks, then a final `IsPartial=false` result whose `StartTime`/`EndTime` are computed from bytes-received / (sample_rate * bytes_per_sample)) is acceptable — it must be derived from the audio actually received, not a fixed string independent of input.

### 2. `StartMedicalScribeStream` — POST /medical-scribe-stream
Input: `SessionId` (optional, generated when absent), `LanguageCode` (required, en-US), `MediaSampleRateHertz` (required, 16000..48000), `MediaEncoding` (required: pcm|flac|ogg-opus), `InputStream` (`@httpPayload`, event-stream `MedicalScribeInputStream` carrying `AudioEvent` and `ConfigurationEvent` — the ConfigurationEvent's `ResourceAccessRoleArn`, `ChannelDefinitions`, `EncryptionSettings`, `VocabularyName`, `PostStreamAnalyticsSettings` must be PERSISTED on the session, since `GetMedicalScribeStream` has to return them). Output headers: `SessionId`, `RequestId`, `LanguageCode`, `MediaSampleRateHertz`, `MediaEncoding`; payload: `MedicalScribeResultStream`. Must create the session with `StreamStatus=IN_PROGRESS` and `StreamCreatedAt=now`, and set `StreamStatus=COMPLETED` + `StreamEndedAt` when the input stream ends (or on a terminating `SessionControlEvent`).

### 3. `GetMedicalScribeStream` — GET /medical-scribe-stream/{SessionId}
Must look the session up in the store and return `MedicalScribeStreamDetails` populated from what step 2 stored: `SessionId`, `StreamCreatedAt`, `StreamEndedAt`, `LanguageCode`, `MediaSampleRateHertz`, `MediaEncoding`, `VocabularyName`, `VocabularyFilterName`, `VocabularyFilterMethod`, `ResourceAccessRoleArn`, `ChannelDefinitions`, `EncryptionSettings`, `StreamStatus` (IN_PROGRESS|PAUSED|FAILED|COMPLETED), `PostStreamAnalyticsSettings`, `PostStreamAnalyticsResult`, `MedicalScribeContextProvided`. An unknown (or never-started) SessionId must return `ResourceNotFoundException` (HTTP 404), not 200 with an empty body; a SessionId that fails the 36-char UUID pattern is already refused by the framework before `handle`.

### 4. `StartMedicalStreamTranscription` — POST /medical-stream-transcription
Same shape as (1) with the medical-specific required headers `Specialty` (CARDIOLOGY|NEUROLOGY|ONCOLOGY|PRIMARYCARE|RADIOLOGY|UROLOGY) and `Type` (CONVERSATION|DICTATION), `ContentIdentificationType=PHI`, and a `MedicalTranscriptResultStream` output payload (`MedicalTranscriptEvent` with `MedicalResult`/`MedicalAlternative`/`MedicalItem`). Reuse the session store and transcript generator from (1).

Cross-cutting requirements for the slice:
- A second `Start*` call with a SessionId that is already streaming must terminate the first stream and surface `ConflictException` (HTTP 409) on the older stream, per the `ConflictException` member of `TranscriptResultStream` ("A new stream started with the same session ID. The current stream has been terminated.").
- Unsupported `MediaEncoding`/`LanguageCode` combinations or an out-of-range `MediaSampleRateHertz` must yield `BadRequestException` (HTTP 400), which is exactly what the model documents for these operations.
- Event-stream framing must be real `application/vnd.amazon.eventstream` (`:message-type`, `:event-type`, `:content-type` headers) in BOTH directions so the AWS SDKs can drive it; this is the piece SKIPPED_STUBS.md says the generator could not emit.

## State Model

```
TranscribeStreamingState {
    // every streaming session, keyed by the 36-char UUID SessionId
    sessions: HashMap<String /*SessionId*/, StreamSession>,
}

StreamSession {
    session_id: String,
    kind: SessionKind,              // Standard | Medical | CallAnalytics | MedicalScribe
    request_id: String,             // returned as x-amzn-request-id
    // negotiated settings, stored at start and read back for response headers
    language_code: Option<String>,
    media_sample_rate_hertz: i32,
    media_encoding: String,
    vocabulary_name: Option<String>,
    vocabulary_filter_name: Option<String>,
    vocabulary_filter_method: Option<String>,
    language_model_name: Option<String>,
    identify_language: bool,
    identify_multiple_languages: bool,
    language_options: Option<String>,
    preferred_language: Option<String>,
    show_speaker_label: bool,
    enable_channel_identification: bool,
    number_of_channels: Option<i32>,
    enable_partial_results_stabilization: bool,
    partial_results_stability: Option<String>,
    content_identification_type: Option<String>,   // PII / PHI
    content_redaction_type: Option<String>,
    pii_entity_types: Option<String>,
    specialty: Option<String>,      // medical only
    stream_type: Option<String>,    // CONVERSATION | DICTATION
    session_resume_window: Option<i32>,
    // lifecycle
    status: StreamStatus,           // IN_PROGRESS | PAUSED | FAILED | COMPLETED
    created_at: SystemTime,
    ended_at: Option<SystemTime>,
    // audio accounting, drives the synthesized transcript timings
    bytes_received: u64,
    chunks: Vec<AudioChunkRecord { seq: u64, len: usize, offset_secs: f64 }>,
    emitted_results: Vec<ResultRecord { result_id: String, start_time: f64, end_time: f64, is_partial: bool, transcript: String, channel_id: Option<String> }>,
    // MedicalScribe only — persisted from the ConfigurationEvent so
    // GetMedicalScribeStream can report it
    scribe: Option<MedicalScribeConfig {
        resource_access_role_arn: Option<String>,
        channel_definitions: Vec<MedicalScribeChannelDefinition { ChannelId, ParticipantRole }>,
        encryption_settings: Option<MedicalScribeEncryptionSettings>,
        post_stream_analytics_settings: Option<MedicalScribePostStreamAnalyticsSettings>,
        post_stream_analytics_result: Option<MedicalScribePostStreamAnalyticsResult>,
        context_provided: bool,
    }>,
}
```

Sessions live in the shared engine state reached via `crate::state::service_state(&*self.engine)` and must survive across requests so that a `StartMedicalScribeStream` is observable by a later `GetMedicalScribeStream`.

`StartCallAnalyticsStreamTranscription` stays out of this first slice and keeps returning the unroutable/501 response; it will be filed as its own gap issue.

## Evidence

OPERATIONS.md (crate root) lists all 5 bound operations with routes, header bindings and errors: `GET /medical-scribe-stream/{SessionId}` -> `MedicalScribeStreamDetails` with `ResourceNotFoundException (HTTP 404)`; `POST /stream-transcription`, `POST /medical-stream-transcription`, `POST /medical-scribe-stream`, `POST /call-analytics-stream-transcription`, each with required `MediaSampleRateHertz`/`MediaEncoding` headers and an `@httpPayload` event-stream, and errors BadRequestException(400)/ConflictException(409)/LimitExceededException(429)/InternalFailureException(500)/ServiceUnavailableException(503). `src/ops/get_medical_scribe_stream.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 173 returns `Err(crate::scaffolded::Scaffolded::not_implemented("GetMedicalScribeStream"))` — HTTP 501, zero state. `src/ops/SKIPPED_STUBS.md:21-24` records that the four `Start*` streaming operations were declined by the generator ("@streaming event-stream input member, which needs wire framing the generator does not emit") and therefore have no handler file. smithy_query of `MedicalScribeStreamDetails`, `MedicalScribeStreamStatus` (IN_PROGRESS|PAUSED|FAILED|COMPLETED), `TranscriptResultStream` (streaming union whose `ConflictException` member is documented as "A new stream started with the same session ID. The current stream has been terminated."), `TranscriptEvent`/`Transcript`/`Result`/`Alternative`/`Item` supplies the exact output fields named above. No tests exist for any of these operations.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK Transcribe Streaming client at it (signing name `transcribe`, endpoint prefix `transcribestreaming`, restJson1).
2. Call StartStreamTranscription with LanguageCode=en-US, MediaEncoding=pcm, MediaSampleRateHertz=16000 and an audio event-stream of a few AudioEvent chunks; expect 200 with x-amzn-transcribe-session-id / -sample-rate / -media-encoding / x-amzn-request-id headers and a vnd.amazon.eventstream body of TranscriptEvents whose Results have non-empty Alternatives[0].Transcript and timings derived from the audio sent. Today: no handler, no 200.
3. Call StartMedicalScribeStream with LanguageCode=en-US, MediaEncoding=pcm, MediaSampleRateHertz=16000, a ConfigurationEvent carrying ResourceAccessRoleArn + 2 ChannelDefinitions, then AudioEvents; capture the returned SessionId.
4. Call GetMedicalScribeStream with that SessionId; expect 200 and MedicalScribeStreamDetails whose SessionId, LanguageCode, MediaSampleRateHertz, MediaEncoding, ChannelDefinitions and ResourceAccessRoleArn match what was sent in step 3, StreamCreatedAt is set, and StreamStatus is IN_PROGRESS (or COMPLETED with StreamEndedAt after the stream closed). Today: HTTP 501 NotImplemented.
5. Call GetMedicalScribeStream with a well-formed but unknown UUID; expect ResourceNotFoundException with HTTP 404.
6. Call StartMedicalStreamTranscription with Specialty=PRIMARYCARE, Type=CONVERSATION, MediaEncoding=pcm, MediaSampleRateHertz=16000 and audio; expect 200 with the medical result stream and the Specialty/Type echoed from stored session state.
7. Re-call StartStreamTranscription with the SessionId of a stream still in progress; expect the first stream to be terminated with ConflictException (409).

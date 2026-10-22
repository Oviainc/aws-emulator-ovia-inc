// SPDX-License-Identifier: CC0-1.0

//! Type definitions for the three `@streaming` operations this crate hand-authors
//! (`src/ops/SKIPPED_STUBS.md` records why the op-stub generator declined them):
//! the header-bound halves of their inputs and outputs, the input events they
//! consume, and the two errors they declare that `GetMedicalScribeStream` does not.
//!
//! No RESULT-stream event structure is declared here. That is deliberate: this
//! emulator runs no speech recogniser, so no `TranscriptEvent`,
//! `MedicalTranscriptEvent` or `MedicalScribeTranscriptEvent` is ever constructed —
//! each result stream carries the modeled `ServiceUnavailableException` member of
//! its union instead (see `super::event_stream::NO_RECOGNISER`). Whoever adds a
//! recogniser declares those shapes then, beside the code that fills them.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all — the shapes the implemented
//! `GetMedicalScribeStream` already declares (`MedicalScribeChannelDefinition`,
//! `MedicalScribeEncryptionSettings`, `MedicalScribePostStreamAnalyticsSettings`,
//! `BadRequestException`, …) are reused from `get_medical_scribe_stream_types`
//! rather than re-declared here.
//!
//! ## Why the `*Settings` / `*Headers` pairs exist
//!
//! A `Start*` operation binds every input member but one to an `@httpHeader`; the
//! remaining member is the `@streaming` event-stream `@httpPayload`, whose framing
//! the generated codec does not speak. Declaring the request shape with that one
//! member in `unsupported = [..]` gives a struct the framework's own
//! `HttpBindingDeserializer` fills from the request headers and
//! `constraints::validate` checks against the model's `@range` / `@pattern` /
//! enum-membership traits — so the header half of the request is validated by the
//! model, not by hand — while the payload half is decoded from real
//! `vnd.amazon.eventstream` frames in `super::event_stream`. The response pairs are
//! the same split: the `*Headers` struct serializes the modeled response headers,
//! and the event stream is the body.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-transcribestreaming-transcribe.json");

/// The one value `MedicalScribeSessionControlEventType` accepts: the event that
/// ends a HealthScribe streaming session
/// (smithy_query(query="shape", shape="MedicalScribeSessionControlEventType")).
pub const END_OF_SESSION: &str = "END_OF_SESSION";

// =============================================================================
// StartStreamTranscription
// =============================================================================

/// The header-bound half of `StartStreamTranscriptionRequest`; `AudioStream` is
/// the event-stream payload and is decoded separately.
#[smithy_macros::smithy(
    shape = "com.amazonaws.transcribestreaming#StartStreamTranscriptionRequest",
    unsupported = ["AudioStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct StartStreamTranscriptionSettings {
    pub language_code: Option<String>,
    pub media_sample_rate_hertz: i32,
    pub media_encoding: String,
    pub vocabulary_name: Option<String>,
    pub session_id: Option<String>,
    pub vocabulary_filter_name: Option<String>,
    pub vocabulary_filter_method: Option<String>,
    pub show_speaker_label: bool,
    pub enable_channel_identification: bool,
    pub number_of_channels: Option<i32>,
    pub enable_partial_results_stabilization: bool,
    pub partial_results_stability: Option<String>,
    pub content_identification_type: Option<String>,
    pub content_redaction_type: Option<String>,
    pub pii_entity_types: Option<String>,
    pub language_model_name: Option<String>,
    pub identify_language: bool,
    pub language_options: Option<String>,
    pub preferred_language: Option<String>,
    pub identify_multiple_languages: bool,
    pub vocabulary_names: Option<String>,
    pub vocabulary_filter_names: Option<String>,
    pub session_resume_window: Option<i32>,
    pub transcript_format: Option<String>,
}

/// The header-bound half of `StartStreamTranscriptionResponse`;
/// `TranscriptResultStream` is the event-stream payload.
#[smithy_macros::smithy(
    shape = "com.amazonaws.transcribestreaming#StartStreamTranscriptionResponse",
    unsupported = ["TranscriptResultStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct StartStreamTranscriptionHeaders {
    pub request_id: Option<String>,
    pub language_code: Option<String>,
    pub media_sample_rate_hertz: Option<i32>,
    pub media_encoding: Option<String>,
    pub vocabulary_name: Option<String>,
    pub session_id: Option<String>,
    pub vocabulary_filter_name: Option<String>,
    pub vocabulary_filter_method: Option<String>,
    pub show_speaker_label: bool,
    pub enable_channel_identification: bool,
    pub number_of_channels: Option<i32>,
    pub enable_partial_results_stabilization: bool,
    pub partial_results_stability: Option<String>,
    pub content_identification_type: Option<String>,
    pub content_redaction_type: Option<String>,
    pub pii_entity_types: Option<String>,
    pub language_model_name: Option<String>,
    pub identify_language: bool,
    pub language_options: Option<String>,
    pub preferred_language: Option<String>,
    pub identify_multiple_languages: bool,
    pub vocabulary_names: Option<String>,
    pub vocabulary_filter_names: Option<String>,
    pub session_resume_window: Option<i32>,
    pub transcript_format: Option<String>,
}

// =============================================================================
// StartMedicalStreamTranscription
// =============================================================================

/// The header-bound half of `StartMedicalStreamTranscriptionRequest`; `AudioStream`
/// is the event-stream payload and is decoded separately.
#[smithy_macros::smithy(
    shape = "com.amazonaws.transcribestreaming#StartMedicalStreamTranscriptionRequest",
    unsupported = ["AudioStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct StartMedicalStreamTranscriptionSettings {
    pub language_code: String,
    pub media_sample_rate_hertz: i32,
    pub media_encoding: String,
    pub vocabulary_name: Option<String>,
    pub specialty: String,
    pub r#type: String,
    pub show_speaker_label: bool,
    pub session_id: Option<String>,
    pub enable_channel_identification: bool,
    pub number_of_channels: Option<i32>,
    pub content_identification_type: Option<String>,
}

/// The header-bound half of `StartMedicalStreamTranscriptionResponse`;
/// `TranscriptResultStream` is the event-stream payload.
#[smithy_macros::smithy(
    shape = "com.amazonaws.transcribestreaming#StartMedicalStreamTranscriptionResponse",
    unsupported = ["TranscriptResultStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct StartMedicalStreamTranscriptionHeaders {
    pub request_id: Option<String>,
    pub language_code: Option<String>,
    pub media_sample_rate_hertz: Option<i32>,
    pub media_encoding: Option<String>,
    pub vocabulary_name: Option<String>,
    pub specialty: Option<String>,
    pub r#type: Option<String>,
    pub show_speaker_label: bool,
    pub session_id: Option<String>,
    pub enable_channel_identification: bool,
    pub number_of_channels: Option<i32>,
    pub content_identification_type: Option<String>,
}

// =============================================================================
// StartMedicalScribeStream
// =============================================================================

/// The header-bound half of `StartMedicalScribeStreamRequest`; `InputStream` is the
/// event-stream payload.
#[smithy_macros::smithy(
    shape = "com.amazonaws.transcribestreaming#StartMedicalScribeStreamRequest",
    unsupported = ["InputStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct StartMedicalScribeStreamSettings {
    pub session_id: Option<String>,
    pub language_code: String,
    pub media_sample_rate_hertz: i32,
    pub media_encoding: String,
}

/// The header-bound half of `StartMedicalScribeStreamResponse`; `ResultStream` is
/// the event-stream payload.
#[smithy_macros::smithy(
    shape = "com.amazonaws.transcribestreaming#StartMedicalScribeStreamResponse",
    unsupported = ["ResultStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct StartMedicalScribeStreamHeaders {
    pub session_id: Option<String>,
    pub request_id: Option<String>,
    pub language_code: Option<String>,
    pub media_sample_rate_hertz: Option<i32>,
    pub media_encoding: Option<String>,
}

/// The event every `MedicalScribeInputStream` must open with, and the one this
/// service persists: `GetMedicalScribeStream` reports everything it carries.
#[smithy_macros::smithy(shape = "com.amazonaws.transcribestreaming#MedicalScribeConfigurationEvent")]
#[derive(Debug, Clone, Default)]
pub struct MedicalScribeConfigurationEvent {
    pub vocabulary_name: Option<String>,
    pub vocabulary_filter_name: Option<String>,
    pub vocabulary_filter_method: Option<String>,
    pub resource_access_role_arn: String,
    pub channel_definitions: Option<Vec<MedicalScribeChannelDefinition>>,
    pub encryption_settings: Option<MedicalScribeEncryptionSettings>,
    pub post_stream_analytics_settings: MedicalScribePostStreamAnalyticsSettings,
    pub medical_scribe_context: Option<MedicalScribeContext>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.transcribestreaming#MedicalScribeContext")]
#[derive(Debug, Clone, Default)]
pub struct MedicalScribeContext {
    pub patient_context: Option<MedicalScribePatientContext>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.transcribestreaming#MedicalScribePatientContext")]
#[derive(Debug, Clone, Default)]
pub struct MedicalScribePatientContext {
    pub pronouns: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.transcribestreaming#MedicalScribeSessionControlEvent")]
#[derive(Debug, Clone, Default)]
pub struct MedicalScribeSessionControlEvent {
    pub r#type: String,
}

// =============================================================================
// Errors the `Start*` operations declare that `GetMedicalScribeStream` does not
// =============================================================================

#[smithy_macros::smithy(shape = "com.amazonaws.transcribestreaming#ConflictException")]
#[derive(Debug, Clone, Default)]
pub struct ConflictException {
    pub message: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.transcribestreaming#ServiceUnavailableException")]
#[derive(Debug, Clone, Default)]
pub struct ServiceUnavailableException {
    pub message: Option<String>,
}

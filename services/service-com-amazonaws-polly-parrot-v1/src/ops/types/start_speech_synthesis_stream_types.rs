// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `start_speech_synthesis_stream` operation.
//!
//! Hand-authored rather than generated: the op-stub generator declines an operation
//! carrying a `@streaming` event-stream member (`src/ops/SKIPPED_STUBS.md` records
//! it), so neither its shapes nor its handler were emitted. Each structure is
//! declared exactly once across this service's `types/` modules, and `types/mod.rs`
//! re-exports them all — the error shapes the other operations already declare
//! (`ServiceFailureException`) are reused from `shared_types` rather than
//! re-declared here.
//!
//! ## Why the input is split in two
//!
//! `StartSpeechSynthesisStreamInput` binds every member but one to an
//! `@httpHeader`; the remaining member, `ActionStream`, is the `@streaming`
//! event-stream `@httpPayload`, whose framing the generated codec does not speak.
//! Declaring the request shape with that member in `unsupported = [..]` gives a
//! struct the framework's own HTTP binding deserializer fills from the request
//! headers and `constraints::validate` checks against the model's enum, length and
//! pattern traits — so the header half of the request is validated by the MODEL —
//! while the payload half is decoded from real `vnd.amazon.eventstream` frames in
//! the operation. The output needs no such split: its only member is the
//! `@streaming` payload, so the stream IS the response body.
//!
//! ## The events declared here, and the one that is not
//!
//! `TextEvent` and `CloseStreamEvent` are the two members of the input union
//! `StartSpeechSynthesisStreamActionStream`, decoded from the frames a caller
//! sends; `StreamClosedEvent` is the output union member that ends the stream.
//! `AudioEvent` is deliberately NOT declared: its `AudioChunk` member is the
//! event's `@eventPayload`, so the frame payload IS the raw chunk rather than a
//! JSON object with a member in it, and the operation writes that frame from the
//! chunk bytes directly (see the framing note on the operation).

#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-polly-parrot-v1.json");

/// The header-bound half of `StartSpeechSynthesisStreamInput`; `ActionStream` is the
/// event-stream payload and is decoded frame by frame instead.
#[smithy_macros::smithy(
    shape = "com.amazonaws.polly#StartSpeechSynthesisStreamInput",
    unsupported = ["ActionStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct StartSpeechSynthesisStreamSettings {
    pub engine: String,
    pub language_code: Option<String>,
    pub lexicon_names: Option<Vec<String>>,
    pub output_format: String,
    pub sample_rate: Option<String>,
    pub voice_id: String,
}

/// One `TextEvent` of the input stream: text to synthesize, how to read it, and
/// whether the audio buffered so far has to be written out at once.
#[smithy_macros::smithy(shape = "com.amazonaws.polly#TextEvent")]
#[derive(Debug, Clone, Default)]
pub struct TextEvent {
    pub text: String,
    pub text_type: Option<String>,
    pub flush_stream_configuration: Option<FlushStreamConfiguration>,
}

/// "Configuration for controlling when synthesized audio flushes to the output
/// stream."
#[smithy_macros::smithy(shape = "com.amazonaws.polly#FlushStreamConfiguration")]
#[derive(Debug, Clone, Default)]
pub struct FlushStreamConfiguration {
    pub force: bool,
}

/// The input event that ends the input stream, after which "all audio will be
/// returned". It carries no member.
#[smithy_macros::smithy(shape = "com.amazonaws.polly#CloseStreamEvent")]
#[derive(Debug, Clone, Default)]
pub struct CloseStreamEvent {}

/// The output event that closes the stream, carrying "the total number of
/// characters synthesized during the streaming session".
#[smithy_macros::smithy(shape = "com.amazonaws.polly#StreamClosedEvent")]
#[derive(Debug, Clone, Default)]
pub struct StreamClosedEvent {
    pub request_characters: i32,
}

/// "The input fails to satisfy the constraints specified by the service" — this
/// operation's 400, which carries the reason class and the fields that failed.
#[smithy_macros::smithy(shape = "com.amazonaws.polly#ValidationException")]
#[derive(Debug, Clone, Default)]
pub struct ValidationException {
    pub message: String,
    pub reason: String,
    pub fields: Option<Vec<ValidationExceptionField>>,
}

/// "Information about a field that failed validation."
#[smithy_macros::smithy(shape = "com.amazonaws.polly#ValidationExceptionField")]
#[derive(Debug, Clone, Default)]
pub struct ValidationExceptionField {
    pub name: String,
    pub message: String,
}

/// "The request would cause a service quota to be exceeded" — this operation's 402.
///
/// Declared because the operation lists it, so its error channel must carry a
/// variant for it; nothing constructs one. Both values of its `QuotaCode`
/// (smithy_query(query="shape", shape="QuotaCode")) are stream TIMEOUT quotas —
/// `input-stream-inbound-event-timeout` and `input-stream-timeout` — and this
/// emulator serves one buffered request per call, so no stream of its is ever left
/// waiting for an event or left open long enough to pass either.
#[smithy_macros::smithy(shape = "com.amazonaws.polly#ServiceQuotaExceededException")]
#[derive(Debug, Clone, Default)]
pub struct ServiceQuotaExceededException {
    pub message: String,
    pub quota_code: String,
    pub service_code: String,
}

/// "The request was denied because of request throttling" — this operation's other
/// 400.
///
/// Declared for the same reason as [`ServiceQuotaExceededException`] and likewise
/// never constructed: the emulator rate-limits nobody, so no request of its is
/// throttled.
#[smithy_macros::smithy(shape = "com.amazonaws.polly#ThrottlingException")]
#[derive(Debug, Clone, Default)]
pub struct ThrottlingException {
    pub message: Option<String>,
    pub throttling_reasons: Option<Vec<ThrottlingReason>>,
}

/// One reason a throttled request was throttled.
#[smithy_macros::smithy(shape = "com.amazonaws.polly#ThrottlingReason")]
#[derive(Debug, Clone, Default)]
pub struct ThrottlingReason {
    pub reason: Option<String>,
    pub resource: Option<String>,
}

/// The values `ValidationExceptionReason` accepts on the wire
/// (smithy_query(query="shape", shape="ValidationExceptionReason")).
pub const VALIDATION_EXCEPTION_REASON_VALUES: &[&str] = &[
    "fieldValidationFailed",
    "invalidInboundEvent",
    "other",
    "unsupportedOperation",
];

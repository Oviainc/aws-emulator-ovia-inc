// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `start_live_tail` operation.
//!
//! Hand-authored rather than generated: `StartLiveTail`'s output member
//! `responseStream` targets a `@streaming` union, so the op-stub generator emitted
//! neither the operation nor its shapes (`src/ops/SKIPPED_STUBS.md`). Two groups:
//!
//! * the REQUEST shape, declared in full so the framework's own deserializer fills it
//!   from the awsJson body and `constraints::validate` checks it against the model's
//!   `@length` / `@pattern` traits — the request half is judged by the model, not by
//!   hand; and
//! * the event structures the output union carries (`sessionStart` and `sessionUpdate`),
//!   with the shapes beneath them. The union itself is never constructed: an event is
//!   framed by its member name, so only the member's target shape needs a Rust type.
//!
//! `StartLiveTailResponse` has no shape here: its only member is the `@streaming`
//! payload, so there are no modelled response headers to serialize.
//!
//! Each structure is declared exactly once across this service's `types/` modules, and
//! `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json");

/// `StartLiveTailRequest`: which log groups the session tails, which of their streams,
/// and the pattern the delivered events must match.
#[smithy_macros::smithy(shape = "com.amazonaws.cloudwatchlogs#StartLiveTailRequest")]
#[derive(Debug, Clone, Default)]
pub struct StartLiveTailInput {
    pub log_group_identifiers: Vec<String>,
    pub log_stream_names: Option<Vec<String>>,
    pub log_stream_name_prefixes: Option<Vec<String>>,
    pub log_event_filter_pattern: Option<String>,
}

/// The `sessionStart` member of `StartLiveTailResponseStream`: "A single
/// LiveTailSessionStart object is sent at the start of the session", naming the session
/// and the selection it was started with.
#[smithy_macros::smithy(shape = "com.amazonaws.cloudwatchlogs#LiveTailSessionStart")]
#[derive(Debug, Clone, Default)]
pub struct LiveTailSessionStart {
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    pub log_group_identifiers: Option<Vec<String>>,
    pub log_stream_names: Option<Vec<String>>,
    pub log_stream_name_prefixes: Option<Vec<String>>,
    pub log_event_filter_pattern: Option<String>,
}

/// The `sessionUpdate` member of `StartLiveTailResponseStream`: one update's log events,
/// with the metadata saying whether they were sampled.
#[smithy_macros::smithy(shape = "com.amazonaws.cloudwatchlogs#LiveTailSessionUpdate")]
#[derive(Debug, Clone, Default)]
pub struct LiveTailSessionUpdate {
    pub session_metadata: Option<LiveTailSessionMetadata>,
    pub session_results: Option<Vec<LiveTailSessionLogEvent>>,
}

/// `LiveTailSessionMetadata`: whether the update's events are a sample of a larger
/// match.
#[smithy_macros::smithy(shape = "com.amazonaws.cloudwatchlogs#LiveTailSessionMetadata")]
#[derive(Debug, Clone, Default)]
pub struct LiveTailSessionMetadata {
    pub sampled: bool,
}

/// `LiveTailSessionLogEvent`: one log event a session delivers, and where it was
/// ingested.
#[smithy_macros::smithy(shape = "com.amazonaws.cloudwatchlogs#LiveTailSessionLogEvent")]
#[derive(Debug, Clone, Default)]
pub struct LiveTailSessionLogEvent {
    pub log_stream_name: Option<String>,
    pub log_group_identifier: Option<String>,
    pub message: Option<String>,
    pub timestamp: Option<i64>,
    pub ingestion_time: Option<i64>,
}

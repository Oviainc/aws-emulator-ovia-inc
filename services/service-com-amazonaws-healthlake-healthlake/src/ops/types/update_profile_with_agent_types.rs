// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `update_profile_with_agent` operation.
//!
//! HAND-AUTHORED, not generated: the op-stub generator declined this operation
//! because its output member is a `@streaming` event-stream union
//! (`src/ops/SKIPPED_STUBS.md`), so it emitted no types for it either. Two groups
//! are declared here:
//!
//! * the REQUEST shape, declared in full so the framework's own deserializer fills
//!   it and `constraints::validate` checks it against the model's `@length`,
//!   `@pattern` and enum-membership traits — the request half is validated by the
//!   model, not by hand; and
//! * the event structures `AgentOutputMessageStream` carries, one per frame the
//!   response emits.
//!
//! `HeartbeatEvent` — the union's remaining member — is deliberately not declared:
//! it exists to hold a long-lived stream open, and a turn this emulator serves is
//! computed in one step, so there is no idle period for one to fill and nothing
//! here would ever construct it.
//!
//! Each structure is declared exactly once across this service's `types/` modules,
//! and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-healthlake-healthlake.json");

/// `UpdateProfileWithAgentRequest`: the profile the turn is about, the caller's
/// message, and the conversation it belongs to (absent on the first turn).
#[smithy_macros::smithy(shape = "com.amazonaws.healthlake#UpdateProfileWithAgentRequest")]
#[derive(Debug, Clone, Default)]
pub struct UpdateProfileWithAgentInput {
    pub profile_id: String,
    pub input_message: AgentInputMessage,
    pub conversation_id: Option<String>,
}

/// One message from the caller to the agent. `Body` is `@sensitive`, so nothing in
/// this crate stores it or quotes it back.
#[smithy_macros::smithy(shape = "com.amazonaws.healthlake#AgentInputMessage")]
#[derive(Debug, Clone, Default)]
pub struct AgentInputMessage {
    pub body: String,
    pub r#type: String,
}

/// The `TextMessageEvent` member of `AgentOutputMessageStream`: the agent's own
/// text for this turn.
#[smithy_macros::smithy(shape = "com.amazonaws.healthlake#TextMessageEvent")]
#[derive(Debug, Clone, Default)]
pub struct TextMessageEvent {
    pub message: String,
}

/// The `ProgressEvent` member: what the agent is doing before it answers.
#[smithy_macros::smithy(shape = "com.amazonaws.healthlake#ProgressEvent")]
#[derive(Debug, Clone, Default)]
pub struct ProgressEvent {
    pub message: String,
}

/// The `ErrorEvent` member: a failure the turn ran into IN BAND — the request and
/// the profile were both fine, so the stream had already begun.
#[smithy_macros::smithy(shape = "com.amazonaws.healthlake#ErrorEvent")]
#[derive(Debug, Clone, Default)]
pub struct ErrorEvent {
    pub error: String,
    pub message: String,
}

/// The `EndOfOutputEvent` member: the turn's final response, the options the next
/// turn may answer with, and the conversation this turn belongs to.
#[smithy_macros::smithy(shape = "com.amazonaws.healthlake#EndOfOutputEvent")]
#[derive(Debug, Clone, Default)]
pub struct EndOfOutputEvent {
    pub final_response_message: String,
    pub options: Option<Vec<String>>,
    pub conversation_id: Option<String>,
}

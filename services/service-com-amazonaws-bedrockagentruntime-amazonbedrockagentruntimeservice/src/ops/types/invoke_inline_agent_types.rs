// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `invoke_inline_agent` operation, HAND-AUTHORED:
//! the op-stub generator declined `InvokeInlineAgent` because its output carries a
//! `@streaming` event-stream member it cannot frame (`../SKIPPED_STUBS.md`), and an
//! operation with no types and no handler is not in the service's routing table at all —
//! so nothing validated this operation's own request.
//!
//! The split follows the one the other hand-authored streaming surfaces in this tree use
//! (`services/service-com-amazonaws-transcribestreaming-transcribe/src/ops/types/\
//! streaming_types.rs`): the members the framework's own codec can read are declared, and
//! the event stream — plus the structured members whose shapes this crate does not declare
//! yet — sit in `unsupported = [..]`, which keeps them out of the schema. What IS declared
//! is what the framework's `HttpBindingDeserializer` and `constraints::validate` then see,
//! so this operation's `@required`, `@length`, `@range` and `@pattern` members are checked
//! against the MODEL rather than by hand.
//!
//! Whoever implements the inline agent's response stream declares the shapes behind the
//! members listed as unsupported here, beside the code that fills them.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

/// The half of `InvokeInlineAgentRequest` the framework's codec reads: the URI label and
/// the scalar body members, including the three the model marks `@required`
/// (`foundationModel`, `instruction` and the `sessionId` label).
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#InvokeInlineAgentRequest",
    unsupported = [
        "actionGroups",
        "knowledgeBases",
        "guardrailConfiguration",
        "promptOverrideConfiguration",
        "collaboratorConfigurations",
        "streamingConfigurations",
        "promptCreationConfigurations",
        "inlineSessionState",
        "collaborators",
        "bedrockModelConfigurations",
        "customOrchestration"
    ]
)]
#[derive(Debug, Clone, Default)]
pub struct InvokeInlineAgentInput {
    pub session_id: String,
    pub foundation_model: String,
    pub instruction: String,
    pub customer_encryption_key_arn: Option<String>,
    pub idle_session_ttl_in_seconds: Option<i32>,
    pub agent_collaboration: Option<String>,
    pub agent_name: Option<String>,
    pub end_session: Option<bool>,
    pub enable_trace: Option<bool>,
    pub input_text: Option<String>,
    pub orchestration_type: Option<String>,
}

/// The header half of `InvokeInlineAgentResponse`; `completion` is the event-stream
/// payload, which no code here frames, so it is unsupported.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#InvokeInlineAgentResponse",
    unsupported = ["completion"]
)]
#[derive(Debug, Clone, Default)]
pub struct InvokeInlineAgentOutput {
    pub content_type: String,
    pub session_id: String,
}

// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `invoke_code_interpreter` operation.
//!
//! Hand-authored rather than generated: `InvokeCodeInterpreter`'s output member
//! `stream` targets a `@streaming` union, so the op-stub generator emitted neither the
//! operation nor its shapes (`src/ops/SKIPPED_STUBS.md`). Three groups, the same three
//! a hand-authored event-stream operation always needs:
//!
//! * the request shape, declared in full so the framework's own HTTP-binding
//!   deserializer fills it from the URI label, the `x-amzn-code-interpreter-session-id`
//!   header and the JSON body, and `constraints::validate` checks it against the
//!   model's `@pattern` / `@length` / enum-membership traits — the request half is
//!   validated by the model, not by hand;
//! * the `@httpHeader`-bound half of `InvokeCodeInterpreterResponse`, whose remaining
//!   member is the event-stream payload; and
//! * the event structure the output union carries (`result`), with the shapes beneath
//!   it.
//!
//! Each structure is declared exactly once across this service's `types/` modules, and
//! `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json");

/// `InvokeCodeInterpreterRequest`. `sessionId` binds the
/// `x-amzn-code-interpreter-session-id` header, `name` is the `ToolName` enum and
/// `arguments` is the one structure every tool reads its parameters from.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcore#InvokeCodeInterpreterRequest")]
#[derive(Debug, Clone, Default)]
pub struct InvokeCodeInterpreterInput {
    pub code_interpreter_identifier: String,
    pub session_id: Option<String>,
    pub trace_id: Option<String>,
    pub trace_parent: Option<String>,
    pub name: String,
    pub arguments: Option<ToolArguments>,
}

/// `ToolArguments`: the union of every tool's parameters, as the model declares it —
/// each tool reads the members that are its own.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcore#ToolArguments")]
#[derive(Debug, Clone, Default)]
pub struct ToolArguments {
    pub code: Option<String>,
    pub language: Option<String>,
    pub clear_context: Option<bool>,
    pub command: Option<String>,
    pub path: Option<String>,
    pub paths: Option<Vec<String>>,
    pub content: Option<Vec<InputContentBlock>>,
    pub directory_path: Option<String>,
    pub task_id: Option<String>,
    pub runtime: Option<String>,
}

/// `InputContentBlock`: one file a `writeFiles` call puts in the session's workspace,
/// as text or as bytes.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcore#InputContentBlock")]
#[derive(Debug, Clone, Default)]
pub struct InputContentBlock {
    pub path: String,
    pub text: Option<String>,
    pub blob: Option<Vec<u8>>,
}

/// The `@httpHeader`-bound half of `InvokeCodeInterpreterResponse`; `stream` is the
/// `CodeInterpreterStreamOutput` event-stream payload and is framed separately.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentcore#InvokeCodeInterpreterResponse",
    unsupported = ["stream"]
)]
#[derive(Debug, Clone, Default)]
pub struct InvokeCodeInterpreterHeaders {
    pub session_id: Option<String>,
}

/// The `result` member of `CodeInterpreterStreamOutput`: what the invoked tool
/// produced.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcore#CodeInterpreterResult")]
#[derive(Debug, Clone, Default)]
pub struct CodeInterpreterResult {
    pub content: Vec<ContentBlock>,
    pub structured_content: Option<ToolResultStructuredContent>,
    pub is_error: Option<bool>,
}

/// `ContentBlock`: one block of a tool result. `type` is the `ContentBlockType` enum
/// (`text` | `image` | `resource` | `resource_link`), and which of the remaining
/// members carry a value follows from it.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcore#ContentBlock")]
#[derive(Debug, Clone, Default)]
pub struct ContentBlock {
    pub r#type: String,
    pub text: Option<String>,
    pub data: Option<Vec<u8>>,
    pub mime_type: Option<String>,
    pub uri: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub size: Option<i64>,
    pub resource: Option<ResourceContent>,
}

/// `ResourceContent`: the contents of a file a `resource` block carries, as text or as
/// bytes.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcore#ResourceContent")]
#[derive(Debug, Clone, Default)]
pub struct ResourceContent {
    pub r#type: String,
    pub uri: Option<String>,
    pub mime_type: Option<String>,
    pub text: Option<String>,
    pub blob: Option<Vec<u8>>,
}

/// `ToolResultStructuredContent`: the machine-readable half of a tool result — what a
/// run wrote to its output streams, how it ended and how long it took.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcore#ToolResultStructuredContent")]
#[derive(Debug, Clone, Default)]
pub struct ToolResultStructuredContent {
    pub task_id: Option<String>,
    pub task_status: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub exit_code: Option<i32>,
    pub execution_time: Option<f64>,
}

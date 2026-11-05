// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `invoke_with_response_stream` operation.
//!
//! Hand-authored, because the scaffold op-stub generator declined this operation (its
//! output binds a `@streaming` union; see `src/ops/SKIPPED_STUBS.md`) and so emitted no
//! types module for it. Every shape is still declared against the Smithy model through
//! `#[smithy_macros::smithy(shape = ...)]`, which is what bakes each member's wire name,
//! HTTP binding and constraint traits onto its schema — so the bindings this operation
//! reads a request through are the model's, not this file's.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-lambda-awsgirapiservice.json");

#[smithy_macros::smithy(shape = "com.amazonaws.lambda#InvokeWithResponseStreamRequest")]
#[derive(Debug, Clone, Default)]
pub struct InvokeWithResponseStreamInput {
    pub function_name: String,
    pub invocation_type: Option<String>,
    pub log_type: Option<String>,
    pub client_context: Option<String>,
    pub qualifier: Option<String>,
    pub payload: Option<Vec<u8>>,
    pub tenant_id: Option<String>,
}

/// The operation's output, minus its `EventStream`.
///
/// `EventStream` targets the `@streaming` union `InvokeWithResponseStreamResponseEvent`,
/// which is not a value this struct can hold: it is the response's event-stream FRAMING,
/// written frame by frame. Everything else is an `@httpResponseCode` or `@httpHeader`
/// member.
#[smithy_macros::smithy(
    shape = "com.amazonaws.lambda#InvokeWithResponseStreamResponse",
    unsupported = ["EventStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct InvokeWithResponseStreamOutput {
    pub status_code: i32,
    pub executed_version: Option<String>,
    pub response_stream_content_type: Option<String>,
}

/// One `PayloadChunk` event: "A chunk of the streamed response payload", whose `Payload`
/// is "Data returned by your Lambda function".
#[smithy_macros::smithy(shape = "com.amazonaws.lambda#InvokeResponseStreamUpdate")]
#[derive(Debug, Clone, Default)]
pub struct InvokeResponseStreamUpdate {
    pub payload: Option<Vec<u8>>,
}

/// The terminating `InvokeComplete` event: "An object that's returned when the stream has
/// ended and all the payload chunks have been returned."
#[smithy_macros::smithy(shape = "com.amazonaws.lambda#InvokeWithResponseStreamCompleteEvent")]
#[derive(Debug, Clone, Default)]
pub struct InvokeWithResponseStreamCompleteEvent {
    pub error_code: Option<String>,
    pub error_details: Option<String>,
    pub log_result: Option<String>,
}

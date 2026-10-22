// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached by the `invoke_endpoint_with_bidirectional_stream`
//! operation.
//!
//! Hand-authored (the scaffold generator declined this service), but every shape
//! is declared against the Smithy model through
//! `#[smithy_macros::smithy(shape = ...)]`, which is what bakes each member's
//! wire name, HTTP binding and `@length`/`@pattern` constraint traits onto its
//! schema. The handler enforces those constraints by running
//! `aws_smithy_schema_server::constraints::validate` over a built value, so the
//! bounds it refuses are the MODEL's and are never restated here.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2.json");

/// The operation's input, minus its `Body`.
///
/// `Body` targets the `@streaming` union `RequestStreamEvent`, which is not a
/// value this struct can hold: it is the request's event-stream FRAMING, decoded
/// frame by frame by the handler. Everything else is an `@httpLabel` or
/// `@httpHeader` member, and holding them here is what lets the handler validate
/// them against the model's own `@length`/`@pattern` traits.
#[smithy_macros::smithy(
    shape = "com.amazonaws.sagemakerruntimehttp2#InvokeEndpointWithBidirectionalStreamInput",
    unsupported = ["Body"]
)]
#[derive(Debug, Clone, Default)]
pub struct InvokeEndpointWithBidirectionalStreamInput {
    pub endpoint_name: String,
    pub target_variant: Option<String>,
    pub model_invocation_path: Option<String>,
    pub model_query_string: Option<String>,
}

/// One decoded `RequestStreamEvent$PayloadPart`.
///
/// Built per request frame so the frame's `@eventHeader` members are validated
/// against the model's `@pattern` traits (`DataType` is `^(UTF8)$|^(BINARY)$`,
/// `CompletionState` is `^(PARTIAL)$|^(COMPLETE)$`).
#[smithy_macros::smithy(shape = "com.amazonaws.sagemakerruntimehttp2#RequestPayloadPart")]
#[derive(Debug, Clone, Default)]
pub struct RequestPayloadPart {
    pub bytes: Option<Vec<u8>>,
    pub data_type: Option<String>,
    pub completion_state: Option<String>,
    pub p: Option<String>,
}

/// One `ResponseStreamEvent$PayloadPart`, as the handler frames it.
#[smithy_macros::smithy(shape = "com.amazonaws.sagemakerruntimehttp2#ResponsePayloadPart")]
#[derive(Debug, Clone, Default)]
pub struct ResponsePayloadPart {
    pub bytes: Option<Vec<u8>>,
    pub data_type: Option<String>,
    pub completion_state: Option<String>,
    pub p: Option<String>,
}

/// `InputValidationError` — HTTP 400. The request named an endpoint that does not
/// exist, a variant the endpoint does not have, or a value outside a modelled
/// constraint.
#[smithy_macros::smithy(shape = "com.amazonaws.sagemakerruntimehttp2#InputValidationError")]
#[derive(Debug, Clone, Default)]
pub struct InputValidationError {
    pub message: Option<String>,
    pub error_code: Option<String>,
}

/// `InternalServerError` — HTTP 500.
#[smithy_macros::smithy(shape = "com.amazonaws.sagemakerruntimehttp2#InternalServerError")]
#[derive(Debug, Clone, Default)]
pub struct InternalServerError {
    pub message: Option<String>,
    pub error_code: Option<String>,
}

/// `ModelError` — HTTP 424. The endpoint's model refused the invocation before
/// any response frame was produced.
#[smithy_macros::smithy(shape = "com.amazonaws.sagemakerruntimehttp2#ModelError")]
#[derive(Debug, Clone, Default)]
pub struct ModelError {
    pub message: Option<String>,
    pub original_status_code: Option<i32>,
    pub original_message: Option<String>,
    pub log_stream_arn: Option<String>,
    pub error_code: Option<String>,
}

/// `ModelStreamError`. Declared by the operation as a top-level error (HTTP 400)
/// AND carried as a member of `ResponseStreamEvent`; the handler delivers it
/// IN-STREAM, because a failure the model reports after the response stream
/// opened cannot change a status already sent.
#[smithy_macros::smithy(shape = "com.amazonaws.sagemakerruntimehttp2#ModelStreamError")]
#[derive(Debug, Clone, Default)]
pub struct ModelStreamError {
    pub message: Option<String>,
    pub error_code: Option<String>,
}

/// `InternalStreamFailure`. As with [`ModelStreamError`], a member of
/// `ResponseStreamEvent` and delivered in-stream.
#[smithy_macros::smithy(shape = "com.amazonaws.sagemakerruntimehttp2#InternalStreamFailure")]
#[derive(Debug, Clone, Default)]
pub struct InternalStreamFailure {
    pub message: Option<String>,
}

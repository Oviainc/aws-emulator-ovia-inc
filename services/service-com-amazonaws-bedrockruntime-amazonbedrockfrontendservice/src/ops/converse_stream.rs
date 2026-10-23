// SPDX-License-Identifier: CC0-1.0

//! @op ConverseStream
//!
//! `POST /model/{modelId}/converse-stream -> 200`. Serves one conversational turn
//! as a `vnd.amazon.eventstream` body: the same turn `Converse` serves — the same
//! validation, the same tokenizer, the same assistant text, the same stop reason
//! and the same token counts, all read from [`crate::state`] — framed as the
//! `ConverseStreamOutput` events instead of one JSON object.
//!
//! The frames, in the order the model's union documents them for a turn that
//! completes: `messageStart` (the assistant's role), one `contentBlockDelta` per
//! content block of the message, `contentBlockStop`, `messageStop` (the stop
//! reason) and `metadata` (the usage and the latency actually measured). No
//! language model runs in this emulator, so the turn's text is produced in one
//! step (see [`crate::state::emulated_completion_text`]) and therefore arrives as
//! one delta per block — nothing here splits it into fabricated increments, and
//! nothing is echoed back from the request.
//!
//! This operation is hand-authored rather than generated: its output is a
//! `@streaming` union, which the op-stub generator declines
//! (`src/ops/SKIPPED_STUBS.md`) and whose generated `ErasedOperation` shim refuses
//! the request. The framing and the routing seam live in `service_common::event_stream`;
//! this file holds the behaviour.
//!
//! ## Input
//!
//! * `modelId` (ConversationalModelId, len 1..2048, `@httpLabel`) **required**
//! * `messages` (Messages), `system` (SystemContentBlocks)
//! * `inferenceConfig` (InferenceConfiguration), `toolConfig` (ToolConfiguration)
//! * `guardrailConfig` (GuardrailStreamConfiguration),
//!   `additionalModelRequestFields` (Document),
//!   `additionalModelResponseFieldPaths` (len ..10)
//! * `promptVariables` (PromptVariableMap), `requestMetadata` (len 1..16)
//! * `performanceConfig` (PerformanceConfiguration), `serviceTier` (ServiceTier),
//!   `outputConfig` (OutputConfig)
//!
//! ## Output
//!
//! * `stream` (ConverseStreamOutput, `@httpPayload`, `@streaming`) — the body; the
//!   response binds no modelled header.
//!
//! ## Errors
//!
//! * `ValidationException` (HTTP 400) — the input fails the constraints Amazon
//!   Bedrock states (a `modelId` that is not a model identifier, a conversation
//!   that is not user-first and alternating, a model the Converse API does not
//!   serve).
//! * `ResourceNotFoundException` (HTTP 404) — the model identifier names no model.
//! * `AccessDeniedException` (403), `InternalServerException` (500),
//!   `ModelErrorException` (424), `ModelNotReadyException` (429),
//!   `ModelTimeoutException` (408), `ServiceUnavailableException` (503),
//!   `ThrottlingException` (429).

#[allow(unused_imports)]
use super::types::*;
use aws_smithy_runtime_api::http::{Request, Response};
use service_common::event_stream::{
    deserialize_request, error_response, event_stream_only_response,
};
use smithy_server_runtime::{BoxFuture, OperationContext, ServerHttpProtocol, Service};
use std::sync::Arc;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json");

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the
/// model declares for it.
#[smithy_macros::operation_error("com.amazonaws.bedrockruntime#ConverseStream")]
#[derive(Debug)]
pub enum ConverseStreamErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ModelErrorException(ModelErrorException),
    ModelNotReadyException(ModelNotReadyException),
    ModelTimeoutException(ModelTimeoutException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's own modelled 400 `ValidationException`.
fn validation(message: impl Into<String>) -> ConverseStreamErrorModeled {
    ConverseStreamErrorModeled::ValidationException(ValidationException {
        message: Some(message.into()),
    })
}

fn internal(message: impl Into<String>) -> ConverseStreamErrorModeled {
    ConverseStreamErrorModeled::InternalServerException(InternalServerException {
        message: Some(message.into()),
    })
}

/// The `ConverseStream` operation.
///
/// Owns the shared engine (the constructor seam). The generated routing table holds
/// one of these and `service.rs` holds another wrapped in an `EventStreamRoute`;
/// both reach the same state, because the state lives in the shared engine, not on
/// the operation.
pub struct ConverseStream<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> ConverseStream<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ConverseStream<Eng> {
    /// The whole request: validate it against the model and the rules the Converse
    /// APIs state, serve the turn, record it in the inference log, and frame the
    /// turn as the event stream.
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
    ) -> Result<Response, ConverseStreamErrorModeled> {
        let started = std::time::Instant::now();
        let input: ConverseStreamInput =
            deserialize_request(protocol, req, &ConverseStreamInput::SCHEMA)
                .map_err(validation)?;

        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(|| internal("the Bedrock Runtime emulator has no state store"))?;

        let model = {
            let guard = state.lock();
            match guard.resolve_model_for_inference(&input.model_id) {
                Ok(entry) => entry.clone(),
                // An identifier the service cannot interpret is a 400; one
                // naming a model or resource that is not available here is a
                // 404 (see `crate::state::resolve_model_for_inference`).
                Err(crate::state::ModelRejection::Invalid(message)) => {
                    return Err(validation(message))
                }
                Err(crate::state::ModelRejection::NotFound(message)) => {
                    return Err(ConverseStreamErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: Some(message),
                        },
                    ))
                }
            }
        };
        if !model.supports_converse {
            return Err(validation(format!(
                "The model {} does not support the Converse API.",
                model.model_id
            )));
        }
        crate::state::validate_conversation(input.messages.as_ref()).map_err(validation)?;

        let turn = crate::state::converse_turn(
            &model,
            input.system.as_ref(),
            input.messages.as_ref(),
            input.tool_config.as_ref(),
            input
                .inference_config
                .as_ref()
                .and_then(|config| config.max_tokens),
        );

        state
            .lock()
            .log_invocation(crate::state::InvocationLogEntry {
                model_id: model.model_id.clone(),
                kind: crate::state::InvokeKind::ConverseStream,
                input_tokens: turn.input_tokens,
                output_tokens: turn.output_tokens,
                at_millis: crate::state::now_millis(),
            });

        // The assistant message this turn is, as one text content block — the same
        // message `Converse` answers with, so the two surfaces cannot drift.
        let blocks = [turn.text.clone()];
        let mut frames = Vec::new();
        self.frame(
            protocol,
            &mut frames,
            "messageStart",
            &MessageStartEvent {
                role: "assistant".to_string(),
            },
            &MessageStartEvent::SCHEMA,
        )?;
        for (index, text) in blocks.iter().enumerate() {
            let content_block_index = i32::try_from(index).unwrap_or(i32::MAX);
            self.frame(
                protocol,
                &mut frames,
                "contentBlockDelta",
                &ContentBlockDeltaEvent {
                    delta: ContentBlockDelta::Text(text.clone()),
                    content_block_index,
                },
                &ContentBlockDeltaEvent::SCHEMA,
            )?;
            self.frame(
                protocol,
                &mut frames,
                "contentBlockStop",
                &ContentBlockStopEvent {
                    content_block_index,
                },
                &ContentBlockStopEvent::SCHEMA,
            )?;
        }
        self.frame(
            protocol,
            &mut frames,
            "messageStop",
            &MessageStopEvent {
                stop_reason: turn.stop_reason().to_string(),
                additional_model_response_fields: None,
            },
            &MessageStopEvent::SCHEMA,
        )?;
        self.frame(
            protocol,
            &mut frames,
            "metadata",
            &ConverseStreamMetadataEvent {
                usage: turn.usage(),
                metrics: ConverseStreamMetrics {
                    latency_ms: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
                },
                trace: None,
                performance_config: input.performance_config.clone(),
                service_tier: input.service_tier.clone(),
            },
            &ConverseStreamMetadataEvent::SCHEMA,
        )?;

        // `ConverseStreamResponse` binds no header: the stream IS the response.
        Ok(event_stream_only_response(frames))
    }

    /// Append one `ConverseStreamOutput` event to the stream.
    fn frame(
        &self,
        protocol: &dyn ServerHttpProtocol,
        frames: &mut Vec<u8>,
        event_type: &str,
        event: &dyn ::aws_smithy_schema::serde::SerializableStruct,
        schema: &::aws_smithy_schema::Schema,
    ) -> Result<(), ConverseStreamErrorModeled> {
        let frame =
            frame_padded_event(protocol, event_type, event, schema).map_err(|error| {
                internal(format!(
                    "the {event_type} event could not be framed: {error}"
                ))
            })?;
        frames.extend_from_slice(&frame);
        Ok(())
    }
}

impl<Eng: Send + Sync + 'static> service_common::event_stream::EventStreamOperation
    for ConverseStream<Eng>
{
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            let response = match self.respond(protocol, req) {
                Ok(response) => response,
                Err(error) => error_response(protocol, &error),
            };
            // One access record per request, so a refused stream is diagnosable.
            // No request payload and no credential reaches it.
            tracing::debug!(
                operation = "ConverseStream",
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served an event-stream request"
            );
            response
        })
    }
}

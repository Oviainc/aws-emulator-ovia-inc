// SPDX-License-Identifier: CC0-1.0

//! @op InvokeInlineAgent
//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` once a
//! request satisfies the model's input constraints, and asserts NOTHING about
//! `InvokeInlineAgent`'s real behaviour. Implement it here — this is the ONLY file to touch
//! for this operation.
//!
//! HAND-AUTHORED, not generated: the op-stub generator declined this operation because its
//! `completion` output member is a `@streaming` event stream it cannot frame
//! (`SKIPPED_STUBS.md`), and an operation with no module at all is in no routing table, so
//! requests to `POST /agents/{sessionId}` reached no handler and were claimed by the
//! build-time plane's own `POST /agents/{agentId}/` route — which validated a member this
//! operation does not have. The request half is declared in
//! `super::types::invoke_inline_agent_types`, so the framework validates THIS operation's
//! members before `handle` runs; the response stream is what remains unimplemented.
//!
//! A 501 from this operation means "nobody has written it yet", never "the emulator is
//! broken". Replace the `Err(..)` in [`InvokeInlineAgent::handle`] with real behaviour and
//! delete the SCAFFOLDED marker line above; that line is the machine-readable record of
//! which operations are still stubs (`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' */src/ops/`),
//! so leaving it on an implemented operation under-reports progress.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `InvokeInlineAgent::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! ---
//!
//! # Modeled contract for `InvokeInlineAgent`
//!
//! What AWS documents for this operation, from this service's Smithy model. For the FULL
//! text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="operation",operation="InvokeInlineAgent",model_path="services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json")
//! ```
//!
//! Invokes an inline Amazon Bedrock agent using the configurations you provide with the
//! request.
//!
//! ## Route
//!
//! `POST /agents/{sessionId} -> 200`
//!
//! ## Input
//!
//! * `sessionId` (SessionId, len 2..100, pattern ^[0-9a-zA-Z._:-]+$, @httpLabel)
//!   **required** — The unique identifier of the session.
//! * `foundationModel` (ModelIdentifier, len 1..2048, pattern
//!   (^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/… …) **required**
//!   — The model identifier (ID) of the model to use for orchestration.
//! * `instruction` (Instruction, len 40.., @sensitive) **required** — The instructions that
//!   tell the inline agent what it should do.
//! * `customerEncryptionKeyArn` (KmsKeyArn, len 1..2048, pattern
//!   ^arn:aws(|-cn|-us-gov):kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$) — The ARN of
//!   the customer-managed key to use for encryption.
//! * `idleSessionTTLInSeconds` (SessionTTL, range 60..3600) — How long the session is kept.
//! * `agentName` (Name, pattern ^([0-9a-zA-Z][_-]?){1,100}$, @sensitive),
//!   `agentCollaboration` (AgentCollaboration, one of: DISABLED | SUPERVISOR |
//!   SUPERVISOR_ROUTER), `orchestrationType` (OrchestrationType, one of:
//!   CUSTOM_ORCHESTRATION | DEFAULT), `inputText` (InputText, len ..25000000, @sensitive),
//!   `endSession` (Boolean), `enableTrace` (Boolean).
//! * The structured members — `actionGroups`, `knowledgeBases`, `guardrailConfiguration`,
//!   `promptOverrideConfiguration`, `collaboratorConfigurations`, `collaborators`,
//!   `inlineSessionState`, `streamingConfigurations`, `promptCreationConfigurations`,
//!   `bedrockModelConfigurations` and `customOrchestration` — are the ones
//!   `super::types::invoke_inline_agent_types` lists as unsupported.
//!
//! ## Output
//!
//! * `completion` (InlineAgentResponseStream, @httpPayload, @streaming) **required** — the
//!   event stream carrying the agent's answer.
//! * `contentType` (@httpHeader x-amzn-bedrock-agent-content-type) **required**,
//!   `sessionId` (@httpHeader x-amz-bedrock-agent-session-id) **required**.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403), `BadGatewayException` (HTTP 502),
//!   `ConflictException` (HTTP 409), `DependencyFailedException` (HTTP 424),
//!   `InternalServerException` (HTTP 500), `ResourceNotFoundException` (HTTP 404),
//!   `ServiceQuotaExceededException` (HTTP 400), `ThrottlingException` (HTTP 429),
//!   `ValidationException` (HTTP 400) — input validation failed.

#[allow(unused_imports)]
use super::types::*;

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema_server::ProtocolError;
use service_common::event_stream::EventStreamOperation;
use smithy_server_runtime::{BoxFuture, OperationContext, Refusal, Service};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `InvokeInlineAgentError` alias below.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentruntime#InvokeInlineAgent")]
#[derive(Debug)]
pub enum InvokeInlineAgentErrorModeled {
    AccessDeniedException(AccessDeniedException),
    BadGatewayException(BadGatewayException),
    ConflictException(ConflictException),
    DependencyFailedException(DependencyFailedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::invoke_inline_agent::InvokeInlineAgentError`). Wrapping the modeled enum
/// is what lets a scaffolded operation answer 501.
pub type InvokeInlineAgentError = crate::scaffolded::Scaffolded<InvokeInlineAgentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant.
impl From<InvokeInlineAgentErrorModeled> for InvokeInlineAgentError {
    fn from(modeled: InvokeInlineAgentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `InvokeInlineAgent` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct InvokeInlineAgent<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> InvokeInlineAgent<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for InvokeInlineAgent<Eng>
{
    type Input = InvokeInlineAgentInput;
    type Output = InvokeInlineAgentOutput;
    type Error = InvokeInlineAgentError;

    const NAME: &'static str = "InvokeInlineAgent";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::INVOKE_INLINE_AGENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::INVOKE_INLINE_AGENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The engine is reachable from `&self` (constructor-injected); `ctx` carries
        // request metadata. Both are bound here so an implementer has them in scope.
        let _ = (&input, ctx, &self.engine);
        // Implement `InvokeInlineAgent` here — the agent's `completion` event stream —
        // then delete the SCAFFOLDED marker at the top of this file. Return modeled
        // failures as `Err(InvokeInlineAgentErrorModeled::Variant(..).into())`.
        //
        // Modeled constraints are already refused BEFORE this handler runs, so reaching
        // here means the input satisfies every one of them: a malformed `foundationModel`,
        // `instruction` or `sessionId` never gets this far.
        Err(crate::scaffolded::Scaffolded::not_implemented(
            "InvokeInlineAgent",
        ))
    }
}

/// The literal first segment of this operation's modelled URI, `POST /agents/{sessionId}`.
const AGENTS_SEGMENT: &str = "agents";

/// Whether `req` addresses this operation's modelled route: `POST` on the one literal
/// segment plus the single `sessionId` label.
///
/// The seam that routes it ([`crate::service::EmulatedService::override_operation`]) asks
/// this, so the path this operation recognises is stated once, beside the operation. WHICH
/// Bedrock service claims such a request at all is `service_common::bedrock`'s: the
/// build-time plane binds `POST /agents/{agentId}/` on the same two segments, and the claim
/// rule splits the two.
pub fn addresses(req: &Request) -> bool {
    if req.method() != "POST" {
        return false;
    }
    let path = service_common::uri_path(req.uri());
    let mut segments = path.trim_start_matches('/').split('/');
    let (Some(AGENTS_SEGMENT), Some(label), None) =
        (segments.next(), segments.next(), segments.next())
    else {
        return false;
    };
    !label.is_empty()
}

/// Serving this operation is hand-authored because the generated `ErasedOperation` shim of
/// a `@streaming`-output operation declines the request (it cannot drive
/// `serialize_as_event_stream<P>` through an object-safe `run`).
///
/// What is served is the REQUEST half: the model's own constraints over this operation's own
/// input, through the framework's deserializer and constraint walk
/// (`service_common::validation::refusing_violations`), refused in this service's own
/// wording (`Service::override_refusal`, so a refusal here reads exactly like one from a
/// non-streaming operation of the same service). A request that satisfies the model reaches
/// the part that is NOT written — the `completion` event stream — and gets the scaffold's
/// 501, the same answer every other unimplemented operation of this service gives.
impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> EventStreamOperation
    for InvokeInlineAgent<Eng>
{
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let _ = ctx;
            let definition = svc.definition();
            let violations = service_common::validation::refusing_violations::<Self>(
                definition.protocol(),
                req,
            );
            if !violations.is_empty() {
                let chosen = svc.override_refusal(&Refusal::Invalid(violations.clone()));
                return definition.refusal_response(
                    ProtocolError::Invalid(violations),
                    chosen,
                    req,
                );
            }
            service_common::event_stream::error_response(
                definition.protocol(),
                &crate::scaffolded::Scaffolded::<InvokeInlineAgentErrorModeled>::not_implemented(
                    "InvokeInlineAgent",
                ),
            )
        })
    }
}

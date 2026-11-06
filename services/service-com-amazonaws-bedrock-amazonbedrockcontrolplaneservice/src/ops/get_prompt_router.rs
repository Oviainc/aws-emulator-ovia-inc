// SPDX-License-Identifier: CC0-1.0

//! @op GetPromptRouter
//!
//! Reports the prompt router a `PromptRouterArn` names: a `custom` router out of what
//! `ops/create_prompt_router.rs` persisted in [`crate::state`], and a `default` one out of
//! the catalogue Amazon Bedrock publishes itself
//! ([`crate::state::default_prompt_routers`], the same rows `ops/list_prompt_routers.rs`
//! reports) — its name, description, target models, fallback model, routing criteria,
//! status, type and the two instants either way.
//! A read, and only a read — nothing in the store changes here.
//!
//! An ARN neither the store nor the published catalogue carries is
//! `ResourceNotFoundException`, which is also what an ARN of another region gets: a stored
//! router is keyed by the ARN it was minted with, a published one carries the ARN of the
//! region it is offered in, and either ARN carries the region it belongs to.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetPromptRouter`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Retrieves details about a prompt router.
//!
//! ## Input
//!
//! * `promptRouterArn` (PromptRouterArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:default-prompt-router/[a-zA-Z0-9-:.]+$)
//!   **required** — The prompt router's ARN
//!
//! ## Output
//!
//! * `createdAt` (Timestamp) — When the router was created.
//! * `description` (PromptRouterDescription, len 1..200, pattern ^([0-9a-zA-Z:.][ _-]?)+$) —
//!   The router's description.
//! * `fallbackModel` (PromptRouterTargetModel) **required** — The router's fallback model.
//! * `models` (PromptRouterTargetModels) **required** — The router's models.
//! * `promptRouterArn` (PromptRouterArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:default-prompt-router/[a-zA-Z0-9-:.]+$)
//!   **required** — The prompt router's ARN
//! * `promptRouterName` (PromptRouterName, len 1..64, pattern ^([0-9a-zA-Z][ _-]?)+$)
//!   **required** — The router's name.
//! * `routingCriteria` (RoutingCriteria) **required** — The router's routing criteria.
//! * `status` (PromptRouterStatus, one of: AVAILABLE) **required** — The router's status.
//! * `type` (PromptRouterType, one of: custom | default) **required** — The router's type.
//! * `updatedAt` (Timestamp) — When the router was updated.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetPromptRouterError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetPromptRouterError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetPromptRouter")]
#[derive(Debug)]
pub enum GetPromptRouterErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_prompt_router::GetPromptRouterError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetPromptRouterError = crate::scaffolded::Scaffolded<GetPromptRouterErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetPromptRouterErrorModeled> for GetPromptRouterError {
    fn from(modeled: GetPromptRouterErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetPromptRouter` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetPromptRouter<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetPromptRouter<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetPromptRouter<Eng>
{
    type Input = GetPromptRouterInput;
    type Output = GetPromptRouterOutput;
    type Error = GetPromptRouterError;

    const NAME: &'static str = "GetPromptRouter";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PROMPT_ROUTER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PROMPT_ROUTER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetPromptRouterErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "prompt router state is not available on this engine".to_string(),
                    ),
                },
            )
            .into());
        };
        let store = state.lock();

        let Some(router) = store
            .prompt_router(&input.prompt_router_arn)
            .map(crate::state::prompt_router_summary)
            .or_else(|| {
                crate::state::default_prompt_router(&input.prompt_router_arn, &region)
            })
        else {
            return Err(GetPromptRouterErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::prompt_router_not_found_message(
                        &input.prompt_router_arn,
                    )),
                },
            )
            .into());
        };

        Ok(GetPromptRouterOutput {
            prompt_router_name: router.prompt_router_name,
            routing_criteria: router.routing_criteria,
            description: router.description,
            created_at: router.created_at,
            updated_at: router.updated_at,
            prompt_router_arn: router.prompt_router_arn,
            models: router.models,
            fallback_model: router.fallback_model,
            status: router.status,
            r#type: router.r#type,
        })
    }
}

// SPDX-License-Identifier: CC0-1.0

//! @op DeletePromptRouter
//!
//! Removes ONE stored prompt router from [`crate::state`] — the router
//! `ops/create_prompt_router.rs` minted — together with its name index entry and the
//! idempotency token that minted it
//! ([`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::remove_prompt_router`]),
//! so a later create of that name starts fresh and a later read of the router reports it
//! gone. "This action cannot be undone": the record is dropped rather than moved to a
//! deleting status, and the model carries no status for one.
//!
//! The rules, each read off this operation's own member
//! (`smithy_query(query="operation",operation="DeletePromptRouter")`):
//!
//! * `promptRouterArn` is the one identifier the request carries, and the same
//!   `PromptRouterArn` `GetPromptRouter` addresses a router by, so this call deletes
//!   exactly the router a read at the same instant would report. An ARN that resolves to
//!   no prompt router of this account and Region — including one a concurrent delete has
//!   already removed — is 404 `ResourceNotFoundException`
//!   ([`crate::state::prompt_router_not_found_message`]), so of two deletes racing for one
//!   router exactly one deletes it and the other finds nothing left;
//! * a router Amazon Bedrock PUBLISHES itself — the ones whose `PromptRouterType` is
//!   [`crate::state::PROMPT_ROUTER_TYPE_DEFAULT`]
//!   ([`crate::state::default_prompt_router`]) — belongs to the service and not to the
//!   account, so deleting one is refused with 400 `ValidationException`
//!   ([`crate::state::DEFAULT_PROMPT_ROUTER_DELETE_REFUSED_MESSAGE`]) and the published
//!   router stays listed. The router is resolved FIRST, so an ARN naming nothing is still
//!   404 rather than told whose router it would have been.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeletePromptRouter`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Deletes a specified prompt router. This action cannot be undone.
//!
//! ## Input
//!
//! * `promptRouterArn` (PromptRouterArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:default-prompt-router/[a-zA-Z0-9-:.]+$)
//!   **required** — The Amazon Resource Name (ARN) of the prompt router to delete.
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
// `DeletePromptRouterError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeletePromptRouterError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#DeletePromptRouter")]
#[derive(Debug)]
pub enum DeletePromptRouterErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_prompt_router::DeletePromptRouterError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeletePromptRouterError = crate::scaffolded::Scaffolded<DeletePromptRouterErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeletePromptRouterErrorModeled> for DeletePromptRouterError {
    fn from(modeled: DeletePromptRouterErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeletePromptRouter` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeletePromptRouter<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeletePromptRouter<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeletePromptRouter<Eng>
{
    type Input = DeletePromptRouterInput;
    type Output = DeletePromptRouterOutput;
    type Error = DeletePromptRouterError;

    const NAME: &'static str = "DeletePromptRouter";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_PROMPT_ROUTER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_PROMPT_ROUTER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DeletePromptRouterErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("prompt router state is not available on this engine".to_string()),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let mut store = state.lock();

        // The ARN resolves to the router a read of it would report: one this account
        // created, or one Amazon Bedrock publishes in the Region the request addressed.
        let Some(router_type) = store
            .prompt_router(&input.prompt_router_arn)
            .map(|router| router.router_type.to_string())
            .or_else(|| {
                crate::state::default_prompt_router(&input.prompt_router_arn, &region)
                    .map(|published| published.r#type)
            })
        else {
            return Err(DeletePromptRouterErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::prompt_router_not_found_message(
                        &input.prompt_router_arn,
                    )),
                },
            )
            .into());
        };

        // A published router is the service's, not the account's, so no caller may delete
        // one — whichever of them the request named.
        if router_type == crate::state::PROMPT_ROUTER_TYPE_DEFAULT {
            return Err(DeletePromptRouterErrorModeled::ValidationException(
                ValidationException {
                    message: Some(
                        crate::state::DEFAULT_PROMPT_ROUTER_DELETE_REFUSED_MESSAGE.to_string(),
                    ),
                },
            )
            .into());
        }

        store.remove_prompt_router(&input.prompt_router_arn);
        Ok(DeletePromptRouterOutput {})
    }
}

// SPDX-License-Identifier: CC0-1.0

//! @op CreatePromptRouter
//!
//! Mints a prompt router and PERSISTS it in [`crate::state`]: the
//! `default-prompt-router/<id>` ARN for the signing region, the request's name,
//! description, target models, fallback model, routing criteria and tags, and the
//! creation instant. The ARN the caller is told is read back out of the stored record,
//! so it is the ARN `GetPromptRouter` answers for.
//!
//! Two refusals this operation makes that the modelled constraints do not:
//!
//! * `routingCriteria.responseQualityDifference` is modelled `@range(0..100)`, and Amazon
//!   Bedrock accepts only the multiples of
//!   [`crate::state::RESPONSE_QUALITY_DIFFERENCE_STEP`] inside that span, so any other
//!   value is refused with 400 `ValidationException`;
//! * `models` is documented as needing at least one entry, and `PromptRouterTargetModels`
//!   declares no `@length`, so an empty list is refused the same way.
//!
//! A replay of the same `clientRequestToken` answers with the router that token already
//! minted (`@idempotencyToken`), and a name another router already holds in the same
//! region is refused with `ConflictException`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreatePromptRouter`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Creates a prompt router that manages the routing of requests between multiple foundation
//! models based on the routing criteria.
//!
//! ## Input
//!
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier that you provide to ensure idempotency of your requests. If
//!   not specified, the Amazon Web Services SDK automatically generates one for you.
//! * `description` (PromptRouterDescription, len 1..200, pattern ^([0-9a-zA-Z:.][ _-]?)+$) —
//!   An optional description of the prompt router to help identify its purpose.
//! * `fallbackModel` (PromptRouterTargetModel) **required** — The default model to use when
//!   the routing criteria is not met.
//! * `models` (PromptRouterTargetModels) **required** — A list of foundation models that the
//!   prompt router can route requests to. At least one model must be specified.
//! * `promptRouterName` (PromptRouterName, len 1..64, pattern ^([0-9a-zA-Z][ _-]?)+$)
//!   **required** — The name of the prompt router. The name must be unique within your Amazon
//!   Web Services account in the current region.
//! * `routingCriteria` (RoutingCriteria) **required** — The criteria, which is the response
//!   quality difference, used to determine how incoming requests are routed to different
//!   models.
//! * `tags` (TagList, len 0..200) — An array of key-value pairs to apply to this resource as
//!   tags. You can use tags to categorize and manage your Amazon Web Services resources.
//!
//! ## Output
//!
//! * `promptRouterArn` (PromptRouterArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:default-prompt-router/[a-zA-Z0-9-:.]+$)
//!   — The Amazon Resource Name (ARN) that uniquely identifies the prompt router.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `ConflictException` (HTTP 400) — Error occurred because of a conflict while performing
//!   an operation.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ServiceQuotaExceededException` (HTTP 400) — The number of requests exceeds the service
//!   quota. Resubmit your request later.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `TooManyTagsException` (HTTP 400) — The request contains more tags than can be
//!   associated with a resource (50 tags per resource). The maximum number of tags includes
//!   both existing tags and those included in your current request.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreatePromptRouterError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreatePromptRouterError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#CreatePromptRouter")]
#[derive(Debug)]
pub enum CreatePromptRouterErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    TooManyTagsException(TooManyTagsException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_prompt_router::CreatePromptRouterError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreatePromptRouterError = crate::scaffolded::Scaffolded<CreatePromptRouterErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreatePromptRouterErrorModeled> for CreatePromptRouterError {
    fn from(modeled: CreatePromptRouterErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreatePromptRouter` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreatePromptRouter<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreatePromptRouter<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreatePromptRouter<Eng>
{
    type Input = CreatePromptRouterInput;
    type Output = CreatePromptRouterOutput;
    type Error = CreatePromptRouterError;

    const NAME: &'static str = "CreatePromptRouter";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_PROMPT_ROUTER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_PROMPT_ROUTER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreatePromptRouterErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("prompt router state is not available on this engine".to_string()),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();

        // Granularity, not range: `@range(0..100)` is already enforced ahead of this
        // handler, and only the multiples of the step inside that span are accepted.
        let quality_difference = input.routing_criteria.response_quality_difference;
        if !crate::state::is_accepted_response_quality_difference(quality_difference) {
            return Err(validation(format!(
                "Response quality difference must be a value multiple of {}",
                crate::state::RESPONSE_QUALITY_DIFFERENCE_STEP
            )));
        }

        // `PromptRouterTargetModels` declares no `@length`, so the "at least one model
        // must be specified" the member documents is refused here rather than at the
        // boundary.
        if input.models.is_empty() {
            return Err(validation(
                "At least one model must be specified in models".to_string(),
            ));
        }

        let tag_count = input.tags.as_ref().map_or(0, Vec::len);
        if tag_count > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(CreatePromptRouterErrorModeled::TooManyTagsException(
                TooManyTagsException {
                    message: Some(format!(
                        "a resource may carry at most {} tags, and this request carries \
                         {tag_count}",
                        crate::state::MAX_TAGS_PER_RESOURCE
                    )),
                    resource_name: None,
                },
            )
            .into());
        }

        let mut store = state.lock();

        // `@idempotencyToken`: a replay answers with what the first request minted.
        if let Some(minted) = input
            .client_request_token
            .as_ref()
            .and_then(|token| {
                store
                    .prompt_router_create_tokens
                    .get(&(region.clone(), token.clone()))
            })
            .cloned()
        {
            if let Some(existing) = store.prompt_router(&minted) {
                return Ok(CreatePromptRouterOutput {
                    prompt_router_arn: Some(existing.arn.clone()),
                });
            }
        }

        let name_key = (region.clone(), input.prompt_router_name.clone());
        if store.prompt_router_names.contains_key(&name_key) {
            return Err(CreatePromptRouterErrorModeled::ConflictException(
                ConflictException {
                    message: Some(format!(
                        "a prompt router named {} already exists in this account in {region}",
                        input.prompt_router_name
                    )),
                },
            )
            .into());
        }

        // The account's prompt router quota. Enforcing it here is also what bounds this
        // part of the store: without a server-side cap a caller could grow it with
        // nothing but fresh names.
        if store.prompt_routers.len() >= crate::state::MAX_PROMPT_ROUTERS_PER_ACCOUNT {
            return Err(CreatePromptRouterErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(format!(
                        "this account already holds the maximum of {} prompt routers; delete \
                         one or request a quota increase",
                        crate::state::MAX_PROMPT_ROUTERS_PER_ACCOUNT
                    )),
                },
            )
            .into());
        }

        let id = store.mint_prompt_router_id(now);
        let arn = crate::state::prompt_router_arn(&region, &id);
        let router = crate::state::PromptRouter {
            arn: arn.clone(),
            name: input.prompt_router_name,
            description: input.description,
            models: input.models,
            fallback_model: input.fallback_model,
            routing_criteria: input.routing_criteria,
            tags: input.tags.unwrap_or_default(),
            status: crate::state::PROMPT_ROUTER_AVAILABLE,
            router_type: crate::state::PROMPT_ROUTER_TYPE_CUSTOM,
            created_at: now,
            updated_at: now,
        };

        store.prompt_router_names.insert(name_key, arn.clone());
        if let Some(token) = input.client_request_token {
            store
                .prompt_router_create_tokens
                .insert((region, token), arn.clone());
        }
        store.prompt_routers.insert(arn.clone(), router);

        // Read the response back out of the STORED record, so what the caller is told is
        // exactly what a later GetPromptRouter will find.
        let stored = store
            .prompt_router(&arn)
            .expect("the prompt router just inserted under this ARN");
        Ok(CreatePromptRouterOutput {
            prompt_router_arn: Some(stored.arn.clone()),
        })
    }
}

/// Refuse the request with the modelled 400 `ValidationException` this operation
/// declares, rather than the synthetic one in `crate::scaffolded`: the model's own shape
/// is what a client of this service parses.
fn validation(message: String) -> CreatePromptRouterError {
    CreatePromptRouterErrorModeled::ValidationException(ValidationException {
        message: Some(message),
    })
    .into()
}

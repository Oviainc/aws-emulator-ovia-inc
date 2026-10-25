// SPDX-License-Identifier: CC0-1.0

//! @op CreateCustomModelDeployment
//!
//! Deploys a custom model for on-demand inference: it mints the
//! `customModelDeploymentArn` every other custom model deployment operation addresses,
//! writes the name, the custom model and the description the request carried into
//! [`crate::state`], and answers that ARN.
//!
//! The rules it enforces, each out of the model or out of what Amazon Bedrock grants
//! rather than out of one request:
//!
//! * `modelArn` names the custom model to deploy, and a `CustomModelArn` EMBEDS the base
//!   model the custom model was customized from ([`base_model_segment`]). A segment that
//!   names no model the account's published catalog carries
//!   ([`crate::state::FoundationModelCatalog::qualified_model`]) is no foundation model to
//!   serve inference with, so the create is refused synchronously with 400
//!   `ValidationException` naming that base model ([`invalid_base_model_message`]). The
//!   [`crate::state::CUSTOM_MODEL_IMPORTED_BASE`] segment names a custom model whose
//!   artifacts were brought in, which has no base model in the catalog, so it carries no
//!   such requirement.
//! * `modelDeploymentName` "must be unique within your Amazon Web Services account and
//!   Region", and it is the other spelling of a `CustomModelDeploymentIdentifier`
//!   (`smithy_query(query="operation",operation="GetCustomModelDeployment")`), so two live
//!   deployments of one name in one region could not both be addressed by it: a second
//!   create of a live name is refused with 400 `ValidationException`.
//! * `tags` past what a resource may carry ([`crate::state::MAX_TAGS_PER_RESOURCE`]) is
//!   refused with 400 `TooManyTagsException`, the error the model declares for it.
//! * `@idempotencyToken`: a replayed `clientRequestToken` answers with the ARN the first
//!   request minted and deploys nothing a second time.
//!
//! Amazon Bedrock brings the deployment up after this call returns (the operation answers
//! 202), so it is stored [`crate::state::CUSTOM_MODEL_DEPLOYMENT_STATUS_CREATING`] and
//! owed [`crate::state::CUSTOM_MODEL_DEPLOYMENT_STATUS_ACTIVE`]
//! [`crate::state::CUSTOM_MODEL_DEPLOYMENT_SETTLE_AFTER`] from now.
//!
//! An ARN naming another region is refused ahead of this handler by the service's own ARN
//! check (`crate::service`), which covers every member of this service that carries a
//! `custom-model` ARN.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateCustomModelDeployment`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Deploys a custom model for on-demand inference in Amazon Bedrock. After you deploy your
//! custom model, you use the deployment's Amazon Resource Name (ARN) as the modelId
//! parameter when you submit prompts and generate responses with model inference. For more
//! information about setting up on-demand inference for custom models, see Set up inference
//! for a custom model. The following actions are related to the CreateCustomModelDeployment
//! operation: GetCustomModelDeployment ListCustomModelDeployments
//! DeleteCustomModelDeployment
//!
//! ## Input
//!
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the operation completes no more than one time.
//!   If this token matches a previous request, Amazon Bedrock ignores the request, but does
//!   not return an error. For more information, see Ensuring idempotency.
//! * `description` (CustomModelDeploymentDescription, len 1..2048, pattern ^.*$) — A
//!   description for the custom model deployment to help you identify its purpose.
//! * `modelArn` (CustomModelArn, len 20..1011, pattern
//!   ^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model/(imported|[a-z0-9-]{1,63}[.]{1}[a-z0-9
//!   …) **required** — The Amazon Resource Name (ARN) of the custom model to deploy for
//!   on-demand inference. The custom model must be in the Active state.
//! * `modelDeploymentName` (ModelDeploymentName, len 1..63, pattern
//!   ^([0-9a-zA-Z][_-]?){1,63}$) **required** — The name for the custom model deployment. The
//!   name must be unique within your Amazon Web Services account and Region.
//! * `tags` (TagList, len 0..200) — Tags to assign to the custom model deployment. You can
//!   use tags to organize and track your Amazon Web Services resources for cost allocation
//!   and management purposes.
//!
//! ## Output
//!
//! * `customModelDeploymentArn` (CustomModelDeploymentArn, len ..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model-deployment/[a-z0-9]{12}$)
//!   **required** — The Amazon Resource Name (ARN) of the custom model deployment. Use this
//!   ARN as the modelId parameter when invoking the model with the InvokeModel or Converse
//!   operations.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
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
// `CreateCustomModelDeploymentError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateCustomModelDeploymentError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#CreateCustomModelDeployment")]
#[derive(Debug)]
pub enum CreateCustomModelDeploymentErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    TooManyTagsException(TooManyTagsException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_custom_model_deployment::CreateCustomModelDeploymentError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateCustomModelDeploymentError = crate::scaffolded::Scaffolded<CreateCustomModelDeploymentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateCustomModelDeploymentErrorModeled> for CreateCustomModelDeploymentError {
    fn from(modeled: CreateCustomModelDeploymentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateCustomModelDeployment` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateCustomModelDeployment<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateCustomModelDeployment<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The base-model segment a `CustomModelArn` embeds: what sits between the
/// `custom-model/` resource type and the custom model's own id, which is either
/// [`crate::state::CUSTOM_MODEL_IMPORTED_BASE`] or the id of the foundation model the
/// custom model was customized from, with the version qualifiers AWS appends to it.
///
/// `None` for a value that is not a `custom-model` ARN of this service's namespace at all;
/// such a value is refused by the member's own modelled constraints before this handler
/// sees it. Read here rather than in `crate::state` because this is the only operation
/// that decides anything from the segment.
///
/// A TOTAL read of the value: the shared six-field ARN parse
/// (`service_common::arn::parse_of`, which answers `None` for anything that is not an ARN
/// of this service's namespace), then `strip_prefix` and `rsplit_once`, each answering
/// `None` rather than panicking. Nothing is indexed, sliced or unwrapped, so no value a
/// caller can send reaches a panic here.
fn base_model_segment(model_arn: &str) -> Option<&str> {
    let arn = ::service_common::arn::parse_of(model_arn, ::service_common::bedrock::ARN_NAMESPACE)?;
    let rest = arn.resource.strip_prefix("custom-model/")?;
    let (segment, id) = rest.rsplit_once('/')?;
    (!segment.is_empty() && !id.is_empty()).then_some(segment)
}

/// What this operation answers as its `ValidationException` message for a `CustomModelArn`
/// whose base-model segment names no foundation model the published catalog carries.
///
/// RECORDED behaviour, and a message BUILT FROM THE REQUEST rather than a constant: the
/// corpus records Amazon Bedrock naming the base model the submitted ARN embedded
/// ([`base_model_segment`]), quoted, in exactly this sentence. So the rule is "name the
/// base model the ARN embeds", not one recorded model id.
fn invalid_base_model_message(base_model_segment: &str) -> String {
    format!(
        "The foundation model '{base_model_segment}' for custom model is not a valid \
         foundation model"
    )
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateCustomModelDeployment<Eng>
{
    type Input = CreateCustomModelDeploymentInput;
    type Output = CreateCustomModelDeploymentOutput;
    type Error = CreateCustomModelDeploymentError;

    const NAME: &'static str = "CreateCustomModelDeployment";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CUSTOM_MODEL_DEPLOYMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CUSTOM_MODEL_DEPLOYMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                CreateCustomModelDeploymentErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "custom model deployment state is not available on this engine"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // `@idempotencyToken`: a replay answers with what the first request minted.
        let replayed = input
            .client_request_token
            .as_ref()
            .and_then(|token| {
                store
                    .custom_model_deployment_create_tokens
                    .get(&(region.clone(), token.clone()))
            })
            .cloned();
        if let Some(minted) = replayed {
            if store.custom_model_deployments.contains_key(&minted) {
                return Ok(CreateCustomModelDeploymentOutput {
                    custom_model_deployment_arn: minted,
                });
            }
        }

        let tags = input.tags.unwrap_or_default();
        if tags.len() > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(
                CreateCustomModelDeploymentErrorModeled::TooManyTagsException(
                    TooManyTagsException {
                        message: Some(format!(
                            "a resource may carry at most {} tags, and this request carries {}",
                            crate::state::MAX_TAGS_PER_RESOURCE,
                            tags.len()
                        )),
                        resource_name: None,
                    },
                )
                .into(),
            );
        }

        // The base model a `CustomModelArn` embeds has to be one the account's published
        // catalog carries: nothing else can serve on-demand inference. An imported custom
        // model is customized from no base model, so it is exempt.
        if let Some(segment) = base_model_segment(&input.model_arn) {
            if segment != crate::state::CUSTOM_MODEL_IMPORTED_BASE
                && store.foundation_models.qualified_model(segment).is_none()
            {
                return Err(
                    CreateCustomModelDeploymentErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(invalid_base_model_message(segment)),
                        },
                    )
                    .into(),
                );
            }
        }

        if store
            .custom_model_deployment_names
            .contains_key(&(region.clone(), input.model_deployment_name.clone()))
        {
            return Err(
                CreateCustomModelDeploymentErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(format!(
                            "modelDeploymentName {} is already used by a custom model \
                             deployment in this account in {region}",
                            input.model_deployment_name
                        )),
                    },
                )
                .into(),
            );
        }

        // The account's deployment quota. Enforcing it here is also what bounds this part
        // of the store: without a server-side cap a caller could grow it with nothing but
        // fresh names.
        if store.custom_model_deployments.len()
            >= crate::state::MAX_CUSTOM_MODEL_DEPLOYMENTS_PER_ACCOUNT
        {
            return Err(
                CreateCustomModelDeploymentErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "this account already holds the maximum of {} custom model \
                             deployments; delete one or request a quota increase",
                            crate::state::MAX_CUSTOM_MODEL_DEPLOYMENTS_PER_ACCOUNT
                        )),
                    },
                )
                .into(),
            );
        }

        let id = store.mint_custom_model_deployment_id(now);
        let arn = crate::state::custom_model_deployment_arn(&region, &id);
        let record = crate::state::CustomModelDeploymentRecord {
            arn: arn.clone(),
            name: input.model_deployment_name.clone(),
            region: region.clone(),
            model_arn: input.model_arn,
            description: input.description,
            tags,
            status: crate::state::CUSTOM_MODEL_DEPLOYMENT_STATUS_CREATING,
            active_due_at: Some(now + crate::state::CUSTOM_MODEL_DEPLOYMENT_SETTLE_AFTER),
            created_at: now,
            // Nothing has changed the deployment since this call created it.
            last_updated_at: None,
        };

        if let Some(token) = input.client_request_token {
            store
                .custom_model_deployment_create_tokens
                .insert((region.clone(), token), arn.clone());
        }
        store
            .custom_model_deployment_names
            .insert((region, input.model_deployment_name), arn.clone());
        store.custom_model_deployments.insert(arn.clone(), record);

        Ok(CreateCustomModelDeploymentOutput {
            custom_model_deployment_arn: arn,
        })
    }
}

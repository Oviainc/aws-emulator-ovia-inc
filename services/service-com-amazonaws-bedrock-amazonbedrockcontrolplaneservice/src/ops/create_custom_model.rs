// SPDX-License-Identifier: CC0-1.0

//! @op CreateCustomModel
//!
//! Mints one custom model from the model artifacts the request names, and stores it in
//! [`crate::state`] for `GetCustomModel` to read back and `ListCustomModels` to report.
//!
//! The rules, each read off this operation's own members and documentation
//! (`smithy_query(query="operation",operation="CreateCustomModel")`,
//! `smithy_query(query="doc",shape="CreateCustomModel")`) rather than off one request:
//!
//! * the artifacts are named ONE way: "You can specify either `customModelDataSource` or
//!   `modelSourceConfig`, but not both", so a request carrying neither or both is refused
//!   with 400 `ValidationException`;
//! * `roleArn` "is required when you use `modelSourceConfig` with an Amazon S3 data
//!   source. It is not required when you use `customModelDataSource` with a model package
//!   ARN", so the role is demanded exactly where the member says it is;
//! * `modelName` is "a unique name for the custom model", so a second create of a name a
//!   live model in this account and region already holds is refused with
//!   `ConflictException`, and `@idempotencyToken` makes a replayed `clientRequestToken`
//!   answer with the ARN the first request minted rather than minting a second model;
//! * the model it mints "appears in `ListCustomModels` with a `customizationType` of
//!   `imported`" and starts in the non-terminal `Creating` state, which the clock owes its
//!   move to `Active` ([`crate::state::CUSTOM_MODEL_ACTIVE_AFTER`]) — nothing here trains
//!   or validates the artifacts, so nothing beyond that progression is reported.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateCustomModel`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Creates a new custom model in Amazon Bedrock. After the model is active, you can use it
//! for inference. You can provide the model data source in one of the following ways:
//! customModelDataSource — Specify a SageMaker AI model package ARN. Amazon Bedrock
//! resolves the model package to retrieve the model artifacts. This is the preferred method
//! for new SageMaker AI training outputs. modelSourceConfig — Specify an Amazon S3 URI
//! pointing to the Amazon-managed Amazon S3 bucket containing your model artifacts. To use
//! the model for inference, you must purchase Provisioned Throughput for it. You can't use
//! On-demand inference with these custom models. For more information about Provisioned
//! Throughput, see Provisioned Throughput. The model appears in ListCustomModels with a
//! customizationType of imported. To track the status of the new model, you use the
//! GetCustomModel API operation. The model can be in the following states: Creating -
//! Initial state during validation and registration Active - Model is ready for use in
//! inference Failed - Creation process encountered an error Related APIs GetCustomModel
//! ListCustomModels DeleteCustomModel
//!
//! ## Input
//!
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the API request completes no more than one
//!   time. If this token matches a previous request, Amazon Bedrock ignores the request, but
//!   does not return an error. For more information, see Ensuring idempotency.
//! * `customModelDataSource` (CustomModelDataSource) — The data source for the custom model.
//!   Use this field to specify a SageMaker AI model package ARN as the source for your custom
//!   model. Amazon Bedrock resolves the model package to retrieve the model artifacts. You
//!   can specify either customModelDataSource or modelSourceConfig, but not both.
//! * `modelKmsKeyArn` (KmsKeyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$) — The Amazon
//!   Resource Name (ARN) of the customer managed KMS key to encrypt the custom model. If you
//!   don't provide a KMS key, Amazon Bedrock uses an Amazon Web Services-managed KMS key to
//!   encrypt the model. If you provide a customer managed KMS key, your Amazon Bedrock
//!   service role must have permissions to use it. For more information see Encryption of
//!   imported models.
//! * `modelName` (CustomModelName, len 1..63, pattern ^([0-9a-zA-Z][_-]?){1,63}$)
//!   **required** — A unique name for the custom model.
//! * `modelSourceConfig` (ModelDataSource) — The data source for the model. The Amazon S3 URI
//!   in the model source must be for the Amazon-managed Amazon S3 bucket containing your
//!   model artifacts.
//! * `modelTags` (TagList, len 0..200) — A list of key-value pairs to associate with the
//!   custom model resource. You can use these tags to organize and identify your resources.
//!   For more information, see Tagging resources in the Amazon Bedrock User Guide.
//! * `roleArn` (RoleArn, len ..2048, pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$) —
//!   The Amazon Resource Name (ARN) of an IAM service role that Amazon Bedrock assumes to
//!   perform tasks on your behalf. This role must have permissions to access the Amazon S3
//!   bucket containing your model artifacts and the KMS key (if specified). For more
//!   information, see Setting up an IAM service role for importing models in the Amazon
//!   Bedrock User Guide. This field is required when you use modelSourceConfig with an Amazon
//!   S3 data source. It is not required when you use customModelDataSource with a model
//!   package ARN, because Amazon Bedrock uses its own credentials to access the model
//!   artifacts.
//!
//! ## Output
//!
//! * `modelArn` (ModelArn, len 20..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/((imported)|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}))((
//!   …) **required** — The Amazon Resource Name (ARN) of the new custom model.
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
// `CreateCustomModelError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateCustomModelError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#CreateCustomModel")]
#[derive(Debug)]
pub enum CreateCustomModelErrorModeled {
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
/// (`crate::ops::create_custom_model::CreateCustomModelError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateCustomModelError = crate::scaffolded::Scaffolded<CreateCustomModelErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateCustomModelErrorModeled> for CreateCustomModelError {
    fn from(modeled: CreateCustomModelErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateCustomModel` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateCustomModel<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateCustomModel<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateCustomModel<Eng>
{
    type Input = CreateCustomModelInput;
    type Output = CreateCustomModelOutput;
    type Error = CreateCustomModelError;

    const NAME: &'static str = "CreateCustomModel";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CUSTOM_MODEL_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CUSTOM_MODEL_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreateCustomModelErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("custom model state is not available on this engine".to_string()),
                },
            )
            .into());
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
                    .custom_model_create_tokens
                    .get(&(region.clone(), token.clone()))
            })
            .cloned();
        if let Some(minted) = replayed {
            if store.custom_models.contains_key(&minted) {
                return Ok(CreateCustomModelOutput { model_arn: minted });
            }
        }

        // "You can specify either customModelDataSource or modelSourceConfig, but not
        // both": the artifacts are named exactly one way.
        match (
            input.model_source_config.as_ref(),
            input.custom_model_data_source.as_ref(),
        ) {
            (None, None) => {
                return Err(CreateCustomModelErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(
                            "either modelSourceConfig or customModelDataSource must be \
                             specified to name the model artifacts"
                                .to_string(),
                        ),
                    },
                )
                .into());
            }
            (Some(_), Some(_)) => {
                return Err(CreateCustomModelErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(
                            "either modelSourceConfig or customModelDataSource may be \
                             specified, but not both"
                                .to_string(),
                        ),
                    },
                )
                .into());
            }
            // `roleArn` "is required when you use modelSourceConfig with an Amazon S3 data
            // source", because Amazon Bedrock reads that bucket as the caller; with a
            // model package source it uses its own credentials and needs no role.
            (Some(_), None) if input.role_arn.is_none() => {
                return Err(CreateCustomModelErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(
                            "roleArn is required when the model artifacts are named by \
                             modelSourceConfig"
                                .to_string(),
                        ),
                    },
                )
                .into());
            }
            _ => {}
        }

        let tags = input.model_tags.unwrap_or_default();
        if tags.len() > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(CreateCustomModelErrorModeled::TooManyTagsException(
                TooManyTagsException {
                    message: Some(format!(
                        "a resource may carry at most {} tags, and this request carries {}",
                        crate::state::MAX_TAGS_PER_RESOURCE,
                        tags.len()
                    )),
                    resource_name: None,
                },
            )
            .into());
        }

        // A model name names at most one custom model of this account and region — the
        // model this call would mint, and the one a customization job is already
        // customizing under that name, which is the same resource reached before it exists.
        if store
            .custom_model_names
            .contains_key(&(region.clone(), input.model_name.clone()))
            || store.customizes_model_named(&region, &input.model_name)
        {
            return Err(CreateCustomModelErrorModeled::ConflictException(
                ConflictException {
                    message: Some(format!(
                        "modelName {} is already used by a custom model in this account in \
                         {region}",
                        input.model_name
                    )),
                },
            )
            .into());
        }

        // The account's custom model quota. Enforcing it here is also what bounds this
        // part of the store: without a server-side cap a caller could grow it with nothing
        // but fresh names — or with fresh regions, which is why the count is of every
        // stored model rather than of this region's.
        if store.custom_models.len() >= crate::state::MAX_CUSTOM_MODELS_PER_ACCOUNT {
            return Err(
                CreateCustomModelErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "this account already holds the maximum of {} custom models; \
                             delete one or request a quota increase",
                            crate::state::MAX_CUSTOM_MODELS_PER_ACCOUNT
                        )),
                    },
                )
                .into(),
            );
        }

        let id = store.mint_custom_model_id(now);
        // The artifacts were brought in rather than customized from a foundation model, so
        // the ARN carries the base segment `CustomModelArn` reserves for that.
        let arn =
            crate::state::custom_model_arn(&region, crate::state::CUSTOM_MODEL_IMPORTED_BASE, &id);
        let record = crate::state::CustomModelRecord {
            arn: arn.clone(),
            name: input.model_name.clone(),
            region: region.clone(),
            // This model was customized from no other model: the request names artifacts,
            // so the model is its own base and no foundation model is attributed to it.
            base_model_arn: arn.clone(),
            base_model_name: crate::state::CUSTOM_MODEL_IMPORTED_BASE.to_string(),
            foundation_model_arn: None,
            // No customization job produced this model: the request named its artifacts.
            job_arn: None,
            customization_type: crate::state::CUSTOMIZATION_TYPE_IMPORTED,
            model_source_config: input.model_source_config,
            custom_model_data_source: input.custom_model_data_source,
            model_kms_key_arn: input.model_kms_key_arn,
            role_arn: input.role_arn,
            tags,
            status: crate::state::CUSTOM_MODEL_STATUS_CREATING,
            active_due_at: Some(now + crate::state::CUSTOM_MODEL_ACTIVE_AFTER),
            created_at: now,
        };

        if let Some(token) = input.client_request_token {
            store
                .custom_model_create_tokens
                .insert((region.clone(), token), arn.clone());
        }
        store
            .custom_model_names
            .insert((region, input.model_name), arn.clone());
        store.custom_models.insert(arn.clone(), record);

        Ok(CreateCustomModelOutput { model_arn: arn })
    }
}

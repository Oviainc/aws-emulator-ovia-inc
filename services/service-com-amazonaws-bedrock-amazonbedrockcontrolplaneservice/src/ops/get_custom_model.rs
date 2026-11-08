// SPDX-License-Identifier: CC0-1.0

//! @op GetCustomModel
//!
//! Reports the properties of ONE stored custom model out of [`crate::state`], which is
//! how the status of a model `CreateCustomModel` minted is tracked ("To track the status
//! of the new model, you use the `GetCustomModel` API operation",
//! `smithy_query(query="doc",shape="CreateCustomModel")`).
//!
//! The rules, each read off this operation's own members
//! (`smithy_query(query="operation",operation="GetCustomModel")`):
//!
//! * the caller has to be ALLOWED to perform the action: IAM decides that ahead of this
//!   service resolving the identifier, so a principal whose identity-based policies do not
//!   grant `bedrock:GetCustomModel` on the model the request addressed — or that an
//!   explicit `Deny` covers — is answered 403 `AccessDeniedException`
//!   ([`crate::service::authorization`]) rather than told whether that model exists;
//! * `modelIdentifier` is the "Name or Amazon Resource Name (ARN) of the custom model", so
//!   both spellings address the same model
//!   ([`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::custom_model`]);
//!   an unresolvable one is refused as the spelling it IS — `ResourceNotFoundException`
//!   (404) for an ARN, which is what that error documents ("The specified resource Amazon
//!   Resource Name (ARN) was not found"), carrying this resource's own not-found sentence
//!   ([`crate::state::CUSTOM_MODEL_NOT_FOUND_MESSAGE`]) — the sentence RECORDED for a
//!   `DeleteCustomModel` of a model this account does not hold, carried over here because it
//!   is a sentence about the RESOURCE rather than about one operation and no recording of
//!   this read words it differently; while a bare NAME that
//!   resolves to no model of the signing region is no ARN to report missing but an
//!   identifier this service could not resolve, so it is 400 `ValidationException`;
//! * every member reported comes out of the stored record, and the members that describe a
//!   customization JOB are absent for a model created from artifacts — `jobArn` states it:
//!   "For models that you create with the `CreateCustomModel` API operation, this is
//!   NULL".
//!
//! `@readonly`, and the one write it makes is the status transition the CLOCK already made
//! due for the record ([`crate::state::CustomModelRecord::settle`]): the status this read
//! reports is the one the record holds now, which is the status any caller would see at
//! the same instant whether or not anybody read it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetCustomModel`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Get the properties associated with a Amazon Bedrock custom model that you have created.
//! For more information, see Custom models in the Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `modelIdentifier` (ModelIdentifier, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/((imported)|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}))((
//!   …) **required** — Name or Amazon Resource Name (ARN) of the custom model.
//!
//! ## Output
//!
//! * `baseModelArn` (ModelArn, len 20..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/((imported)|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}))((
//!   …) — Amazon Resource Name (ARN) of the base model.
//! * `creationTime` (Timestamp) **required** — Creation time of the model.
//! * `customizationConfig` (CustomizationConfig) — The customization configuration for the
//!   custom model.
//! * `customizationType` (CustomizationType, one of: CONTINUED_PRE_TRAINING | DISTILLATION |
//!   FINE_TUNING | IMPORTED | REINFORCEMENT_FINE_TUNING) — The type of model customization.
//! * `failureMessage` (ErrorMessage, len ..2048) — A failure message for any issues that
//!   occurred when creating the custom model. This is included for only a failed
//!   CreateCustomModel operation.
//! * `hyperParameters` (ModelCustomizationHyperParameters) — Hyperparameter values associated
//!   with this model. For details on the format for different models, see Custom model
//!   hyperparameters.
//! * `jobArn` (ModelCustomizationJobArn, len ..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-customization-job/[a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}([a-z0-9
//!   …) — Job Amazon Resource Name (ARN) associated with this model. For models that you
//!   create with the CreateCustomModel API operation, this is NULL.
//! * `jobName` (JobName, len 1..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9\+\-\.])*$) — Job name
//!   associated with this model.
//! * `modelArn` (ModelArn, len 20..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/((imported)|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}))((
//!   …) **required** — Amazon Resource Name (ARN) associated with this model.
//! * `modelKmsKeyArn` (KmsKeyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$) — The custom model
//!   is encrypted at rest using this key.
//! * `modelName` (CustomModelName, len 1..63, pattern ^([0-9a-zA-Z][_-]?){1,63}$)
//!   **required** — Model name associated with this model.
//! * `modelStatus` (ModelStatus, one of: Active | Creating | Failed) — The current status of
//!   the custom model. Possible values include: Creating - The model is being created and
//!   validated. Active - The model has been successfully created and is ready for use. Failed
//!   - The model creation process failed. Check the failureMessage field for details.
//! * `outputDataConfig` (OutputDataConfig) — Output data configuration associated with this
//!   custom model.
//! * `trainingDataConfig` (TrainingDataConfig) — Contains information about the training
//!   dataset.
//! * `trainingMetrics` (TrainingMetrics) — Contains training metrics from the job creation.
//! * `validationDataConfig` (ValidationDataConfig) — Contains information about the
//!   validation dataset.
//! * `validationMetrics` (ValidationMetrics) — The validation metrics from the job creation.
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
// `GetCustomModelError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetCustomModelError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetCustomModel")]
#[derive(Debug)]
pub enum GetCustomModelErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_custom_model::GetCustomModelError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetCustomModelError = crate::scaffolded::Scaffolded<GetCustomModelErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetCustomModelErrorModeled> for GetCustomModelError {
    fn from(modeled: GetCustomModelErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetCustomModel` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetCustomModel<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetCustomModel<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetCustomModel<Eng>
{
    type Input = GetCustomModelInput;
    type Output = GetCustomModelOutput;
    type Error = GetCustomModelError;

    const NAME: &'static str = "GetCustomModel";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CUSTOM_MODEL_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CUSTOM_MODEL_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetCustomModelErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("custom model state is not available on this engine".to_string()),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        let Some(record) = store.custom_model(&input.model_identifier, &region, now) else {
            // `modelIdentifier` admits the model's ARN or its bare NAME, and the two
            // unresolvable spellings are not the same failure — the one reading every
            // model-identifying member shares ([`crate::state::unresolved_model`]), each
            // answering it with the error its own model declares for that class.
            let refusal = crate::state::unresolved_model(
                &input.model_identifier,
                crate::state::CUSTOM_MODEL_RESOURCES,
            );
            let message = Some(refusal.message(&input.model_identifier));
            return Err(if refusal.missing_resource() {
                GetCustomModelErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                    message,
                })
            } else {
                GetCustomModelErrorModeled::ValidationException(ValidationException { message })
            }
            .into());
        };
        let record = record.clone();
        // The members a model customization JOB fills are read off THAT job — the one
        // record holding the hyperparameters, the datasets and the output location the
        // customization ran with — so this read and `GetModelCustomizationJob` report the
        // same values. A model created from artifacts was produced by no such job
        // (`jobArn` says so itself: "For models that you create with the CreateCustomModel
        // API operation, this is NULL"), so for it there is nothing of the kind to report.
        let job = record
            .job_arn
            .as_ref()
            .and_then(|arn| store.model_customization_jobs.get(arn));

        Ok(GetCustomModelOutput {
            model_arn: record.arn.clone(),
            model_name: record.name.clone(),
            base_model_arn: Some(record.base_model_arn.clone()),
            customization_type: Some(record.customization_type.to_string()),
            model_kms_key_arn: record.model_kms_key_arn.clone(),
            creation_time: crate::state::date_time(record.created_at),
            model_status: Some(record.status.to_string()),
            job_name: job.map(|job| job.name.clone()),
            job_arn: record.job_arn.clone(),
            hyper_parameters: job.and_then(|job| job.hyper_parameters.clone()),
            training_data_config: job.map(|job| job.training_data_config.clone()),
            validation_data_config: job.map(|job| job.validation_data_config.clone()),
            output_data_config: job.map(|job| job.output_data_config.clone()),
            customization_config: job.and_then(|job| job.customization_config.clone()),
            // The training and validation metrics are computed BY the customization run;
            // this emulator runs none, so neither is reported rather than reported as a
            // fabricated loss.
            training_metrics: None,
            validation_metrics: None,
            // "A failure message for any issues that occurred when creating the custom
            // model": nothing here makes an accepted create or customization fail, so there
            // is none.
            failure_message: None,
        })
    }
}

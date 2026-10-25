// SPDX-License-Identifier: CC0-1.0

//! @op GetInferenceProfile
//!
//! Reports the application inference profile an `inferenceProfileIdentifier` names,
//! entirely out of the record `ops/create_inference_profile.rs` stored in
//! [`crate::state`]: the ARN and id it was minted with, its name, description, tracked
//! models, status, type and the two instants. Both spellings the identifier admits — the
//! profile ARN and the bare `inferenceProfileId` — resolve to that one record
//! ([`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::inference_profile`]),
//! and an identifier naming no profile of this account and region is 404
//! `ResourceNotFoundException`.
//!
//! A pure READ: it writes nothing, because a created profile is `ACTIVE` from the call
//! that minted it and is owed no later transition.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetInferenceProfile`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Gets information about an inference profile. For more information, see Increase
//! throughput and resilience with cross-region inference in Amazon Bedrock. in the Amazon
//! Bedrock User Guide.
//!
//! ## Input
//!
//! * `inferenceProfileIdentifier` (InferenceProfileIdentifier, len 1..2048, pattern
//!   ^(arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:(|[0-9a-z-]{0,20}):(|[0-9]{12}):(inference-profile|application-inference-pro
//!   …) **required** — The ID or Amazon Resource Name (ARN) of the inference profile.
//!
//! ## Output
//!
//! * `createdAt` (Timestamp) — The time at which the inference profile was created.
//! * `description` (InferenceProfileDescription, len 1..200, pattern ^([0-9a-zA-Z:.][
//!   _-]?)+$) — The description of the inference profile.
//! * `inferenceProfileArn` (InferenceProfileArn, len 1..2048, pattern
//!   ^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:(|[0-9a-z-]{0,20}):(|[0-9]{12}):(inference-profile|application-inference-prof
//!   …) **required** — The Amazon Resource Name (ARN) of the inference profile.
//! * `inferenceProfileId` (InferenceProfileId, len 1..64, pattern ^[a-zA-Z0-9-:.]+$)
//!   **required** — The unique identifier of the inference profile.
//! * `inferenceProfileName` (InferenceProfileName, len 1..64, pattern ^([0-9a-zA-Z][ _-]?)+$)
//!   **required** — The name of the inference profile.
//! * `models` (InferenceProfileModels, len 1..5) **required** — A list of information about
//!   each model in the inference profile.
//! * `status` (InferenceProfileStatus, one of: ACTIVE) **required** — The status of the
//!   inference profile. ACTIVE means that the inference profile is ready to be used.
//! * `type` (InferenceProfileType, one of: APPLICATION | SYSTEM_DEFINED) **required** — The
//!   type of the inference profile. The following types are possible: SYSTEM_DEFINED – The
//!   inference profile is defined by Amazon Bedrock. You can route inference requests across
//!   regions with these inference profiles. APPLICATION – The inference profile was created
//!   by a user. This type of inference profile can track metrics and costs when invoking the
//!   model in it. The inference profile may route requests to one or multiple regions.
//! * `updatedAt` (Timestamp) — The time at which the inference profile was last updated.
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
// `GetInferenceProfileError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetInferenceProfileError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetInferenceProfile")]
#[derive(Debug)]
pub enum GetInferenceProfileErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_inference_profile::GetInferenceProfileError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetInferenceProfileError = crate::scaffolded::Scaffolded<GetInferenceProfileErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetInferenceProfileErrorModeled> for GetInferenceProfileError {
    fn from(modeled: GetInferenceProfileErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetInferenceProfile` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetInferenceProfile<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetInferenceProfile<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetInferenceProfile<Eng>
{
    type Input = GetInferenceProfileInput;
    type Output = GetInferenceProfileOutput;
    type Error = GetInferenceProfileError;

    const NAME: &'static str = "GetInferenceProfile";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_INFERENCE_PROFILE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_INFERENCE_PROFILE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetInferenceProfileErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "inference profile state is not available on this engine".to_string(),
                    ),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let store = state.lock();

        let Some(profile) = store.inference_profile(&input.inference_profile_identifier, &region)
        else {
            return Err(GetInferenceProfileErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(format!(
                        "no inference profile {} exists in this account",
                        input.inference_profile_identifier
                    )),
                },
            )
            .into());
        };

        Ok(GetInferenceProfileOutput {
            inference_profile_name: profile.name.clone(),
            description: profile.description.clone(),
            created_at: Some(crate::state::date_time(profile.created_at)),
            updated_at: Some(crate::state::date_time(profile.updated_at)),
            inference_profile_arn: profile.arn.clone(),
            models: profile.models.clone(),
            inference_profile_id: profile.id.clone(),
            status: profile.status.to_string(),
            r#type: profile.profile_type.to_string(),
        })
    }
}

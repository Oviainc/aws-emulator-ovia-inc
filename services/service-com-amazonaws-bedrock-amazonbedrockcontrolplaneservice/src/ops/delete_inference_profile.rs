// SPDX-License-Identifier: CC0-1.0

//! @op DeleteInferenceProfile
//!
//! Forgets the application inference profile an `inferenceProfileIdentifier` names, out of
//! the store `ops/create_inference_profile.rs` wrote it to, so a later
//! `GetInferenceProfile` for it answers 404. Both spellings the identifier admits resolve
//! to that one record, and an identifier naming no profile of this account and region is
//! 404 `ResourceNotFoundException` — including a second delete of one already gone.
//!
//! Nothing else referring to the profile holds the delete back: a prompt router, or any
//! other resource, names a model source by ARN rather than owning the profile, so the
//! `ConflictException` this operation models is not what a referenced profile gets.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteInferenceProfile`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Deletes an application inference profile. For more information, see Increase throughput
//! and resilience with cross-region inference in Amazon Bedrock. in the Amazon Bedrock User
//! Guide.
//!
//! ## Input
//!
//! * `inferenceProfileIdentifier` (InferenceProfileIdentifier, len 1..2048, pattern
//!   ^(arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:(|[0-9a-z-]{0,20}):(|[0-9]{12}):(inference-profile|application-inference-pro
//!   …) **required** — The Amazon Resource Name (ARN) or ID of the application inference
//!   profile to delete.
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
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteInferenceProfileError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteInferenceProfileError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#DeleteInferenceProfile")]
#[derive(Debug)]
pub enum DeleteInferenceProfileErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_inference_profile::DeleteInferenceProfileError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteInferenceProfileError = crate::scaffolded::Scaffolded<DeleteInferenceProfileErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteInferenceProfileErrorModeled> for DeleteInferenceProfileError {
    fn from(modeled: DeleteInferenceProfileErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteInferenceProfile` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteInferenceProfile<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteInferenceProfile<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteInferenceProfile<Eng>
{
    type Input = DeleteInferenceProfileInput;
    type Output = DeleteInferenceProfileOutput;
    type Error = DeleteInferenceProfileError;

    const NAME: &'static str = "DeleteInferenceProfile";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_INFERENCE_PROFILE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_INFERENCE_PROFILE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DeleteInferenceProfileErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "inference profile state is not available on this engine".to_string(),
                    ),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let mut store = state.lock();

        let Some(arn) =
            store.inference_profile_arn_of(&input.inference_profile_identifier, &region)
        else {
            return Err(DeleteInferenceProfileErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(format!(
                        "no inference profile {} exists in this account",
                        input.inference_profile_identifier
                    )),
                },
            )
            .into());
        };

        store.remove_inference_profile(&arn);
        Ok(DeleteInferenceProfileOutput {})
    }
}

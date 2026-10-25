// SPDX-License-Identifier: CC0-1.0

//! @op GetUseCaseForModelAccess
//!
//! Reports the account's acknowledged use case for model access: the
//! `AcknowledgementFormDataBody` `PutUseCaseForModelAccess` last submitted into
//! [`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::use_case_for_model_access`],
//! byte for byte as it was submitted.
//!
//! The response member is `@required`, so there is no answer that reports "no form":
//! while the account has submitted none, this operation looks up a form that is not there
//! and answers the modelled `ResourceNotFoundException` (HTTP 404). Once a form has been
//! submitted, every read reports the one the LAST admitted put stored, since a put
//! replaces the single account-level form whole.
//!
//! This operation is `@readonly`: it reports the stored form and writes nothing, so
//! repeated reads report the same bytes and a read never disturbs what a later caller
//! sees.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetUseCaseForModelAccess`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Get usecase for model access.
//!
//! ## Output
//!
//! * `formData` (AcknowledgementFormDataBody, len 10..16384) **required** — Get customer
//!   profile Response.
//!
//! ## Errors
//!
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
// `GetUseCaseForModelAccessError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetUseCaseForModelAccessError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetUseCaseForModelAccess")]
#[derive(Debug)]
pub enum GetUseCaseForModelAccessErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_use_case_for_model_access::GetUseCaseForModelAccessError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetUseCaseForModelAccessError = crate::scaffolded::Scaffolded<GetUseCaseForModelAccessErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetUseCaseForModelAccessErrorModeled> for GetUseCaseForModelAccessError {
    fn from(modeled: GetUseCaseForModelAccessErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetUseCaseForModelAccess` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetUseCaseForModelAccess<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetUseCaseForModelAccess<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetUseCaseForModelAccess<Eng>
{
    type Input = GetUseCaseForModelAccessInput;
    type Output = GetUseCaseForModelAccessOutput;
    type Error = GetUseCaseForModelAccessError;

    const NAME: &'static str = "GetUseCaseForModelAccess";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_USE_CASE_FOR_MODEL_ACCESS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_USE_CASE_FOR_MODEL_ACCESS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // This operation takes no input members, and `ctx` carries request metadata it
        // has no use for: the form it reports is account-level.
        let _ = (&input, ctx);
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetUseCaseForModelAccessErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "use case for model access state is not available on this engine"
                            .to_string(),
                    ),
                },
            )
            .into());
        };
        let store = state.lock();

        let Some(form_data) = store.use_case_for_model_access.clone() else {
            return Err(GetUseCaseForModelAccessErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(
                        "no use case for model access has been submitted for this account"
                            .to_string(),
                    ),
                },
            )
            .into());
        };

        Ok(GetUseCaseForModelAccessOutput { form_data })
    }
}

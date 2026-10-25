// SPDX-License-Identifier: CC0-1.0

//! @op PutUseCaseForModelAccess
//!
//! Stores the `formData` the request carries as the account's acknowledged use case for
//! model access in
//! [`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::use_case_for_model_access`],
//! so `GetUseCaseForModelAccess` reports it back byte for byte.
//!
//! The form is a single account-level record: this put REPLACES whatever was submitted
//! before, whole, so the same request applied twice leaves the same form behind and the
//! store cannot grow by putting again. The response has no members, and the operation
//! answers HTTP 201 as the model binds it.
//!
//! The one constraint the model states — `AcknowledgementFormDataBody` 10..16384 bytes —
//! is refused by the centralized validation walk BEFORE this handler runs, so a request
//! that reaches here is one this emulator stores as given. What the form SAYS is a
//! document this emulator does not interpret.
//!
//! The writer half of this operation's reader, `GetUseCaseForModelAccess`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PutUseCaseForModelAccess`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Put usecase for model access.
//!
//! ## Input
//!
//! * `formData` (AcknowledgementFormDataBody, len 10..16384) **required** — Put customer
//!   profile Request.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutUseCaseForModelAccessError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutUseCaseForModelAccessError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#PutUseCaseForModelAccess")]
#[derive(Debug)]
pub enum PutUseCaseForModelAccessErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_use_case_for_model_access::PutUseCaseForModelAccessError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutUseCaseForModelAccessError = crate::scaffolded::Scaffolded<PutUseCaseForModelAccessErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutUseCaseForModelAccessErrorModeled> for PutUseCaseForModelAccessError {
    fn from(modeled: PutUseCaseForModelAccessErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutUseCaseForModelAccess` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutUseCaseForModelAccess<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutUseCaseForModelAccess<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutUseCaseForModelAccess<Eng>
{
    type Input = PutUseCaseForModelAccessInput;
    type Output = PutUseCaseForModelAccessOutput;
    type Error = PutUseCaseForModelAccessError;

    const NAME: &'static str = "PutUseCaseForModelAccess";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_USE_CASE_FOR_MODEL_ACCESS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_USE_CASE_FOR_MODEL_ACCESS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `ctx` carries request metadata this operation has no use for: the form it
        // stores is account-level.
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(PutUseCaseForModelAccessErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "use case for model access state is not available on this engine"
                            .to_string(),
                    ),
                },
            )
            .into());
        };
        state.lock().use_case_for_model_access = Some(input.form_data);

        Ok(PutUseCaseForModelAccessOutput {})
    }
}

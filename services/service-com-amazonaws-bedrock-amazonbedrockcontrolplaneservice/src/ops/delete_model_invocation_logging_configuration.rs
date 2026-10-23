// SPDX-License-Identifier: CC0-1.0

//! @op DeleteModelInvocationLoggingConfiguration
//!
//! Removes the account's model invocation logging configuration from
//! [`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::model_invocation_logging`],
//! so `GetModelInvocationLoggingConfiguration` afterwards reports no `loggingConfig`
//! at all.
//!
//! The call is `@idempotent` and names no resource, and the model declares no
//! not-found error for it, so deleting while nothing is configured succeeds exactly
//! like deleting a configuration that was in force: the post-state is "invocation
//! logging is not configured" either way. The response has no members.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteModelInvocationLoggingConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Delete the invocation logging.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteModelInvocationLoggingConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteModelInvocationLoggingConfigurationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#DeleteModelInvocationLoggingConfiguration")]
#[derive(Debug)]
pub enum DeleteModelInvocationLoggingConfigurationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_model_invocation_logging_configuration::DeleteModelInvocationLoggingConfigurationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteModelInvocationLoggingConfigurationError = crate::scaffolded::Scaffolded<DeleteModelInvocationLoggingConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteModelInvocationLoggingConfigurationErrorModeled> for DeleteModelInvocationLoggingConfigurationError {
    fn from(modeled: DeleteModelInvocationLoggingConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteModelInvocationLoggingConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteModelInvocationLoggingConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteModelInvocationLoggingConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteModelInvocationLoggingConfiguration<Eng>
{
    type Input = DeleteModelInvocationLoggingConfigurationInput;
    type Output = DeleteModelInvocationLoggingConfigurationOutput;
    type Error = DeleteModelInvocationLoggingConfigurationError;

    const NAME: &'static str = "DeleteModelInvocationLoggingConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_MODEL_INVOCATION_LOGGING_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_MODEL_INVOCATION_LOGGING_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // This operation takes no input members, and `ctx` carries request metadata it
        // has no use for: the configuration it removes is account-level.
        let _ = (&input, ctx);
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                DeleteModelInvocationLoggingConfigurationErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "model invocation logging state is not available on this engine"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };
        state.lock().model_invocation_logging = None;

        Ok(DeleteModelInvocationLoggingConfigurationOutput {})
    }
}

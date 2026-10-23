// SPDX-License-Identifier: CC0-1.0

//! @op PutModelInvocationLoggingConfiguration
//!
//! Stores the `loggingConfig` the request carries as the account's model invocation
//! logging configuration in
//! [`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::model_invocation_logging`],
//! so `GetModelInvocationLoggingConfiguration` reports it back.
//!
//! The configuration is a SETTINGS record, and this call is `@idempotent`: it
//! REPLACES whatever was in force whole rather than merging into it, so a member the
//! request omits is not configured afterwards even if an earlier put set it, and the
//! same request applied twice leaves the same configuration behind. The response has
//! no members.
//!
//! Every constraint the model states — `logGroupName` 1..512, the `roleArn` IAM role
//! pattern, `bucketName` 3..63, `keyPrefix` ..1024 — is refused by the centralized
//! validation walk BEFORE this handler runs, so a request that reaches here is one
//! this emulator stores as given. Whether the named role may actually write to the
//! named log group or bucket is an IAM decision this emulator does not take.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PutModelInvocationLoggingConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Set the configuration values for model invocation logging.
//!
//! ## Input
//!
//! * `loggingConfig` (LoggingConfig) **required** — The logging configuration values to set.
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
// `PutModelInvocationLoggingConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutModelInvocationLoggingConfigurationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#PutModelInvocationLoggingConfiguration")]
#[derive(Debug)]
pub enum PutModelInvocationLoggingConfigurationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_model_invocation_logging_configuration::PutModelInvocationLoggingConfigurationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutModelInvocationLoggingConfigurationError = crate::scaffolded::Scaffolded<PutModelInvocationLoggingConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutModelInvocationLoggingConfigurationErrorModeled> for PutModelInvocationLoggingConfigurationError {
    fn from(modeled: PutModelInvocationLoggingConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutModelInvocationLoggingConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutModelInvocationLoggingConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutModelInvocationLoggingConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutModelInvocationLoggingConfiguration<Eng>
{
    type Input = PutModelInvocationLoggingConfigurationInput;
    type Output = PutModelInvocationLoggingConfigurationOutput;
    type Error = PutModelInvocationLoggingConfigurationError;

    const NAME: &'static str = "PutModelInvocationLoggingConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_MODEL_INVOCATION_LOGGING_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_MODEL_INVOCATION_LOGGING_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `ctx` carries request metadata this operation has no use for: the
        // configuration it sets is account-level.
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                PutModelInvocationLoggingConfigurationErrorModeled::InternalServerException(
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
        state.lock().model_invocation_logging = Some(input.logging_config);

        Ok(PutModelInvocationLoggingConfigurationOutput {})
    }
}

// SPDX-License-Identifier: CC0-1.0

//! @op DeleteGroupingConfiguration
//! Removes this account's persisted grouping configuration.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `DeleteGroupingConfiguration::new(engine)` and drives it through the centralized request
//! walk: deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteGroupingConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-applicationsignals-applicationsignals/smithy-model/com-amazonaws-applicationsignals-applicationsignals.json")
//! ```
//!
//! Deletes the grouping configuration for this account. This removes all custom grouping
//! attribute definitions that were previously configured.
//!
//! `DELETE /grouping-configuration`, success code 200, `@idempotent`.
//!
//! ## Input
//!
//! `smithy.api#Unit` — this operation takes no members at all.
//!
//! ## Output
//!
//! `DeleteGroupingConfigurationOutput` — an empty structure, so a success is HTTP 200 with
//! the empty JSON body `{}`.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You don't have sufficient permissions to perform
//!   this action.
//! * `ThrottlingException` (HTTP 429) — The request was throttled because of quota limits.
//! * `ValidationException` (HTTP 400) — The resource is not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteGroupingConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-applicationsignals-applicationsignals.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteGroupingConfigurationError` alias below.
#[smithy_macros::operation_error("com.amazonaws.applicationsignals#DeleteGroupingConfiguration")]
#[derive(Debug)]
pub enum DeleteGroupingConfigurationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_grouping_configuration::DeleteGroupingConfigurationError`).
pub type DeleteGroupingConfigurationError =
    crate::scaffolded::Scaffolded<DeleteGroupingConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<DeleteGroupingConfigurationErrorModeled> for DeleteGroupingConfigurationError {
    fn from(modeled: DeleteGroupingConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteGroupingConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct DeleteGroupingConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteGroupingConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteGroupingConfiguration<Eng>
{
    type Input = DeleteGroupingConfigurationInput;
    type Output = DeleteGroupingConfigurationOutput;
    type Error = DeleteGroupingConfigurationError;

    const NAME: &'static str = "DeleteGroupingConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DELETE_GROUPING_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DELETE_GROUPING_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    /// Clear the stored grouping configuration, so a later read reports no definitions.
    ///
    /// Modelled `@idempotent` and declaring NO not-found error, so deleting when nothing is
    /// configured is a plain success: the post-condition ("no grouping configuration for
    /// this account") already holds, and repeating the call answers the same way.
    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = (&input, ctx);
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DeleteGroupingConfigurationErrorModeled::ThrottlingException(
                ThrottlingException {
                    message: crate::state::STATE_UNAVAILABLE.to_string(),
                },
            )
            .into());
        };

        // The removal is the whole effect; whether anything was configured changes nothing
        // the caller can observe, because the output structure declares no members.
        state.lock().clear_grouping_configuration();
        Ok(DeleteGroupingConfigurationOutput {})
    }
}

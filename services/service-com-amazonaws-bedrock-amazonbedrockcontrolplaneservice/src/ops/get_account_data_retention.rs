// SPDX-License-Identifier: CC0-1.0

//! @op GetAccountDataRetention
//!
//! Reports the account's data retention setting: the `mode` `PutAccountDataRetention` last
//! stored in
//! [`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::account_data_retention`],
//! together with the `updatedAt` instant that put stamped on it.
//!
//! While no put has set a mode the account still HAS one — `mode` is `@required`, so there
//! is no answer without it — and that mode is
//! [`crate::state::DATA_RETENTION_MODE_DEFAULT`], reported with NO `updatedAt`: the member
//! is optional, so its absence is how this operation says nothing has ever updated the
//! setting.
//!
//! This operation is `@readonly`: it reports the stored setting and writes nothing, so
//! repeated reads report the same thing and a read never disturbs what a later caller
//! sees.
//!
//! The reader half of `PutAccountDataRetention`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetAccountDataRetention`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Returns the account-wide data retention mode for Amazon Bedrock.
//!
//! ## Output
//!
//! * `mode` (DataRetentionMode, one of: aws_review | default | inherit | none |
//!   provider_data_share) **required** — The data retention mode configured for the account.
//! * `updatedAt` (Timestamp) — The time at which the data retention mode was last updated.
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
// `GetAccountDataRetentionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetAccountDataRetentionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetAccountDataRetention")]
#[derive(Debug)]
pub enum GetAccountDataRetentionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_account_data_retention::GetAccountDataRetentionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetAccountDataRetentionError = crate::scaffolded::Scaffolded<GetAccountDataRetentionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetAccountDataRetentionErrorModeled> for GetAccountDataRetentionError {
    fn from(modeled: GetAccountDataRetentionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAccountDataRetention` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetAccountDataRetention<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAccountDataRetention<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAccountDataRetention<Eng>
{
    type Input = GetAccountDataRetentionInput;
    type Output = GetAccountDataRetentionOutput;
    type Error = GetAccountDataRetentionError;

    const NAME: &'static str = "GetAccountDataRetention";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ACCOUNT_DATA_RETENTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ACCOUNT_DATA_RETENTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // This operation takes no input members, and `ctx` carries request metadata it has
        // no use for: the setting it reports is account-level.
        let _ = (&input, ctx);
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetAccountDataRetentionErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "account data retention state is not available on this engine"
                            .to_string(),
                    ),
                },
            )
            .into());
        };
        let store = state.lock();

        // The stored setting, or — while none was ever put — the mode the account retains
        // under, with no instant to report because nothing has updated it.
        let (mode, updated_at) = match store.account_data_retention.as_ref() {
            Some(stored) => (
                stored.mode.clone(),
                Some(crate::state::date_time(stored.updated_at)),
            ),
            None => (
                crate::state::DATA_RETENTION_MODE_DEFAULT.to_string(),
                None,
            ),
        };

        Ok(GetAccountDataRetentionOutput { mode, updated_at })
    }
}

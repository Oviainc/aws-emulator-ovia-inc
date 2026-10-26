// SPDX-License-Identifier: CC0-1.0

//! @op PutAccountDataRetention
//!
//! Stores the `mode` the request carries as the account's data retention setting in
//! [`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::account_data_retention`],
//! so `GetAccountDataRetention` reports that same mode back.
//!
//! The setting is a single account-level record: this put REPLACES whatever mode was in
//! force, whole, so the same request applied twice leaves the same mode behind and the
//! store cannot grow by putting again — which is what the model's `@idempotent` says. The
//! `updatedAt` both this response and the later read carry is the instant of the last
//! admitted put, because that is when the setting was last written.
//!
//! The one constraint the model states — `mode` is a `DataRetentionMode`, so one of that
//! enum's values — is refused by the centralized validation walk BEFORE this handler runs,
//! so a request that reaches here carries a mode this emulator stores as given.
//!
//! The writer half of `GetAccountDataRetention`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PutAccountDataRetention`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Sets the account-wide data retention mode for Amazon Bedrock.
//!
//! ## Input
//!
//! * `mode` (DataRetentionMode, one of: aws_review | default | inherit | none |
//!   provider_data_share) **required** — The data retention mode to set for the account.
//!
//! ## Output
//!
//! * `mode` (DataRetentionMode, one of: aws_review | default | inherit | none |
//!   provider_data_share) **required** — The data retention mode set for the account.
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
// `PutAccountDataRetentionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutAccountDataRetentionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#PutAccountDataRetention")]
#[derive(Debug)]
pub enum PutAccountDataRetentionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_account_data_retention::PutAccountDataRetentionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutAccountDataRetentionError = crate::scaffolded::Scaffolded<PutAccountDataRetentionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutAccountDataRetentionErrorModeled> for PutAccountDataRetentionError {
    fn from(modeled: PutAccountDataRetentionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutAccountDataRetention` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutAccountDataRetention<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutAccountDataRetention<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutAccountDataRetention<Eng>
{
    type Input = PutAccountDataRetentionInput;
    type Output = PutAccountDataRetentionOutput;
    type Error = PutAccountDataRetentionError;

    const NAME: &'static str = "PutAccountDataRetention";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_ACCOUNT_DATA_RETENTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_ACCOUNT_DATA_RETENTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `ctx` carries request metadata this operation has no use for: the setting it
        // stores is account-level.
        let _ = ctx;
        let now = ::std::time::SystemTime::now();
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(PutAccountDataRetentionErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "account data retention state is not available on this engine"
                            .to_string(),
                    ),
                },
            )
            .into());
        };
        let mut store = state.lock();

        // The record this put leaves behind, and the response read back out of THAT record,
        // so what the caller is told is exactly what the next `GetAccountDataRetention`
        // reports. `Option::insert` both stores it and hands back the stored value.
        let stored = store
            .account_data_retention
            .insert(crate::state::AccountDataRetention {
                mode: input.mode,
                updated_at: now,
            });
        Ok(PutAccountDataRetentionOutput {
            mode: stored.mode.clone(),
            updated_at: Some(crate::state::date_time(stored.updated_at)),
        })
    }
}

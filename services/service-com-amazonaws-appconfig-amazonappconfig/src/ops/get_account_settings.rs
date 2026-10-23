// SPDX-License-Identifier: CC0-1.0

//! @op GetAccountSettings
//! IMPLEMENTED: reports the account-level AppConfig settings held in [`crate::state`],
//! as the last successful `UpdateAccountSettings` wrote them.
//!
//! A pure read: this handler takes the store's guard, copies the account's settings
//! record and releases it, so a Get leaves the next caller seeing exactly what it saw.
//! An account that has written no settings is not unconfigured — it is running on
//! AppConfig's documented defaults, "the default interval specified by
//! `ProtectionPeriodInMinutes` is 60" — so the response reports
//! [`crate::state::AccountSettingsRecord::default`] until a write replaces it.
//!
//! Hand-authored rather than scaffolded: the op-stub generator declined this operation
//! because its input is `smithy.api#Unit` (recorded in `src/ops/SKIPPED_STUBS.md`), so
//! it had no handler file and no entry in the service's `operations(...)` allow-list and
//! was answered by the framework's unroutable-operation path — a 501 whose message read
//! "operation GET /settings is not implemented in this emulator". `build.rs` derives
//! that allow-list from the `//! @op <Name>` header of every `src/ops/*.rs`, so this
//! file is what wires it.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back; never
//! construct an engine in a handler.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetAccountSettings`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json")
//! ```
//!
//! Returns information about the status of the `DeletionProtection` parameter.
//!
//! ## Input
//!
//! `smithy.api#Unit` — no members. Bound to `GET /settings`.
//!
//! ## Output
//!
//! * `DeletionProtection` (DeletionProtectionSettings) — A parameter to configure deletion
//!   protection. Deletion protection prevents a user from deleting a configuration profile or
//!   an environment if AppConfig has called either GetLatestConfiguration or for the
//!   configuration profile or from the environment during the specified interval. The default
//!   interval for ProtectionPeriodInMinutes is 60.
//! * `VendedMetrics` (VendedMetricsSettings) — The configuration for vended metrics in the
//!   account.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — The input fails to satisfy the constraints specified
//!   by an Amazon Web Services service.
//! * `InternalServerException` (HTTP 500) — There was an internal failure in the AppConfig
//!   service.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetAccountSettingsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appconfig-amazonappconfig.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetAccountSettingsError` alias below.
#[smithy_macros::operation_error("com.amazonaws.appconfig#GetAccountSettings")]
#[derive(Debug)]
pub enum GetAccountSettingsErrorModeled {
    BadRequestException(BadRequestException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_account_settings::GetAccountSettingsError`).
pub type GetAccountSettingsError = crate::scaffolded::Scaffolded<GetAccountSettingsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<GetAccountSettingsErrorModeled> for GetAccountSettingsError {
    fn from(modeled: GetAccountSettingsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAccountSettings` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetAccountSettings<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAccountSettings<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAccountSettings<Eng>
{
    type Input = GetAccountSettingsInput;
    type Output = GetAccountSettingsOutput;
    type Error = GetAccountSettingsError;

    const NAME: &'static str = "GetAccountSettings";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_ACCOUNT_SETTINGS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_ACCOUNT_SETTINGS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing to read from it, and `ctx`
        // carries only request metadata this read does not need.
        let _ = (&input, ctx);

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetAccountSettingsErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("AppConfig state is not reachable on this engine".to_string()),
                },
            )
            .into());
        };

        // Read-only: take the guard, copy the record, release it.
        let settings = state.lock().account_settings.clone();
        Ok(GetAccountSettingsOutput {
            deletion_protection: Some(settings.deletion_protection()),
            vended_metrics: settings.vended_metrics(),
        })
    }
}

// SPDX-License-Identifier: CC0-1.0

//! @op GetGlobalSettings
//!
//! Reports the account-level global settings held in [`crate::state`] — "Global settings
//! for all workloads" — as the last successful `UpdateGlobalSettings` recorded them.
//!
//! A pure read: this handler takes the store's guard, projects the singleton and releases
//! it, so a read leaves the next caller seeing exactly what it saw. An account that has
//! recorded no settings is not unconfigured — it is running with every account-level
//! feature off — so each status is reported as the value such an account is running under
//! ([`crate::state::GlobalSettingsRecord::to_global_settings`]) until a write replaces
//! it.
//!
//! Hand-authored rather than scaffolded: the op-stub generator declined this operation
//! because its input is `smithy.api#Unit` (recorded in `src/ops/SKIPPED_STUBS.md`), so it
//! had no handler file and no entry in the service's `operations(...)` allow-list and was
//! answered by the framework's unroutable-operation path. `build.rs` derives that
//! allow-list from the `//! @op <Name>` header of every `src/ops/*.rs`, so this file is
//! what wires it.
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
//! # Modeled contract for `GetGlobalSettings`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json")
//! ```
//!
//! Global settings for all workloads.
//!
//! ## Input
//!
//! `smithy.api#Unit` — no members.
//!
//! ## Output
//!
//! * `DiscoveryIntegrationStatus` (DiscoveryIntegrationStatus, one of: DISABLED |
//!   ENABLED) — Discovery integration status.
//! * `JiraConfiguration` (AccountJiraConfigurationOutput) — Jira configuration status.
//! * `OrganizationSharingStatus` (OrganizationSharingStatus, one of: DISABLED | ENABLED) —
//!   Amazon Web Services Organizations sharing status.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — User does not have sufficient access to perform
//!   this action.
//! * `InternalServerException` (HTTP 500) — There is a problem with the Well-Architected Tool
//!   API service.
//! * `ThrottlingException` (HTTP 429) — Request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The user input is not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetGlobalSettingsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetGlobalSettingsError` alias below.
#[smithy_macros::operation_error("com.amazonaws.wellarchitected#GetGlobalSettings")]
#[derive(Debug)]
pub enum GetGlobalSettingsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_global_settings::GetGlobalSettingsError`).
pub type GetGlobalSettingsError = crate::scaffolded::Scaffolded<GetGlobalSettingsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<GetGlobalSettingsErrorModeled> for GetGlobalSettingsError {
    fn from(modeled: GetGlobalSettingsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetGlobalSettings` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetGlobalSettings<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetGlobalSettings<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetGlobalSettings<Eng>
{
    type Input = GetGlobalSettingsInput;
    type Output = GetGlobalSettingsOutput;
    type Error = GetGlobalSettingsError;

    const NAME: &'static str = "GetGlobalSettings";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_GLOBAL_SETTINGS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_GLOBAL_SETTINGS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing to read from it; `ctx`
        // carries the Region whose settings this read reports.
        let _ = &input;

        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(|| {
            GetGlobalSettingsErrorModeled::InternalServerException(InternalServerException {
                message: "the Well-Architected Tool global settings store is unavailable"
                    .to_string(),
            })
        })?;

        // Take the guard, project what is stored, release it: a read of the settings
        // leaves them exactly as the last write left them.
        let settings = state.lock().global_settings.to_global_settings();

        Ok(settings)
    }
}

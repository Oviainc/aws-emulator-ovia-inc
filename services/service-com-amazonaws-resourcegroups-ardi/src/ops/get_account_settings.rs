// SPDX-License-Identifier: CC0-1.0

//! @op GetAccountSettings
//!
//! Reports the status of this account's optional Resource Groups features in the
//! region the request was signed for — what the last successful
//! `UpdateAccountSettings` asked for there, and what is in effect now — or the
//! settings of a region nothing has been written for, which is the feature turned
//! off.
//!
//! Reporting: the only thing this handler writes is the status change the clock had
//! already made due (a switch `UpdateAccountSettings` accepted whose span has
//! elapsed), which was owed at this instant whether or not anybody called. It
//! stores no record for a region that has none, so a read leaves the next caller
//! seeing exactly what it saw.
//!
//! The operation models no not-found error, and an account always HAS settings, so
//! there is no 404 here: an account that has never asked for a feature is a 200
//! reporting it off.
//!
//! Hand-authored rather than scaffolded: the op-stub generator declined this
//! operation because its input is `smithy.api#Unit` (recorded in
//! `src/ops/SKIPPED_STUBS.md`), so it had no handler file and no entry in the
//! service's `operations(...)` allow-list and was answered by the framework's
//! unroutable-operation path. `build.rs` derives that allow-list from the
//! `//! @op <Name>` header of every `src/ops/*.rs`, so this file is what wires it.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back;
//! never construct an engine in a handler.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! the SigV4 credential scope this handler reads the region from lives there.
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
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-resourcegroups-ardi/smithy-model/com-amazonaws-resourcegroups-ardi.json")
//! ```
//!
//! Retrieves the current status of optional features in Resource Groups.
//!
//! ## Input
//!
//! `smithy.api#Unit` — no members.
//!
//! ## Output
//!
//! * `AccountSettings` (AccountSettings) — The current settings for the optional features
//!   in Resource Groups.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — The request includes one or more parameters that
//!   violate validation rules.
//! * `ForbiddenException` (HTTP 403) — The caller isn't authorized to make the request. Check
//!   permissions.
//! * `InternalServerErrorException` (HTTP 500) — An internal error occurred while processing
//!   the request. Try again later.
//! * `MethodNotAllowedException` (HTTP 405) — The request uses an HTTP method that isn't
//!   allowed for the specified resource.
//! * `TooManyRequestsException` (HTTP 429) — You've exceeded throttling limits by making too
//!   many requests in a period of time.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetAccountSettingsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-resourcegroups-ardi.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetAccountSettingsError` alias below.
#[smithy_macros::operation_error("com.amazonaws.resourcegroups#GetAccountSettings")]
#[derive(Debug)]
pub enum GetAccountSettingsErrorModeled {
    BadRequestException(BadRequestException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    MethodNotAllowedException(MethodNotAllowedException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_account_settings::GetAccountSettingsError`).
pub type GetAccountSettingsError = crate::scaffolded::Scaffolded<GetAccountSettingsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<GetAccountSettingsErrorModeled> for GetAccountSettingsError {
    fn from(modeled: GetAccountSettingsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAccountSettings` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
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
        // Authorization is not emulated: the emulator verifies request signatures
        // centrally but holds no principals or policies, so the modelled
        // `ForbiddenException` is deliberately unreachable here and a success says
        // nothing about the caller's permissions. The input is `smithy.api#Unit`, so
        // there is nothing to read from it: which settings this reports is decided by
        // the region the request was signed for, exactly as the region the writer
        // keyed them under was. Reporting changes nothing a later caller sees, so
        // this operation emits no audit record.
        let _ = &input;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetAccountSettingsErrorModeled::InternalServerErrorException(
                InternalServerErrorException {
                    message: Some("the resource-groups store is unavailable".to_string()),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        // The store settles the switch the clock already owes and hands back the
        // record it then holds — the default for a region it holds none for, which it
        // does NOT store.
        let settings = state
            .lock()
            .account_settings_at(&region, std::time::SystemTime::now());
        Ok(GetAccountSettingsOutput {
            // An account always has optional-feature settings, so the member is
            // always reported: an account that has asked for nothing is reported with
            // the feature off, never with an absent document.
            account_settings: Some(settings.to_account_settings()),
        })
    }
}

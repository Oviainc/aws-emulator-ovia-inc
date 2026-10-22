// SPDX-License-Identifier: CC0-1.0

//! @op GetSendQuota
//!
//! Reports this account's sending limits and how much of the daily quota the
//! trailing 24 hours of sends have used.
//!
//! HAND-AUTHORED, unlike its siblings: the scaffold op-stub generator declined
//! `GetSendQuota` because the model binds it to the `smithy.api#Unit` input
//! shape, which the generator does not emit yet (`src/ops/SKIPPED_STUBS.md`
//! still lists it). The file follows a rendered sibling exactly — `//! @op
//! GetSendQuota` above is what puts the operation back in the service's
//! `operations(...)` allow-list, and `types/get_send_quota_types.rs` carries its
//! two shapes.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `GetSendQuota::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
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
//! ---
//!
//! # Modeled contract for `GetSendQuota`
//!
//! Provides the sending limits for the Amazon SES account. You can execute this
//! operation no more than once per second.
//!
//! ## Input
//!
//! (no members: the modeled input is `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `Max24HourSend` (double, default 0) — The maximum number of emails the user is
//!   allowed to send in a 24-hour interval. A value of -1 signifies an unlimited quota.
//! * `MaxSendRate` (double, default 0) — The maximum number of emails that Amazon SES
//!   can accept from the user's account per second.
//! * `SentLast24Hours` (double, default 0) — The number of emails sent during the
//!   previous 24 hours.
//!
//! ## Errors
//!
//! (none)

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetSendQuotaError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ses-simpleemailservice.json");

// The operation's MODELED errors. `GetSendQuota` declares none, so this enum is
// UNINHABITED, which is correct and expected: there is nothing to construct
// until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ses#GetSendQuota")]
#[derive(Debug)]
pub enum GetSendQuotaErrorModeled {}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_send_quota::GetSendQuotaError`). The wrapper is also the only
/// place an operation whose model declares no error at all can refuse a request.
pub type GetSendQuotaError = crate::scaffolded::Scaffolded<GetSendQuotaErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant.
impl From<GetSendQuotaErrorModeled> for GetSendQuotaError {
    fn from(modeled: GetSendQuotaErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetSendQuota` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetSendQuota<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetSendQuota<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetSendQuota<Eng>
{
    type Input = GetSendQuotaInput;
    type Output = GetSendQuotaOutput;
    type Error = GetSendQuotaError;

    const NAME: &'static str = "GetSendQuota";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_SEND_QUOTA_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_SEND_QUOTA_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The modeled input has no members, and `ctx` carries only request
        // metadata; this operation reads neither.
        let _ = (&input, ctx);
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // See `verify_email_identity.rs`: an engine with no store for this
            // service cannot serve the operation at all, and it models no server
            // error of its own.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "GetSendQuota",
            ));
        };
        let state = state.lock();

        // Reporting only: the limits come from the stored account record (so work
        // that raises the quota changes one place), and the usage is COMPUTED from
        // the send log on every read, so it falls again as messages age out of the
        // 24-hour window instead of being a counter that can drift.
        Ok(GetSendQuotaOutput {
            max24_hour_send: state.account.max_24_hour_send,
            max_send_rate: state.account.max_send_rate,
            sent_last24_hours: state.sent_last_24_hours(std::time::SystemTime::now()),
        })
    }
}

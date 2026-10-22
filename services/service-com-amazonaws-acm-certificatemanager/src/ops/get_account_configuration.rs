// SPDX-License-Identifier: CC0-1.0

//! @op GetAccountConfiguration
//!
//! Reports the account-level expiry-events configuration held in [`crate::state`],
//! as the last successful `PutAccountConfiguration` wrote it.
//!
//! A pure read: this handler takes the store's guard, copies the singleton and
//! releases it, so a Get leaves the next caller seeing exactly what it saw. An
//! account that has written no configuration is not unconfigured — it is running on
//! ACM's documented default, "accounts receive events starting 45 days before
//! certificate expiration" (`ExpiryEventsConfiguration$DaysBeforeExpiry` doc) — so
//! `ExpiryEvents` reports [`crate::state::DEFAULT_DAYS_BEFORE_EXPIRY`] until a write
//! replaces it.
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
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetAccountConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json")
//! ```
//!
//! Returns the account configuration options associated with an Amazon Web Services
//! account.
//!
//! ## Input
//!
//! `smithy.api#Unit` — no members.
//!
//! ## Output
//!
//! * `ExpiryEvents` (ExpiryEventsConfiguration) — Expiration events configuration options
//!   associated with the Amazon Web Services account.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have access required to perform this
//!   action.
//! * `ThrottlingException` (HTTP 400) — The request was denied because it exceeded a quota.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetAccountConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-acm-certificatemanager.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetAccountConfigurationError` alias below.
#[smithy_macros::operation_error("com.amazonaws.acm#GetAccountConfiguration")]
#[derive(Debug)]
pub enum GetAccountConfigurationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_account_configuration::GetAccountConfigurationError`).
pub type GetAccountConfigurationError =
    crate::scaffolded::Scaffolded<GetAccountConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<GetAccountConfigurationErrorModeled> for GetAccountConfigurationError {
    fn from(modeled: GetAccountConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAccountConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetAccountConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAccountConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAccountConfiguration<Eng>
{
    type Input = GetAccountConfigurationInput;
    type Output = GetAccountConfigurationOutput;
    type Error = GetAccountConfigurationError;

    const NAME: &'static str = "GetAccountConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_ACCOUNT_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_ACCOUNT_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing to read from it, and
        // `ctx` carries only request metadata this read does not need.
        let _ = (&input, ctx);

        // Unreachable in the assembled emulator (the engine is the shared store), and
        // this operation models no server fault, so the wrapper's refusal is the one
        // honest answer left.
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(crate::scaffolded::Scaffolded::validation(
                "the acm account configuration store is unavailable",
            ));
        };

        // Read-only for the account configuration: take the guard, copy what is
        // stored, release it. The one write is the crate-wide convention — apply
        // whatever the clock already owes before reading the store, so every
        // operation sees the state a caller would see after any other.
        let stored = {
            let mut store = state.lock();
            store.materialize_due(std::time::SystemTime::now());
            store.account_configuration.clone()
        };

        Ok(GetAccountConfigurationOutput {
            expiry_events: Some(crate::state::expiry_events_configuration(stored.as_ref())),
        })
    }
}

// SPDX-License-Identifier: CC0-1.0

//! @op GetHarness
//!
//! Reports one stored harness: what [`crate::ops::create_harness`] persisted in
//! [`crate::state`], never anything echoed from this request. `@readonly`, so it writes
//! nothing — the only state it resolves is the `CREATING` -> `READY` transition the clock
//! already made due.
//!
//! HAND-AUTHORED, not generated: the scaffold's op-stub generator declined this operation
//! ("shape missing from model: smithy.api#Unit", see `src/ops/SKIPPED_STUBS.md`), so it
//! was not in the service's `operations(..)` allow-list at all — and a route the model
//! binds but the service does not wire is not claimed by any service, which the router
//! answers `404 UnknownService` in its own envelope. Its shapes live in
//! `types/harness_types.rs`.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `GetHarness::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. Only the `handle` body is this file's.
//!
//! `harnessId`'s and `harnessVersion`'s modelled constraints are enforced by that walk
//! BEFORE `handle` runs, and refused with HTTP 400 `ValidationException`.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach the
//! store through [`crate::state::service_state`]. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `GetHarness`
//!
//! From this service's Smithy model
//! (`smithy_query(query="operation",operation="GetHarness",model_path="services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json")`).
//!
//! Operation to get a single harness. `GET /harnesses/{harnessId}`, success status 200,
//! `@readonly`.
//!
//! ## Input
//!
//! * `harnessId` (HarnessId, @httpLabel, pattern
//!   ^[a-zA-Z][a-zA-Z0-9_]{0,39}-[a-zA-Z0-9]{10}$) **required** — the harness to retrieve.
//! * `harnessVersion` (HarnessVersion, @httpQuery `harnessVersion`, len 1..5, pattern
//!   ^([1-9][0-9]{0,4})$) — the version of the harness to retrieve.
//!
//! ## Output
//!
//! * `harness` (Harness) **required** — the harness.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — the request is denied per access permissions.
//! * `InternalServerException` (HTTP 500) — an unexpected error during processing.
//! * `ResourceNotFoundException` (HTTP 404) — a resource the operation references does not
//!   exist.
//! * `ThrottlingException` (HTTP 429) — the number of requests exceeds the limit.
//! * `ValidationException` (HTTP 400) — the input fails to satisfy the service's
//!   constraints.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json");

/// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentcorecontrol#GetHarness")]
#[derive(Debug)]
pub enum GetHarnessErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type GetHarnessError = crate::scaffolded::Scaffolded<GetHarnessErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetHarnessErrorModeled> for GetHarnessError {
    fn from(modeled: GetHarnessErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetHarness` operation.
pub struct GetHarness<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetHarness<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetHarness<Eng>
{
    type Input = GetHarnessInput;
    type Output = GetHarnessOutput;
    type Error = GetHarnessError;

    const NAME: &'static str = "GetHarness";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_HARNESS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_HARNESS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetHarnessErrorModeled::InternalServerException(InternalServerException {
                message: Some("the harness store is unavailable".to_string()),
            })
        })?;
        let now = ::std::time::SystemTime::now();
        let store = state.lock();

        let not_found = || {
            GetHarnessErrorModeled::ResourceNotFoundException(crate::state::harness_not_found(
                &input.harness_id,
            ))
        };
        let record = store.harnesses.get(&input.harness_id).ok_or_else(not_found)?;
        // A `harnessVersion` no stored version matches is as absent as an unknown id.
        if !record.holds_version(input.harness_version.as_deref()) {
            return Err(not_found().into());
        }

        Ok(GetHarnessOutput {
            harness: record.harness(now),
        })
    }
}

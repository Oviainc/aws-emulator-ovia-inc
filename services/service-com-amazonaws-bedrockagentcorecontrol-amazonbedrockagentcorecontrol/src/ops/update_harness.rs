// SPDX-License-Identifier: CC0-1.0

//! @op UpdateHarness
//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and
//! asserts NOTHING about `UpdateHarness`'s real behaviour. Implement it here — this is
//! the ONLY file to touch for this operation.
//!
//! A 501 from this operation means "nobody has written it yet", never "the emulator is
//! broken". Replace the `Err(..)` in [`UpdateHarness::handle`] with real behaviour and
//! delete the SCAFFOLDED marker line above; that line is the machine-readable record of
//! which operations are still stubs (`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' */src/ops/`),
//! so leaving it on an implemented operation under-reports progress.
//!
//! HAND-AUTHORED, not generated: the scaffold's op-stub generator declined all four
//! harness operations ("shape missing from model: smithy.api#Unit"), so `UpdateHarness`
//! was not in the service's `operations(..)` allow-list at all — and a route the model
//! binds but the service does not wire is claimed by no service, which the router answers
//! `404 UnknownService` in its own envelope, an error neither this operation nor any AWS
//! model declares. This file is the typed skeleton that puts `PATCH /harnesses/{harnessId}`
//! back on the routing table, so the request is validated against the model and answered
//! 501 — "not written yet" — instead. Implementing it means publishing a second version of
//! the stored harness ([`crate::state::HarnessRecord`]), which
//! [`crate::ops::create_harness`] writes at version 1, and honouring the three
//! `Updated*` PATCH wrappers (`optionalValue` present sets, absent clears, the wrapper
//! itself absent leaves unchanged).
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `UpdateHarness::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! ---
//!
//! # Modeled contract for `UpdateHarness`
//!
//! From this service's Smithy model
//! (`smithy_query(query="operation",operation="UpdateHarness",model_path="services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json")`).
//!
//! Operation to update a harness. `PATCH /harnesses/{harnessId}`, success status 200.
//!
//! ## Input
//!
//! * `harnessId` (HarnessId, @httpLabel, pattern
//!   ^[a-zA-Z][a-zA-Z0-9_]{0,39}-[a-zA-Z0-9]{10}$) **required** — the harness to update.
//! * `clientToken` (ClientToken, len 33..256, @idempotencyToken)
//! * `executionRoleArn` (RoleArn, len 1..2048) — retained when not specified.
//! * `environment` (HarnessEnvironmentProviderRequest) — retained when not specified.
//! * `environmentArtifact` (UpdatedHarnessEnvironmentArtifact) — PATCH wrapper.
//! * `environmentVariables` (EnvironmentVariablesMap, len 0..50) — replaces all when given.
//! * `authorizerConfiguration` (UpdatedAuthorizerConfiguration) — PATCH wrapper.
//! * `model` (HarnessModelConfiguration), `systemPrompt`, `tools`, `skills`,
//!   `allowedTools`, `truncation`, `hooks` — each replaces the stored value when given.
//! * `memory` (UpdatedHarnessMemoryConfiguration) — PATCH wrapper.
//! * `maxIterations`, `maxTokens`, `timeoutSeconds` (Integer) — retained when not given.
//!
//! ## Output
//!
//! * `harness` (Harness) **required** — the updated harness.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — the request is denied per access permissions.
//! * `ConflictException` (HTTP 409) — the request conflicts with the resource's state.
//! * `InternalServerException` (HTTP 500) — an unexpected error during processing.
//! * `ResourceNotFoundException` (HTTP 404) — a resource the operation references does not
//!   exist.
//! * `ServiceQuotaExceededException` (HTTP 402) — a service quota was exceeded.
//! * `ThrottlingException` (HTTP 429) — the number of requests exceeds the limit.
//! * `ValidationException` (HTTP 400) — the input fails to satisfy the service's
//!   constraints.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json");

/// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentcorecontrol#UpdateHarness")]
#[derive(Debug)]
pub enum UpdateHarnessErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it. Wrapping
/// the modeled enum is what lets a scaffolded operation answer 501.
pub type UpdateHarnessError = crate::scaffolded::Scaffolded<UpdateHarnessErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<UpdateHarnessErrorModeled> for UpdateHarnessError {
    fn from(modeled: UpdateHarnessErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateHarness` operation.
pub struct UpdateHarness<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateHarness<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateHarness<Eng>
{
    type Input = UpdateHarnessInput;
    type Output = UpdateHarnessOutput;
    type Error = UpdateHarnessError;

    const NAME: &'static str = "UpdateHarness";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::UPDATE_HARNESS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::UPDATE_HARNESS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The engine is reachable from `&self` (constructor-injected); `ctx` carries
        // request metadata. Both are bound here so an implementer has them in scope.
        let _ = (&input, ctx, &self.engine);
        // Implement `UpdateHarness` here, then delete the SCAFFOLDED marker at the top of
        // this file. Return modeled failures as
        // `Err(UpdateHarnessErrorModeled::Variant(..).into())`.
        Err(crate::scaffolded::Scaffolded::not_implemented("UpdateHarness"))
    }
}

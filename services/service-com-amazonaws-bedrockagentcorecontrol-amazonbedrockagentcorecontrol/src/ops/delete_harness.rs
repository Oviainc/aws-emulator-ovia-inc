// SPDX-License-Identifier: CC0-1.0

//! @op DeleteHarness
//!
//! Removes one stored harness from [`crate::state`] and reports the harness it removed,
//! in the `DELETING` status a just-deleted resource carries. A second delete of the same
//! harness finds nothing and answers `ResourceNotFoundException`, as does any later read.
//! A harness that still holds endpoints is not removed at all: its children are deleted
//! first ([`crate::ops::delete_harness_endpoint`]).
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
//! `DeleteHarness::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. Only the `handle` body is this file's.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach the
//! store through [`crate::state::service_state`]. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `DeleteHarness`
//!
//! From this service's Smithy model
//! (`smithy_query(query="operation",operation="DeleteHarness",model_path="services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json")`).
//!
//! Operation to delete a Harness. `DELETE /harnesses/{harnessId}`, success status 200.
//!
//! ## Input
//!
//! * `harnessId` (HarnessId, @httpLabel, pattern
//!   ^[a-zA-Z][a-zA-Z0-9_]{0,39}-[a-zA-Z0-9]{10}$) **required** — the harness to delete.
//! * `clientToken` (ClientToken, @httpQuery `clientToken`, len 33..256, @idempotencyToken)
//! * `deleteManagedMemory` (Boolean, @httpQuery `deleteManagedMemory`) — whether to delete
//!   the managed memory on harness deletion. Default: true. If false, the memory is
//!   disassociated and becomes a regular customer-owned resource.
//!
//! ## Output
//!
//! * `harness` (Harness) — the harness that was deleted.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — the request is denied per access permissions.
//! * `ConflictException` (HTTP 409) — the request conflicts with the resource's state.
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
#[smithy_macros::operation_error("com.amazonaws.bedrockagentcorecontrol#DeleteHarness")]
#[derive(Debug)]
pub enum DeleteHarnessErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type DeleteHarnessError = crate::scaffolded::Scaffolded<DeleteHarnessErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<DeleteHarnessErrorModeled> for DeleteHarnessError {
    fn from(modeled: DeleteHarnessErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteHarness` operation.
pub struct DeleteHarness<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteHarness<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteHarness<Eng>
{
    type Input = DeleteHarnessInput;
    type Output = DeleteHarnessOutput;
    type Error = DeleteHarnessError;

    const NAME: &'static str = "DeleteHarness";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DELETE_HARNESS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DELETE_HARNESS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteHarnessErrorModeled::InternalServerException(InternalServerException {
                message: Some("the harness store is unavailable".to_string()),
            })
        })?;
        let mut store = state.lock();

        // An endpoint is addressed only through the harness that holds it, so a harness
        // that still has one is not destroyed: the delete is refused as the conflict with
        // the harness's own state that it is, and the caller deletes the endpoints first,
        // rather than the endpoints being silently cascaded away with the harness they
        // front.
        if let Some(harness) = store.harnesses.get(&input.harness_id) {
            if !harness.endpoints.is_empty() {
                return Err(DeleteHarnessErrorModeled::ConflictException(ConflictException {
                    message: Some(crate::state::harness_has_endpoints(&harness.harness_name)),
                })
                .into());
            }
        }

        let record = store.harnesses.remove(&input.harness_id).ok_or_else(|| {
            DeleteHarnessErrorModeled::ResourceNotFoundException(crate::state::harness_not_found(
                &input.harness_id,
            ))
        })?;
        store.harness_names.remove(&record.harness_name);
        // An idempotency record naming a harness that is gone can never replay anything
        // again, and nothing else would reclaim it.
        store.forget_harness_client_tokens_for(&record.harness_id);

        // `deleteManagedMemory` decides what happens to a memory resource the SERVICE
        // created for this harness (`memory.managedMemoryConfiguration`). This emulator
        // creates no memory resource for a harness — `CreateMemory` is a separate,
        // unimplemented operation — so there is nothing to delete or to disassociate
        // either way, and the member changes nothing here.
        let _ = input.delete_managed_memory;

        Ok(DeleteHarnessOutput {
            // The harness this call just removed, in the status a deletion puts it in.
            harness: Some(
                record.harness_with_status(crate::state::STATUS_DELETING.to_string()),
            ),
        })
    }
}

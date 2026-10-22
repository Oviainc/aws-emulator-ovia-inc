// SPDX-License-Identifier: CC0-1.0

//! @op DeletePolicyStore
//!
//! `@idempotent`: removes the store and everything it holds, so a following
//! `GetPolicyStore`/`GetPolicy`/`ListPolicies` answers
//! `ResourceNotFoundException`. Deleting a store that is already gone succeeds —
//! the model declares no not-found error for this operation.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeletePolicyStore`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Deletes the specified policy store. This operation is idempotent. If you specify a
//! policy store that does not exist, the request response will still return a successful
//! HTTP 200 status code.
//!
//! ## Input
//!
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that you want to delete. To specify a policy store,
//!   the alias name cannot be used. Only the ID can be used.
//!
//! ## Errors
//!
//! * `InvalidStateException` (HTTP 406) — The policy store can't be deleted because deletion
//!   protection is enabled. To delete this policy store, disable deletion protection.

#[allow(unused_imports)]
use super::types::*;

/// The deletion-protection refusal message, as a CONSTANT.
///
/// `com.amazonaws.verifiedpermissions#InvalidStateException` documents one fixed
/// sentence pair for this refusal ("The policy store can't be deleted because
/// deletion protection is enabled. ..."), and the recorded service answers the
/// same constant on every call; the shape's only member is `message`, and
/// `DeletePolicyStoreInput` carries nothing the service could report back. So no
/// request value and no minted identifier belongs in this text: a client, test or
/// log rule that matches it must match it identically for every store. The
/// wording below is the sentence pair the service emits ON THE WIRE, which is
/// what a client reads; the shape's documentation states the same fixed refusal
/// in prose ("The policy store can't be deleted because deletion protection is
/// enabled. To delete this policy store, disable deletion protection."), so
/// taking the doc's phrasing instead would diverge from the recorded bytes while
/// asserting the same rule.
const DELETION_PROTECTION_MESSAGE: &str = "The policy store cannot be deleted because deletion \
     protection is enabled. Deletion protection must be disabled first.";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeletePolicyStoreError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeletePolicyStoreError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#DeletePolicyStore",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum DeletePolicyStoreErrorModeled {
    InvalidStateException(InvalidStateException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_policy_store::DeletePolicyStoreError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeletePolicyStoreError = crate::scaffolded::Scaffolded<DeletePolicyStoreErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeletePolicyStoreErrorModeled> for DeletePolicyStoreError {
    fn from(modeled: DeletePolicyStoreErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeletePolicyStore` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeletePolicyStore<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeletePolicyStore<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeletePolicyStore<Eng>
{
    type Input = DeletePolicyStoreInput;
    type Output = DeletePolicyStoreOutput;
    type Error = DeletePolicyStoreError;

    const NAME: &'static str = "DeletePolicyStore";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_POLICY_STORE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_POLICY_STORE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DeletePolicyStoreErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the service state store is unavailable".to_string(),
                },
            )
            .into());
        };
        let mut state = state.lock();

        // No such store is a success: `DeletePolicyStore` is `@idempotent` and
        // models no not-found error, so a repeat delete must not fail.
        let Some(store_id) = state.resolve_store_id(&input.policy_store_id) else {
            crate::ops::audit(
                ctx,
                "DeletePolicyStore",
                &input.policy_store_id,
                "already absent",
            );
            return Ok(DeletePolicyStoreOutput {});
        };

        // Deletion protection is the one failure this operation models.
        if state
            .policy_stores
            .get(&store_id)
            .is_some_and(|store| store.deletion_protection == "ENABLED")
        {
            crate::ops::audit(
                ctx,
                "DeletePolicyStore",
                &store_id,
                "refused: deletion protection enabled",
            );
            return Err(DeletePolicyStoreErrorModeled::InvalidStateException(
                InvalidStateException {
                    message: DELETION_PROTECTION_MESSAGE.to_string(),
                },
            )
            .into());
        }

        // The store owns its policies, so removing it removes them too, and the
        // aliases that named it stop resolving.
        state.policy_stores.remove(&store_id);
        state
            .aliases
            .retain(|_, alias| alias.policy_store_id != store_id);
        crate::ops::audit(ctx, "DeletePolicyStore", &store_id, "deleted");

        Ok(DeletePolicyStoreOutput {})
    }
}

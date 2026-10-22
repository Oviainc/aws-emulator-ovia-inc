// SPDX-License-Identifier: CC0-1.0

//! @op DeletePolicy
//!
//! Removes a policy from its store, so a following `GetPolicy` answers
//! `ResourceNotFoundException` and `IsAuthorized` no longer evaluates it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeletePolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Deletes the specified policy from the policy store. This operation is idempotent; if you
//! specify a policy that doesn't exist, the request response returns a successful HTTP 200
//! status code.
//!
//! ## Input
//!
//! * `policyId` (PolicyId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** — Specifies
//!   the ID of the policy that you want to delete. You can use the policy name in place of
//!   the policy ID. When using a name, prefix it with name/. For example: ID:
//!   SPEXAMPLEabcdefg111111 Name: name/example-policy
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that contains the policy that you want to delete.
//!   To specify a policy store, use its ID or alias name. When using an alias name, prefix it
//!   with policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — The request failed because another request to modify a
//!   resource occurred at the same time.
//! * `ResourceNotFoundException` (HTTP 404) — The request failed because it references a
//!   resource that doesn't exist.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeletePolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeletePolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#DeletePolicy",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum DeletePolicyErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_policy::DeletePolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeletePolicyError = crate::scaffolded::Scaffolded<DeletePolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeletePolicyErrorModeled> for DeletePolicyError {
    fn from(modeled: DeletePolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeletePolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeletePolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeletePolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeletePolicy<Eng>
{
    type Input = DeletePolicyInput;
    type Output = DeletePolicyOutput;
    type Error = DeletePolicyError;

    const NAME: &'static str = "DeletePolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_POLICY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store_not_found = || {
            DeletePolicyErrorModeled::ResourceNotFoundException(crate::ops::resource_not_found(
                &input.policy_store_id,
                "POLICY_STORE",
            ))
        };
        let policy_not_found = || {
            DeletePolicyErrorModeled::ResourceNotFoundException(crate::ops::resource_not_found(
                &input.policy_id,
                "POLICY",
            ))
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeletePolicyErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let mut state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(store_not_found)?;
        let store = state
            .policy_stores
            .get_mut(&store_id)
            .ok_or_else(store_not_found)?;
        let policy_id = store
            .resolve_policy_id(&input.policy_id)
            .ok_or_else(policy_not_found)?;
        store.policies.remove(&policy_id);
        crate::ops::audit(ctx, "DeletePolicy", &policy_id, "deleted");

        Ok(DeletePolicyOutput {})
    }
}

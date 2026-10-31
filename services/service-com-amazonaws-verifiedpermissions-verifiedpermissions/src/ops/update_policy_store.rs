// SPDX-License-Identifier: CC0-1.0

//! @op UpdatePolicyStore
//!
//! Modifies a policy store in place: `validationSettings` is required and
//! replaces the store's mode, `description` is replaced by what the request
//! carries (and cleared when it carries none), and `deletionProtection` is the
//! one member the model documents as "unchanged unless explicitly included in
//! the call". The store's `createdDate` is untouched and its `lastUpdatedDate`
//! moves to now; the response is read back out of the persisted store.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdatePolicyStore`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Modifies the validation setting for a policy store. Verified Permissions is eventually
//! consistent . It can take a few seconds for a new or changed element to propagate through
//! the service and be visible in the results of other Verified Permissions operations.
//!
//! ## Input
//!
//! * `deletionProtection` (DeletionProtection, one of: DISABLED | ENABLED) — Specifies
//!   whether the policy store can be deleted. If enabled, the policy store can't be deleted.
//!   When you call UpdatePolicyStore, this parameter is unchanged unless explicitly included
//!   in the call.
//! * `description` (PolicyStoreDescription, len 0..150) — Descriptive text that you can
//!   provide to help with identification of the current policy store.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that you want to update To specify a policy store,
//!   use its ID or alias name. When using an alias name, prefix it with policy-store-alias/.
//!   For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//! * `validationSettings` (ValidationSettings) **required** — A structure that defines the
//!   validation settings that want to enable for the policy store.
//!
//! ## Output
//!
//! * `arn` (ResourceArn, len 1..2500, pattern ^arn:[^:]*:[^:]*:[^:]*:[^:]*:.*$) **required**
//!   — The Amazon Resource Name (ARN) of the updated policy store.
//! * `createdDate` (TimestampFormat) **required** — The date and time that the policy store
//!   was originally created.
//! * `lastUpdatedDate` (TimestampFormat) **required** — The date and time that the policy
//!   store was most recently updated.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   The ID of the updated policy store.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — The request failed because another request to modify a
//!   resource occurred at the same time.
//! * `ResourceNotFoundException` (HTTP 404) — The request failed because it references a
//!   resource that doesn't exist.
//!
//! ## Which members this call replaces
//!
//! `validationSettings` is `@required`, so every call replaces the store's
//! validation mode. `deletionProtection` is the one member the model documents
//! as sticky — "this parameter is unchanged unless explicitly included in the
//! call" — so an absent one leaves the stored flag alone. `description` carries
//! no such sentence, in deliberate contrast, so it is replaced by what the
//! request carries, which is also the only way a caller can clear it.
//!
//! `ConflictException` is modelled for "another request to modify a resource
//! occurred at the same time". This emulator serves a request under the state
//! lock, so two updates are never in flight at once and that error has no
//! occasion to be raised here.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdatePolicyStoreError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdatePolicyStoreError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#UpdatePolicyStore",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum UpdatePolicyStoreErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_policy_store::UpdatePolicyStoreError`).
pub type UpdatePolicyStoreError = crate::scaffolded::Scaffolded<UpdatePolicyStoreErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper.
impl From<UpdatePolicyStoreErrorModeled> for UpdatePolicyStoreError {
    fn from(modeled: UpdatePolicyStoreErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdatePolicyStore` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdatePolicyStore<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdatePolicyStore<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdatePolicyStore<Eng>
{
    type Input = UpdatePolicyStoreInput;
    type Output = UpdatePolicyStoreOutput;
    type Error = UpdatePolicyStoreError;

    const NAME: &'static str = "UpdatePolicyStore";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_POLICY_STORE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_POLICY_STORE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let not_found = || {
            UpdatePolicyStoreErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(UpdatePolicyStoreErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the service state store is unavailable".to_string(),
                },
            )
            .into());
        };
        let mut state = state.lock();

        // `policyStoreId` takes an id or a `policy-store-alias/<name>`, which is
        // what `resolve_store_id` answers for; an unknown reference is the 404
        // this operation models.
        let store_id = state
            .resolve_store_id(
                &crate::state::request_region(ctx.parts()),
                &input.policy_store_id,
            )
            .ok_or_else(not_found)?;
        let store = state
            .policy_stores
            .get_mut(&store_id)
            .ok_or_else(not_found)?;

        store.validation_mode = input.validation_settings.mode.clone();
        // Only `deletionProtection` is documented as "unchanged unless explicitly
        // included in the call", so `description` follows the request: a call
        // that carries none leaves the store without one.
        store.description = input.description.clone();
        if let Some(protection) = input.deletion_protection.clone() {
            store.deletion_protection = protection;
        }
        store.last_updated_date = crate::state::now();
        crate::ops::audit(ctx, "UpdatePolicyStore", &store_id, "updated");

        // Built from the store as it now stands in state, so a field that did
        // not reach state cannot appear in the response.
        Ok(UpdatePolicyStoreOutput {
            policy_store_id: store.policy_store_id.clone(),
            arn: store.arn.clone(),
            created_date: store.created_date,
            last_updated_date: store.last_updated_date,
        })
    }
}

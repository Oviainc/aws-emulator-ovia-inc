// SPDX-License-Identifier: CC0-1.0

//! @op GetPolicyStore
//!
//! Hand-authored. The scaffold op-stub generator declined this operation because
//! `GetPolicyStoreOutput$encryptionState` targets a union with a `smithy.api#Unit`
//! arm (see `src/ops/SKIPPED_STUBS.md`); the output type in
//! `types/get_policy_store_types.rs` declares that one member `unsupported` and
//! serves every other one.
//!
//! `@readonly`: looks the store up in [`crate::state`] and reports exactly what
//! was persisted. It writes nothing.
//!
//! ## Input
//!
//! * `policyStoreId` (PolicyStoreId, len 1..200) **required** — the store id, or
//!   `policy-store-alias/<name>`.
//! * `tags` (Boolean, default false) — include the store's tags in the response.
//!
//! ## Output
//!
//! * `policyStoreId`, `arn`, `validationSettings`, `createdDate`,
//!   `lastUpdatedDate` — all **required**.
//! * `description`, `deletionProtection`, `cedarVersion`, `tags` — present when set.
//! * `encryptionState` (EncryptionState) — not served by this slice.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — no such policy store.

#[allow(unused_imports)]
use super::types::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

/// The operation's MODELED errors.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#GetPolicyStore",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum GetPolicyStoreErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type GetPolicyStoreError = crate::scaffolded::Scaffolded<GetPolicyStoreErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetPolicyStoreErrorModeled> for GetPolicyStoreError {
    fn from(modeled: GetPolicyStoreErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetPolicyStore` operation.
pub struct GetPolicyStore<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetPolicyStore<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetPolicyStore<Eng>
{
    type Input = GetPolicyStoreInput;
    type Output = GetPolicyStoreOutput;
    type Error = GetPolicyStoreError;

    const NAME: &'static str = "GetPolicyStore";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_POLICY_STORE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_POLICY_STORE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let not_found = || {
            GetPolicyStoreErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: format!(
                    "No policy store exists with the input ID: {}",
                    input.policy_store_id
                ),
                resource_id: input.policy_store_id.clone(),
                resource_type: "POLICY_STORE".to_string(),
            })
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetPolicyStoreErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
        let store = state.policy_stores.get(&store_id).ok_or_else(not_found)?;

        Ok(GetPolicyStoreOutput {
            policy_store_id: store.policy_store_id.clone(),
            arn: store.arn.clone(),
            validation_settings: ValidationSettings {
                mode: store.validation_mode.clone(),
            },
            created_date: store.created_date,
            last_updated_date: store.last_updated_date,
            description: store.description.clone(),
            deletion_protection: Some(store.deletion_protection.clone()),
            // `cedarVersion` is reported only once a schema pins it, and
            // `PutSchema` is a later slice, so no store carries one yet.
            cedar_version: None,
            // The tags are reported only when the request asked for them.
            tags: input
                .tags
                .then(|| store.tags.clone().into_iter().collect()),
        })
    }
}

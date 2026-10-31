// SPDX-License-Identifier: CC0-1.0

//! @op GetPolicyStore
//!
//! Hand-authored. The scaffold op-stub generator declined this operation because
//! `GetPolicyStoreOutput$encryptionState` targets a union with a `smithy.api#Unit`
//! arm (see `src/ops/SKIPPED_STUBS.md`); the output type and its schema are
//! written out in `types/get_policy_store_types.rs`, which serves every modelled
//! member of this output, that union included.
//!
//! `@readonly`: looks the store up in [`crate::state`] and reports exactly what
//! was persisted. It writes nothing.
//!
//! ## Input
//!
//! * `policyStoreId` (PolicyStoreId, len 1..200) **required** — the store id, or
//!   `policy-store-alias/<name>`.
//! * `tags` (Boolean, default false) — include the store's tags in the response.
//!   When asked for on a store that carries none, the output member is omitted
//!   rather than reported as an empty map.
//!
//! ## Output
//!
//! * `policyStoreId`, `arn`, `validationSettings`, `createdDate`,
//!   `lastUpdatedDate` — all **required**.
//! * `description`, `deletionProtection`, `cedarVersion`, `encryptionState`,
//!   `tags` — present when set.
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
        // The reference this request named goes in `resourceId`, which the model
        // makes required for exactly that, so the message is the constant every
        // `ResourceNotFoundException` of this service carries — see
        // [`crate::state::RESOURCE_NOT_FOUND_MESSAGE`] for the recordings it
        // comes from, and `crate::ops::resource_not_found` for the one builder
        // that fills it.
        let not_found = || {
            // The reference may name a store by id or an alias by
            // `policy-store-alias/<name>`: the error reports the kind the
            // reference named and that resource's own id, not the path.
            GetPolicyStoreErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetPolicyStoreErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();
        let store_id = state
            .resolve_store_id(
                &crate::state::request_region(ctx.parts()),
                &input.policy_store_id,
            )
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
            // Both of these are properties of the store that no request member
            // sets, so `CreatePolicyStore` persists the service's own default
            // for each and this reports whatever the record holds.
            encryption_state: Some(match &store.encryption {
                crate::state::StoreEncryption::AwsOwnedKey => EncryptionState::Default,
                crate::state::StoreEncryption::CustomerManagedKey {
                    key,
                    encryption_context,
                } => EncryptionState::KmsEncryptionState(KmsEncryptionState {
                    key: key.clone(),
                    encryption_context: encryption_context.clone().into_iter().collect(),
                }),
            }),
            cedar_version: Some(store.cedar_version.clone()),
            // The tags are reported only when the request asked for them, AND
            // only when the store carries any: the model documents that a store
            // with no tags answers with the member OMITTED, not with an empty
            // map, so "absent" and "present but empty" stay distinguishable.
            tags: (input.tags && !store.tags.is_empty())
                .then(|| store.tags.clone().into_iter().collect()),
        })
    }
}

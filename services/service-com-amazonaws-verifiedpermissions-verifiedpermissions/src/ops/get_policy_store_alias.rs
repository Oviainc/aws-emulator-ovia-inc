// SPDX-License-Identifier: CC0-1.0

//! @op GetPolicyStoreAlias
//!
//! `@readonly`: looks the alias up in [`crate::state`] and reports exactly what
//! was persisted — including its `state`, so a soft-deleted alias is visible as
//! `PendingDeletion` rather than as absent. It writes nothing.
//!
//! Implemented alongside `CreatePolicyStoreAlias` as that operation's reader: a
//! created alias is only real if it can be read back.
//!
//! ## Input
//!
//! * `aliasName` (Alias, len 0..150, pattern ^[a-zA-Z0-9-/_]*$) **required** — the
//!   alias name, prefixed with `policy-store-alias/`
//!   ([`crate::state::ALIAS_PREFIX`]).
//!
//! ## Output
//!
//! * `aliasArn`, `aliasName`, `createdAt`, `policyStoreId`, `state` — all
//!   **required**. `state` is `Active` or `PendingDeletion`.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — no alias of that name exists.

#[allow(unused_imports)]
use super::types::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

/// The operation's MODELED errors. `InternalServerException` is a service-level
/// error this operation's own `errors` list does not repeat, so it comes in
/// through `extra_errors`.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#GetPolicyStoreAlias",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum GetPolicyStoreAliasErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_policy_store_alias::GetPolicyStoreAliasError`).
pub type GetPolicyStoreAliasError = crate::scaffolded::Scaffolded<GetPolicyStoreAliasErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetPolicyStoreAliasErrorModeled> for GetPolicyStoreAliasError {
    fn from(modeled: GetPolicyStoreAliasErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetPolicyStoreAlias` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`].
pub struct GetPolicyStoreAlias<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetPolicyStoreAlias<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetPolicyStoreAlias<Eng>
{
    type Input = GetPolicyStoreAliasInput;
    type Output = GetPolicyStoreAliasOutput;
    type Error = GetPolicyStoreAliasError;

    const NAME: &'static str = "GetPolicyStoreAlias";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_POLICY_STORE_ALIAS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_POLICY_STORE_ALIAS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetPolicyStoreAliasErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();
        let alias = state.aliases.get(&input.alias_name).ok_or_else(|| {
            GetPolicyStoreAliasErrorModeled::ResourceNotFoundException(
                crate::ops::resource_not_found(&input.alias_name, "POLICY_STORE_ALIAS"),
            )
        })?;

        Ok(GetPolicyStoreAliasOutput {
            alias_name: alias.alias_name.clone(),
            policy_store_id: alias.policy_store_id.clone(),
            alias_arn: alias.arn.clone(),
            created_at: alias.created_at,
            state: alias.state.clone(),
        })
    }
}

// SPDX-License-Identifier: CC0-1.0

//! @op DeletePolicyStoreAlias
//!
//! Deletes a policy store alias in [`crate::state`], in the mode the request
//! asks for. Implemented alongside `CreatePolicyStoreAlias` because it is what
//! moves an alias into `PendingDeletion`, the state that create's name conflict
//! and `GetPolicyStoreAlias`'s `state` member report.
//!
//! The model documents the three rules this handler implements:
//!
//! * **`@idempotent`** — "If you specify a policy store alias that does not
//!   exist, the request response will still return a successful HTTP 200 status
//!   code", so a repeat delete is a success, not a not-found.
//! * **`SoftDelete` (the default)** — the alias "enters the `PendingDeletion`
//!   state". It stays in state, so `GetPolicyStoreAlias` still reports it and its
//!   name stays taken; it stops resolving in every `policyStoreId` member, which
//!   the model states fails "with a `ResourceNotFound` exception". AWS publishes
//!   no window after which a `PendingDeletion` alias disappears on its own, so
//!   this emulator keeps it until a `HardDelete` removes it rather than inventing
//!   an expiry.
//! * **`HardDelete`** — the alias "is immediately deleted, bypassing the
//!   `PendingDeletion` state", so the record is removed and its name is free
//!   again.
//!
//! ## Input
//!
//! * `aliasName` (Alias, len 0..150, pattern ^[a-zA-Z0-9-/_]*$) **required** — the
//!   alias name, which "must always be prefixed with `policy-store-alias/`"
//!   ([`crate::state::ALIAS_PREFIX`]). A value without that prefix is refused as
//!   a malformed member ([`crate::ops::invalid_alias_name`]): the documented
//!   idempotent success is for a name that is well formed and names nothing, not
//!   for one no alias could carry.
//! * `deletionMode` (DeletionMode, one of: SoftDelete | HardDelete) — absent
//!   means `SoftDelete`.
//!
//! ## Output
//!
//! * empty.
//!
//! ## Errors
//!
//! * `ValidationException` (HTTP 400) — the name does not carry the documented
//!   prefix, so it is not a well-formed alias name.
//! * `InvalidStateException` (HTTP 406) is modeled, and this handler answers it
//!   for no request: the model documents it as the deletion-protection refusal,
//!   which is a property of a policy store (`DeletePolicyStore` answers it), and
//!   an alias carries no deletion protection of its own.

#[allow(unused_imports)]
use super::types::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

/// The operation's MODELED errors. `InternalServerException` is a service-level
/// error this operation's own `errors` list does not repeat, so it comes in
/// through `extra_errors`.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#DeletePolicyStoreAlias",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum DeletePolicyStoreAliasErrorModeled {
    InvalidStateException(InvalidStateException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_policy_store_alias::DeletePolicyStoreAliasError`).
pub type DeletePolicyStoreAliasError =
    crate::scaffolded::Scaffolded<DeletePolicyStoreAliasErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<DeletePolicyStoreAliasErrorModeled> for DeletePolicyStoreAliasError {
    fn from(modeled: DeletePolicyStoreAliasErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeletePolicyStoreAlias` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`].
pub struct DeletePolicyStoreAlias<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeletePolicyStoreAlias<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeletePolicyStoreAlias<Eng>
{
    type Input = DeletePolicyStoreAliasInput;
    type Output = DeletePolicyStoreAliasOutput;
    type Error = DeletePolicyStoreAliasError;

    const NAME: &'static str = "DeletePolicyStoreAlias";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DELETE_POLICY_STORE_ALIAS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DELETE_POLICY_STORE_ALIAS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The member's own documented prefix is a request-SHAPE rule, so it is
        // answered before state is consulted — and before the documented
        // idempotent success, which is about an alias that is absent, not about a
        // name that could never be one.
        if let Some(invalid) = crate::ops::invalid_alias_name(&input.alias_name) {
            return Err(DeletePolicyStoreAliasErrorModeled::ValidationException(invalid).into());
        }

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DeletePolicyStoreAliasErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the service state store is unavailable".to_string(),
                },
            )
            .into());
        };
        let mut state = state.lock();

        // Absent means `SoftDelete`: the model calls it "the default behavior
        // when no deletionMode is specified".
        let hard_delete = input.deletion_mode.as_deref() == Some("HardDelete");

        if !state.aliases.contains_key(&input.alias_name) {
            crate::ops::audit(
                ctx,
                "DeletePolicyStoreAlias",
                &input.alias_name,
                "already absent",
            );
            return Ok(DeletePolicyStoreAliasOutput {});
        }

        if hard_delete {
            state.aliases.remove(&input.alias_name);
            crate::ops::audit(ctx, "DeletePolicyStoreAlias", &input.alias_name, "deleted");
        } else if let Some(alias) = state.aliases.get_mut(&input.alias_name) {
            alias.state = crate::state::ALIAS_STATE_PENDING_DELETION.to_string();
            crate::ops::audit(
                ctx,
                "DeletePolicyStoreAlias",
                &input.alias_name,
                "pending deletion",
            );
        }

        Ok(DeletePolicyStoreAliasOutput {})
    }
}

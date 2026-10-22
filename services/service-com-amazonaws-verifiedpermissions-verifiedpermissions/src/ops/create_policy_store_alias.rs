// SPDX-License-Identifier: CC0-1.0

//! @op CreatePolicyStoreAlias
//!
//! Creates a policy store alias — an alternative identifier for a policy store —
//! and INSERTS it into [`crate::state`]: the ARN, name, target store and
//! timestamp this returns are the ones `GetPolicyStoreAlias` reads back, and the
//! ones that make `policy-store-alias/<name>` resolve in every operation taking a
//! `policyStoreId`.
//!
//! The model documents three rules this handler implements:
//!
//! * **Idempotent on `(aliasName, policyStoreId)`** — "If multiple
//!   `CreatePolicyStoreAlias` requests are made where the `aliasName` and
//!   `policyStoreId` fields are the same between the requests, subsequent
//!   requests will be ignored. For each duplicate … a Success response will be
//!   returned and a new policy store alias will not be created." The duplicate
//!   therefore answers with the PERSISTED alias, timestamp included, rather than
//!   minting a second one.
//! * **The name is unique in the account and Region** — a request naming a name
//!   another store already holds conflicts (HTTP 409), which is what makes
//!   concurrent creations of one name have exactly one winner.
//! * **A name in `PendingDeletion` cannot be taken** —
//!   `DeletePolicyStoreAlias` documents that "when a policy store alias is in the
//!   `PendingDeletion` state, new policy store aliases cannot be created with the
//!   same name", so that name conflicts until it is hard-deleted.
//!
//! ## Input
//!
//! * `aliasName` (Alias, len 0..150, pattern ^[a-zA-Z0-9-/_]*$) **required** — the
//!   alias name, which "must always be prefixed with `policy-store-alias/`"
//!   ([`crate::state::ALIAS_PREFIX`]).
//! * `policyStoreId` (PolicyStoreId, len 1..200) **required** — "The associated
//!   policy store must be specified using its ID. The alias name cannot be used",
//!   so this member is looked up as a store ID only.
//!
//! ## Output
//!
//! * `aliasArn`, `aliasName`, `createdAt`, `policyStoreId` — all **required**.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — the name is already held by an alias of
//!   another store, or by an alias awaiting deletion.
//! * `ResourceNotFoundException` (HTTP 404) — no policy store has that ID.
//! * `ServiceQuotaExceededException` (HTTP 402) — the account is at
//!   [`crate::state::MAX_POLICY_STORE_ALIASES`] aliases.
//! * `ValidationException` (HTTP 400) — the name does not carry the documented
//!   prefix, so no `policyStoreId` member could ever reference it.

#[allow(unused_imports)]
use super::types::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

/// The operation's MODELED errors. `InternalServerException` is a service-level
/// error this operation's own `errors` list does not repeat, so it comes in
/// through `extra_errors`; it is the honest answer when the service's state store
/// cannot be reached, which is a server fault and not a bad request.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#CreatePolicyStoreAlias",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum CreatePolicyStoreAliasErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_policy_store_alias::CreatePolicyStoreAliasError`).
pub type CreatePolicyStoreAliasError = crate::scaffolded::Scaffolded<CreatePolicyStoreAliasErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<CreatePolicyStoreAliasErrorModeled> for CreatePolicyStoreAliasError {
    fn from(modeled: CreatePolicyStoreAliasErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreatePolicyStoreAlias` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct CreatePolicyStoreAlias<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreatePolicyStoreAlias<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The conflict answered when `alias_name` is already taken: it names the alias
/// involved, which is the resource the caller has to deal with.
fn already_taken(
    alias_name: &str,
    held_by: &crate::state::PolicyStoreAlias,
) -> CreatePolicyStoreAliasErrorModeled {
    let detail = if held_by.state == crate::state::ALIAS_STATE_PENDING_DELETION {
        format!(
            "it is in the {} state",
            crate::state::ALIAS_STATE_PENDING_DELETION
        )
    } else {
        format!(
            "it is already associated with policy store {}",
            held_by.policy_store_id
        )
    };
    CreatePolicyStoreAliasErrorModeled::ConflictException(ConflictException {
        message: format!(
            "the policy store alias {alias_name} cannot be created because {detail}"
        ),
        resources: vec![ResourceConflict {
            resource_id: held_by.alias_name.clone(),
            resource_type: "POLICY_STORE_ALIAS".to_string(),
        }],
    })
}

/// Render a persisted alias as this operation's output, so a value that was never
/// stored cannot appear on the wire.
fn output_for(alias: &crate::state::PolicyStoreAlias) -> CreatePolicyStoreAliasOutput {
    CreatePolicyStoreAliasOutput {
        alias_name: alias.alias_name.clone(),
        policy_store_id: alias.policy_store_id.clone(),
        alias_arn: alias.arn.clone(),
        created_at: alias.created_at,
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreatePolicyStoreAlias<Eng>
{
    type Input = CreatePolicyStoreAliasInput;
    type Output = CreatePolicyStoreAliasOutput;
    type Error = CreatePolicyStoreAliasError;

    const NAME: &'static str = "CreatePolicyStoreAlias";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::CREATE_POLICY_STORE_ALIAS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::CREATE_POLICY_STORE_ALIAS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let unavailable = || {
            CreatePolicyStoreAliasErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        };

        // The documented prefix is a rule the model's `pattern` cannot express —
        // it admits any `[a-zA-Z0-9-/_]*` — and a name without it could never be
        // referenced through a `policyStoreId` member, so the request is refused
        // rather than persisted as unreachable.
        if !input.alias_name.starts_with(crate::state::ALIAS_PREFIX) {
            return Err(crate::scaffolded::Scaffolded::validation(format!(
                "aliasName must be prefixed with {}, got {}",
                crate::state::ALIAS_PREFIX,
                input.alias_name
            )));
        }

        let state = crate::state::service_state(&*self.engine).ok_or_else(unavailable)?;
        let mut state = state.lock();

        // `policyStoreId` is documented as an ID here — "The alias name cannot be
        // used" — so it is looked up directly instead of through
        // `resolve_store_id`, which would also accept an alias reference.
        if !state.policy_stores.contains_key(&input.policy_store_id) {
            return Err(
                CreatePolicyStoreAliasErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: format!(
                            "No policy store exists with the input ID: {}",
                            input.policy_store_id
                        ),
                        resource_id: input.policy_store_id.clone(),
                        resource_type: "POLICY_STORE".to_string(),
                    },
                )
                .into(),
            );
        }

        // The name is unique in the account: a duplicate of the SAME association
        // is the documented no-op, and anything else holding the name conflicts.
        if let Some(existing) = state.aliases.get(&input.alias_name) {
            if existing.state == crate::state::ALIAS_STATE_ACTIVE
                && existing.policy_store_id == input.policy_store_id
            {
                let output = output_for(existing);
                crate::ops::audit(
                    ctx,
                    "CreatePolicyStoreAlias",
                    &output.alias_name,
                    "already created",
                );
                return Ok(output);
            }
            return Err(already_taken(&input.alias_name, existing).into());
        }

        // The alias table is the one structure this request grows, so the bound is
        // checked before anything is persisted.
        if state.aliases.len() >= crate::state::MAX_POLICY_STORE_ALIASES {
            return Err(
                CreatePolicyStoreAliasErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: format!(
                            "this account already holds the maximum of {} policy store aliases",
                            crate::state::MAX_POLICY_STORE_ALIASES
                        ),
                        resource_type: "POLICY_STORE_ALIAS".to_string(),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        let alias = crate::state::PolicyStoreAlias {
            alias_name: input.alias_name.clone(),
            arn: crate::state::alias_arn(&input.alias_name),
            policy_store_id: input.policy_store_id.clone(),
            created_at: crate::state::now(),
            state: crate::state::ALIAS_STATE_ACTIVE.to_string(),
        };
        state.aliases.insert(input.alias_name.clone(), alias);

        // Read the response back out of what was just persisted.
        let stored = state
            .aliases
            .get(&input.alias_name)
            .ok_or_else(unavailable)?;
        let output = output_for(stored);
        crate::ops::audit(ctx, "CreatePolicyStoreAlias", &output.alias_name, "created");
        Ok(output)
    }
}

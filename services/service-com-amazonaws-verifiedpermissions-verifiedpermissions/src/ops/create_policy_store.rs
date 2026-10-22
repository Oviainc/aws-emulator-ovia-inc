// SPDX-License-Identifier: CC0-1.0

//! @op CreatePolicyStore
//!
//! Hand-authored. The scaffold op-stub generator declined this operation because
//! `CreatePolicyStoreInput$encryptionSettings` targets a union with a
//! `smithy.api#Unit` arm (see `src/ops/SKIPPED_STUBS.md`); the input type in
//! `types/create_policy_store_types.rs` declares that one member `unsupported` and
//! serves every other one, so the operation is wired into the service's
//! `operations(...)` allow-list like any rendered sibling.
//!
//! Creates a policy store and INSERTS it into [`crate::state`]: the id, ARN and
//! timestamps this returns are the ones `GetPolicyStore` reads back.
//!
//! ## Input
//!
//! * `clientToken` (IdempotencyToken, len 1..64, pattern ^[a-zA-Z0-9-]*$, @idempotencyToken)
//! * `validationSettings` (ValidationSettings) **required** — `mode`: `OFF` | `STRICT`.
//! * `description` (PolicyStoreDescription, len 0..150, @sensitive)
//! * `deletionProtection` (DeletionProtection, one of: DISABLED | ENABLED)
//! * `encryptionSettings` (EncryptionSettings) — not served by this slice.
//! * `tags` (TagMap, len ..200)
//!
//! ## Output
//!
//! * `policyStoreId`, `arn`, `createdDate`, `lastUpdatedDate` — all **required**.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — the same `clientToken` was replayed with
//!   different parameters.
//! * `ServiceQuotaExceededException` (HTTP 402).

#[allow(unused_imports)]
use super::types::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

/// The operation's MODELED errors. `InternalServerException` is a service-level
/// error this operation's own `errors` list does not repeat, so it comes in
/// through `extra_errors`; it is the honest answer when the service's state store
/// cannot be reached, which is a server fault and not a bad request.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#CreatePolicyStore",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum CreatePolicyStoreErrorModeled {
    ConflictException(ConflictException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type CreatePolicyStoreError = crate::scaffolded::Scaffolded<CreatePolicyStoreErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<CreatePolicyStoreErrorModeled> for CreatePolicyStoreError {
    fn from(modeled: CreatePolicyStoreErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreatePolicyStore` operation.
pub struct CreatePolicyStore<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreatePolicyStore<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// A canonical rendering of the parameters one `clientToken` was first used with.
/// A replay carrying the same token must carry the same parameters, or the model
/// documents a `ConflictException`.
fn canonical_parameters(input: &CreatePolicyStoreInput) -> String {
    let mut tags: Vec<(&String, &String)> = input
        .tags
        .as_ref()
        .map(|t| t.iter().collect())
        .unwrap_or_default();
    tags.sort();
    format!(
        "mode={};description={:?};deletionProtection={:?};tags={:?}",
        input.validation_settings.mode, input.description, input.deletion_protection, tags
    )
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreatePolicyStore<Eng>
{
    type Input = CreatePolicyStoreInput;
    type Output = CreatePolicyStoreOutput;
    type Error = CreatePolicyStoreError;

    const NAME: &'static str = "CreatePolicyStore";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::CREATE_POLICY_STORE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::CREATE_POLICY_STORE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            crate::scaffolded::Scaffolded::Modeled(
                CreatePolicyStoreErrorModeled::InternalServerException(InternalServerException {
                    message: "the service state store is unavailable".to_string(),
                }),
            )
        })?;
        let mut state = state.lock();

        // `@idempotencyToken` replay: the same token answers with the store the
        // first call created, and the same token with different parameters is the
        // `ConflictException` the model documents.
        let parameters = canonical_parameters(&input);
        if let Some(token) = input.client_token.as_deref() {
            if let Some(record) = state.idempotency_replay("CreatePolicyStore", token) {
                if record.parameters != parameters {
                    // `ConflictException`'s message is the service's fixed
                    // summary; the resource the token already created is
                    // reported in the required `resources` list, which also
                    // keeps the token value itself out of the response.
                    return Err(CreatePolicyStoreErrorModeled::ConflictException(
                        crate::ops::conflict([crate::ops::resource_conflict(
                            &record.resource_id,
                            crate::state::RESOURCE_TYPE_POLICY_STORE,
                        )]),
                    )
                    .into());
                }
                if let Some(store) = state.policy_stores.get(&record.resource_id) {
                    return Ok(CreatePolicyStoreOutput {
                        policy_store_id: store.policy_store_id.clone(),
                        arn: store.arn.clone(),
                        created_date: store.created_date,
                        last_updated_date: store.last_updated_date,
                    });
                }
            }
        }

        // The store map is the one structure this request grows, so the quota is
        // checked before anything is minted.
        if state.policy_stores.len() >= crate::state::MAX_POLICY_STORES {
            return Err(CreatePolicyStoreErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "this account already holds the maximum of {} policy stores",
                        crate::state::MAX_POLICY_STORES
                    ),
                    resource_type: "POLICY_STORE".to_string(),
                    ..Default::default()
                },
            )
            .into());
        }

        let policy_store_id = state.mint_policy_store_id();
        // Verified Permissions policy-store ARNs carry no region segment.
        let arn = ::engine::emulator_arn(
            "verifiedpermissions",
            "",
            &format!("policy-store/{policy_store_id}"),
        );
        let created = crate::state::now();
        let store = crate::state::PolicyStore {
            policy_store_id: policy_store_id.clone(),
            arn: arn.clone(),
            validation_mode: input.validation_settings.mode.clone(),
            description: input.description.clone(),
            deletion_protection: input
                .deletion_protection
                .clone()
                .unwrap_or_else(|| "DISABLED".to_string()),
            created_date: created,
            last_updated_date: created,
            // Set from the service's own default: no request member of any
            // modelled operation carries a Cedar version.
            cedar_version: crate::state::DEFAULT_CEDAR_VERSION.to_string(),
            // A store this operation creates is encrypted with an AWS owned
            // key: the one request member that selects a customer-managed key,
            // `CreatePolicyStoreInput$encryptionSettings`, is not served by this
            // slice (it is declared `unsupported` in
            // `types/create_policy_store_types.rs`), so nothing can ask for one.
            encryption: crate::state::StoreEncryption::default(),
            tags: input
                .tags
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            policies: ::std::collections::BTreeMap::new(),
            policy_templates: ::std::collections::BTreeMap::new(),
            identity_sources: ::std::collections::BTreeMap::new(),
            next_id_seq: 0,
            // A new store holds no schema: `PutSchema` is what puts one there,
            // and until it does the store reports the EMPTY schema document
            // (`crate::state::PolicyStore::schema_document`).
            schema: None,
        };
        state.policy_stores.insert(policy_store_id.clone(), store);
        if let Some(token) = input.client_token.as_deref() {
            state.record_idempotency("CreatePolicyStore", token, &policy_store_id, parameters);
        }

        // Read the response back out of what was just persisted, so a field that
        // did not reach state cannot appear in the response.
        let stored = state.policy_stores.get(&policy_store_id).ok_or_else(|| {
            crate::scaffolded::Scaffolded::Modeled(
                CreatePolicyStoreErrorModeled::InternalServerException(InternalServerException {
                    message: format!("policy store {policy_store_id} could not be persisted"),
                }),
            )
        })?;
        let output = CreatePolicyStoreOutput {
            policy_store_id: stored.policy_store_id.clone(),
            arn: stored.arn.clone(),
            created_date: stored.created_date,
            last_updated_date: stored.last_updated_date,
        };
        crate::ops::audit(ctx, "CreatePolicyStore", &output.policy_store_id, "created");
        Ok(output)
    }
}

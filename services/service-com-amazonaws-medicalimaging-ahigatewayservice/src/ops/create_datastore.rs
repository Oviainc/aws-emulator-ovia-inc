// SPDX-License-Identifier: CC0-1.0

//! @op CreateDatastore
//!
//! Mints a data store, persists it in [`crate::state`] and answers with its id and
//! status. `clientToken` is a real `@idempotencyToken`: a repeat call with the same
//! token returns the SAME `datastoreId` instead of minting a second store.
//!
//! The data store is returned `CREATING`, the status AWS HealthImaging answers with,
//! and the clock owes its move to `ACTIVE` — see
//! [`crate::state::DATASTORE_ACTIVATION_DELAY`]. Nothing here sleeps.
//!
//! The `tags` map is validated before anything is minted, by the rules of the TAG MAP
//! rather than of this operation: a key outside `TagKey`'s modelled constraints, or more
//! entries than one resource may hold ([`crate::state::MAX_TAGS_PER_RESOURCE`]), is
//! refused with `ValidationException` and creates no data store.
//!
//! ---
//!
//! # Modeled contract for `CreateDatastore`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! Create a data store.
//!
//! ## Input
//!
//! * `clientToken` (ClientToken, len 1..64, pattern ^[A-Za-z0-9._-]+$, @idempotencyToken)
//!   **required** — A unique identifier for API idempotency.
//! * `datastoreName` (DatastoreName, len 1..256, pattern ^[A-Za-z0-9._/#-]+$) — The data
//!   store name.
//! * `kmsKeyArn` (KmsKeyArn, len 1..512) — The Amazon Resource Name (ARN) assigned to the
//!   Key Management Service (KMS) key for accessing encrypted data.
//! * `lambdaAuthorizerArn` (LambdaArn) — The ARN of the authorizer's Lambda function.
//! * `losslessStorageFormat` (LosslessStorageFormat, one of: HTJ2K | JPEG_2000_LOSSLESS) —
//!   The lossless storage format for the datastore.
//! * `tags` (TagMap) — The tags provided when creating a data store.
//!
//! ## Output
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `datastoreStatus` (DatastoreStatus, one of: ACTIVE | CREATE_FAILED | CREATING | DELETED
//!   | DELETING) **required** — The data store status.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The user does not have sufficient access to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — Updating or deleting a resource can cause an
//!   inconsistent state.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during processing of
//!   the request.
//! * `ResourceNotFoundException` (HTTP 404) — The request references a resource which does
//!   not exist.
//! * `ServiceQuotaExceededException` (HTTP 402) — The request caused a service quota to be
//!   exceeded.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints set by the
//!   service.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{Datastore, DatastoreStatus};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateDatastoreError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateDatastoreError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#CreateDatastore")]
#[derive(Debug)]
pub enum CreateDatastoreErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_datastore::CreateDatastoreError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateDatastoreError = crate::scaffolded::Scaffolded<CreateDatastoreErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateDatastoreErrorModeled> for CreateDatastoreError {
    fn from(modeled: CreateDatastoreErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateDatastore` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateDatastore<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateDatastore<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateDatastore<Eng>
{
    type Input = CreateDatastoreInput;
    type Output = CreateDatastoreOutput;
    type Error = CreateDatastoreError;

    const NAME: &'static str = "CreateDatastore";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_DATASTORE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_DATASTORE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx);

        // The tag map is validated BEFORE anything is read or written: a refused
        // request mints no data store and consumes no `clientToken`, so an illegal tag
        // map is observable nowhere. Both rules below belong to the TAG MAP rather than
        // to this operation, so they are the same two `TagResource` applies — a map one
        // write path refuses cannot be installed through the other.
        //
        // The ENTRY COUNT first, as the boundary validator checks a collection's size
        // before its elements: one call may not install more tags than a resource
        // holds ([`crate::state::MAX_TAGS_PER_RESOURCE`]).
        if let Some(tags) = &input.tags {
            if tags.len() > crate::state::MAX_TAGS_PER_RESOURCE {
                return Err(CreateDatastoreErrorModeled::ValidationException(
                    ValidationException {
                        message: crate::state::too_many_tags_message(tags.len()),
                    },
                )
                .into());
            }
            // Then the KEYS, against every constraint the model puts on a `TagKey`.
            // The boundary validator never visits a map key, so this is the only place
            // the key rules are applied — see `crate::state::tag_map_violation`.
            if let Some(violation) = crate::state::tag_map_violation("tags", tags) {
                return Err(CreateDatastoreErrorModeled::ValidationException(
                    ValidationException {
                        message: crate::state::one_validation_error(&violation),
                    },
                )
                .into());
            }
        }

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateDatastoreErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        // `@idempotencyToken`: a token already in the ledger names the store it minted,
        // so the repeat call reports that store's CURRENT status rather than creating
        // a second one.
        let token_key = crate::state::client_token_key("CreateDatastore", &input.client_token);
        if let Some(existing_id) = store.client_tokens.get(&token_key).cloned() {
            if let Some(existing) = store.datastores.get(&existing_id) {
                return Ok(CreateDatastoreOutput {
                    datastore_id: existing.datastore_id.clone(),
                    datastore_status: existing.datastore_status.as_wire().to_string(),
                });
            }
        }

        // Bound the memory one caller can make this emulator hold. The cap is the
        // emulator's own ([`crate::state::MAX_DATASTORES`]), refused with the
        // operation's modelled quota error rather than an unmodelled one.
        if store.datastores.len() >= crate::state::MAX_DATASTORES {
            return Err(CreateDatastoreErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "this emulator holds at most {} data stores; delete one before creating another",
                        crate::state::MAX_DATASTORES
                    ),
                },
            )
            .into());
        }

        let datastore_id = store.mint_id();
        let datastore = Datastore {
            datastore_id: datastore_id.clone(),
            datastore_name: input.datastore_name.clone(),
            datastore_status: DatastoreStatus::Creating,
            datastore_arn: crate::state::datastore_arn(&region, &datastore_id),
            kms_key_arn: input.kms_key_arn.clone(),
            lambda_authorizer_arn: input.lambda_authorizer_arn.clone(),
            lossless_storage_format: input.lossless_storage_format.clone(),
            tags: input.tags.clone().unwrap_or_default(),
            created_at: now,
            updated_at: now,
            status_due_at: Some(now + crate::state::DATASTORE_ACTIVATION_DELAY),
            import_jobs: ::std::collections::HashMap::new(),
            image_sets: ::std::collections::HashMap::new(),
        };
        let datastore_status = datastore.datastore_status.as_wire().to_string();

        store.remember_client_token(token_key, datastore_id.clone());
        store.datastores.insert(datastore_id.clone(), datastore);

        Ok(CreateDatastoreOutput {
            datastore_id,
            datastore_status,
        })
    }
}

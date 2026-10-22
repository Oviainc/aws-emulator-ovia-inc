// SPDX-License-Identifier: CC0-1.0

//! @op DeleteDatastore
//!
//! Moves the persisted data store to `DELETING` — the status AWS HealthImaging answers
//! this call with — and leaves the clock to finish the deletion
//! ([`crate::state::DATASTORE_DELETION_DELAY`]). Once that instant passes, the record
//! is gone: `GetDatastore` answers `ResourceNotFoundException` and `ListDatastores` no
//! longer lists it.
//!
//! The model documents a precondition — "before a data store can be deleted, you must
//! first delete all image sets within it" — so a store that still owns image sets is
//! refused with `ConflictException`.
//!
//! ---
//!
//! # Modeled contract for `DeleteDatastore`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! Delete a data store. Before a data store can be deleted, you must first delete all image
//! sets within it.
//!
//! ## Input
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
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
//! * `ThrottlingException` (HTTP 429) — The request was denied due to throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints set by the
//!   service.

#[allow(unused_imports)]
use super::types::*;

use crate::state::DatastoreStatus;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteDatastoreError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteDatastoreError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#DeleteDatastore")]
#[derive(Debug)]
pub enum DeleteDatastoreErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_datastore::DeleteDatastoreError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteDatastoreError = crate::scaffolded::Scaffolded<DeleteDatastoreErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteDatastoreErrorModeled> for DeleteDatastoreError {
    fn from(modeled: DeleteDatastoreErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteDatastore` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteDatastore<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteDatastore<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteDatastore<Eng>
{
    type Input = DeleteDatastoreInput;
    type Output = DeleteDatastoreOutput;
    type Error = DeleteDatastoreError;

    const NAME: &'static str = "DeleteDatastore";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_DATASTORE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_DATASTORE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteDatastoreErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        let datastore = store.datastores.get_mut(&input.datastore_id).ok_or_else(|| {
            DeleteDatastoreErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                // Constant, and stated by resource KIND: the requested id is not
                // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;

        if !datastore.image_sets.is_empty() {
            return Err(DeleteDatastoreErrorModeled::ConflictException(ConflictException {
                message: format!(
                    "Datastore {} still contains image sets; delete them before deleting the data store",
                    datastore.datastore_id
                ),
            })
            .into());
        }

        // `@idempotent`: a repeat delete reports the same `DELETING` without pushing the
        // deletion the clock already owes further out.
        if datastore.datastore_status != DatastoreStatus::Deleting {
            datastore.datastore_status = DatastoreStatus::Deleting;
            datastore.updated_at = now;
            datastore.status_due_at = Some(now + crate::state::DATASTORE_DELETION_DELAY);
        }

        Ok(DeleteDatastoreOutput {
            datastore_id: datastore.datastore_id.clone(),
            datastore_status: datastore.datastore_status.as_wire().to_string(),
        })
    }
}

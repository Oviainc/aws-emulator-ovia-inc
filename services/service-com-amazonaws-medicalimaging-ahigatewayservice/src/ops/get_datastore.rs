// SPDX-License-Identifier: CC0-1.0

//! @op GetDatastore
//!
//! Reports the persisted data store's properties. A reporting operation (`@readonly`):
//! it writes nothing of its own, and only materialises the status changes the clock
//! already owed — see [`crate::state::ComAmazonawsMedicalimagingAhigatewayserviceState::advance_clock`].
//!
//! Every status the model declares is reportable here, `DELETED` included: a deletion
//! that has landed is reported in that status for as long as its record survives
//! ([`crate::state::DATASTORE_DELETED_RETENTION`]), and only once the clock has reaped
//! the record does this answer `ResourceNotFoundException`.
//!
//! ---
//!
//! # Modeled contract for `GetDatastore`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! Get data store properties.
//!
//! ## Input
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//!
//! ## Output
//!
//! * `datastoreProperties` (DatastoreProperties) **required** — The data store properties.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The user does not have sufficient access to perform
//!   this action.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during processing of
//!   the request.
//! * `ResourceNotFoundException` (HTTP 404) — The request references a resource which does
//!   not exist.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints set by the
//!   service.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetDatastoreError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetDatastoreError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#GetDatastore")]
#[derive(Debug)]
pub enum GetDatastoreErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_datastore::GetDatastoreError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetDatastoreError = crate::scaffolded::Scaffolded<GetDatastoreErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetDatastoreErrorModeled> for GetDatastoreError {
    fn from(modeled: GetDatastoreErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetDatastore` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetDatastore<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetDatastore<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetDatastore<Eng>
{
    type Input = GetDatastoreInput;
    type Output = GetDatastoreOutput;
    type Error = GetDatastoreError;

    const NAME: &'static str = "GetDatastore";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_DATASTORE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_DATASTORE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx);
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetDatastoreErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        // Reached in the region this request was addressed to: a data store exists in
        // ONE region, so one held in another is not found here at all
        // (`crate::state::ComAmazonawsMedicalimagingAhigatewayserviceState::reported_datastore_in`).
        //
        // REPORTING: a data store whose deletion has landed is read here too, because
        // `DELETED` is a status this operation reports — for as long as the record
        // survives the deletion (`crate::state::DATASTORE_DELETED_RETENTION`), after
        // which the clock has reaped it and this answers `ResourceNotFoundException`.
        let datastore = store
            .reported_datastore_in(&region, &input.datastore_id)
            .ok_or_else(|| {
                GetDatastoreErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                    // Constant, and stated by resource KIND: the requested id is not
                    // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                    message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
                })
            })?;

        Ok(GetDatastoreOutput {
            datastore_properties: DatastoreProperties {
                datastore_id: datastore.datastore_id.clone(),
                datastore_name: datastore.reported_name(),
                datastore_status: datastore.datastore_status.as_wire().to_string(),
                kms_key_arn: datastore.kms_key_arn.clone(),
                lambda_authorizer_arn: datastore.lambda_authorizer_arn.clone(),
                lossless_storage_format: datastore.lossless_storage_format.clone(),
                datastore_arn: Some(datastore.datastore_arn.clone()),
                created_at: Some(crate::state::to_date_time(datastore.created_at)),
                updated_at: Some(crate::state::to_date_time(datastore.updated_at)),
            },
        })
    }
}

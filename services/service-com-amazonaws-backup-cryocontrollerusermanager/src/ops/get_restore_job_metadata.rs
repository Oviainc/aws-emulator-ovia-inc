// SPDX-License-Identifier: CC0-1.0

//! @op GetRestoreJobMetadata
//!
//! Reports the `Metadata` the restore was started with, read off the stored
//! restore job ([`crate::state::RestoreJobRecord::metadata`]) that
//! `StartRestoreJob` recorded it on — "the metadata that you've specified as
//! key-value pairs" for that restore, so which keys this read answers with are
//! the keys that call was given and nothing synthesised beside them.
//!
//! The VALUES are masked ([`crate::state::METADATA_VALUE_MASK`]): the map is
//! `@sensitive` in the model and its pairs are the caller's own configuration
//! of the restore, so the read discloses the keys a restore was configured with
//! and not what each one was set to.
//!
//! An id naming no stored restore job is the one not-found refusal every call
//! that addresses a restore job shares
//! ([`crate::state::restore_job_not_found`]).
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetRestoreJobMetadata`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! This request returns the metadata for the specified restore job.
//!
//! ## Input
//!
//! * `RestoreJobId` (RestoreJobId) **required** — This is a unique identifier of a restore
//!   job within Backup.
//!
//! ## Output
//!
//! * `Metadata` (Metadata) — This contains the metadata of the specified backup job.
//! * `RestoreJobId` (RestoreJobId) — This is a unique identifier of a restore job within
//!   Backup.
//!
//! ## Errors
//!
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
//! * `MissingParameterValueException` (HTTP 400) — Indicates that a required parameter is
//!   missing.
//! * `ResourceNotFoundException` (HTTP 400) — A resource that is required for the action
//!   doesn't exist.
//! * `ServiceUnavailableException` (HTTP 500) — The request failed due to a temporary failure
//!   of the server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetRestoreJobMetadataError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetRestoreJobMetadataError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#GetRestoreJobMetadata")]
#[derive(Debug)]
pub enum GetRestoreJobMetadataErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    MissingParameterValueException(MissingParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_restore_job_metadata::GetRestoreJobMetadataError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetRestoreJobMetadataError = crate::scaffolded::Scaffolded<GetRestoreJobMetadataErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetRestoreJobMetadataErrorModeled> for GetRestoreJobMetadataError {
    fn from(modeled: GetRestoreJobMetadataErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetRestoreJobMetadata` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetRestoreJobMetadata<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetRestoreJobMetadata<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetRestoreJobMetadata<Eng>
{
    type Input = GetRestoreJobMetadataInput;
    type Output = GetRestoreJobMetadataOutput;
    type Error = GetRestoreJobMetadataError;

    const NAME: &'static str = "GetRestoreJobMetadata";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_RESTORE_JOB_METADATA_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_RESTORE_JOB_METADATA_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetRestoreJobMetadataErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup vault store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;
        let store = state.lock();
        // A read of the job's metadata changes nothing about the job: the
        // metadata is what the start was given, whatever status the job is in
        // now, so this read materialises no transition either.
        let job = store
            .stored_restore_job(&input.restore_job_id)
            .map_err(GetRestoreJobMetadataErrorModeled::ResourceNotFoundException)?;

        Ok(GetRestoreJobMetadataOutput {
            restore_job_id: Some(job.restore_job_id.clone()),
            // Every key the restore was configured with, each reported with the
            // mask rather than the value it was set to — see
            // `crate::state::METADATA_VALUE_MASK`.
            metadata: Some(
                job.metadata
                    .keys()
                    .map(|key| {
                        (
                            key.clone(),
                            crate::state::METADATA_VALUE_MASK.to_string(),
                        )
                    })
                    .collect(),
            ),
        })
    }
}

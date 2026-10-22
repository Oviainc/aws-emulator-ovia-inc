// SPDX-License-Identifier: CC0-1.0

//! @op StartDICOMImportJob
//!
//! Submits a DICOM import job against a persisted data store and answers with the
//! job's identity and `SUBMITTED` status. `clientToken` is a real
//! `@idempotencyToken`: a repeat call with the same token against the same data store
//! returns the SAME `jobId` instead of submitting a second job.
//!
//! The job is returned `SUBMITTED`, the status AWS HealthImaging answers with, and the
//! clock owes its moves to `IN_PROGRESS` and then `COMPLETED` — see
//! [`crate::state::IMPORT_JOB_IN_PROGRESS_DELAY`] and
//! [`crate::state::IMPORT_JOB_COMPLETION_DELAY`]. Nothing here sleeps.
//!
//! ---
//!
//! # Modeled contract for `StartDICOMImportJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! Start importing bulk data into an ACTIVE data store. The import job imports DICOM P10
//! files or enhances existing DICOM files with JSON metadata. The importConfiguration
//! parameter specifies the import type. The data is found in the S3 prefix specified by the
//! inputS3Uri parameter. The import job stores processing results in the file specified by
//! the outputS3Uri parameter.
//!
//! ## Input
//!
//! * `clientToken` (ClientToken, len 1..64, pattern ^[A-Za-z0-9._-]+$, @idempotencyToken)
//!   **required** — A unique identifier for API idempotency.
//! * `dataAccessRoleArn` (RoleArn, len 20..2048, pattern
//!   ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$) **required** — The Amazon Resource Name (ARN)
//!   of the IAM role that grants permission to access medical imaging resources.
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `importConfiguration` (ImportConfiguration) — The import configuration for the import
//!   job.
//! * `inputOwnerAccountId` (AwsAccountId, len 12..12, pattern ^\d+$) — The account ID of the
//!   source S3 bucket owner.
//! * `inputS3Uri` (S3Uri, len 1..1024, pattern
//!   ^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$) **required** — The input prefix path
//!   for the S3 bucket that contains the DICOM files to be imported.
//! * `jobName` (JobName, len 1..64, pattern ^[A-Za-z0-9._/#-]+$) — The import job name.
//! * `outputS3Uri` (S3Uri, len 1..1024, pattern
//!   ^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$) **required** — The output prefix of the
//!   S3 bucket to upload the results of the DICOM import job.
//!
//! ## Output
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `jobId` (JobId, len 1..32, pattern ^[0-9a-z]+$) **required** — The import job
//!   identifier.
//! * `jobStatus` (JobStatus, one of: COMPLETED | FAILED | IN_PROGRESS | SUBMITTED)
//!   **required** — The import job status.
//! * `submittedAt` (Date) **required** — The timestamp when the import job was submitted.
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

use crate::state::{ImportJob, JobStatus};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartDICOMImportJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartDICOMImportJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#StartDICOMImportJob")]
#[derive(Debug)]
pub enum StartDICOMImportJobErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_dicom_import_job::StartDICOMImportJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartDICOMImportJobError = crate::scaffolded::Scaffolded<StartDICOMImportJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartDICOMImportJobErrorModeled> for StartDICOMImportJobError {
    fn from(modeled: StartDICOMImportJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartDICOMImportJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartDICOMImportJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartDICOMImportJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartDICOMImportJob<Eng>
{
    type Input = StartDICOMImportJobInput;
    type Output = StartDICOMImportJobOutput;
    type Error = StartDICOMImportJobError;

    const NAME: &'static str = "StartDICOMImportJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_DICOM_IMPORT_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_DICOM_IMPORT_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StartDICOMImportJobErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        if !store.datastores.contains_key(&input.datastore_id) {
            return Err(StartDICOMImportJobErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    // Constant, and stated by resource KIND: the requested id is not
                    // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                    message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
                },
            )
            .into());
        }

        // `@idempotencyToken`, scoped to the data store the job runs against: a token
        // already in the ledger names the job it submitted, so the repeat call reports
        // that job's CURRENT status rather than submitting a second one.
        let token_key = crate::state::client_token_key(
            &format!("StartDICOMImportJob/{}", input.datastore_id),
            &input.client_token,
        );
        if let Some(existing_id) = store.client_tokens.get(&token_key).cloned() {
            if let Some(existing) = store
                .datastores
                .get(&input.datastore_id)
                .and_then(|datastore| datastore.import_jobs.get(&existing_id))
            {
                return Ok(StartDICOMImportJobOutput {
                    datastore_id: existing.datastore_id.clone(),
                    job_id: existing.job_id.clone(),
                    job_status: existing.job_status.as_wire().to_string(),
                    submitted_at: crate::state::to_date_time(existing.submitted_at),
                });
            }
        }

        // Bound the memory one caller can make this emulator hold, per data store, so
        // the product of data stores and jobs stays bounded too. The cap is the
        // emulator's own ([`crate::state::MAX_IMPORT_JOBS_PER_DATASTORE`]), refused
        // with the operation's modelled quota error.
        if store
            .datastores
            .get(&input.datastore_id)
            .is_some_and(|datastore| {
                datastore.import_jobs.len() >= crate::state::MAX_IMPORT_JOBS_PER_DATASTORE
            })
        {
            return Err(StartDICOMImportJobErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "this emulator holds at most {} import jobs per data store",
                        crate::state::MAX_IMPORT_JOBS_PER_DATASTORE
                    ),
                },
            )
            .into());
        }

        let job_id = store.mint_id();
        let job = ImportJob {
            job_id: job_id.clone(),
            job_name: input.job_name.clone(),
            job_status: JobStatus::Submitted,
            datastore_id: input.datastore_id.clone(),
            data_access_role_arn: input.data_access_role_arn.clone(),
            input_s3_uri: input.input_s3_uri.clone(),
            output_s3_uri: input.output_s3_uri.clone(),
            input_owner_account_id: input.input_owner_account_id.clone(),
            import_configuration: input.import_configuration.clone(),
            submitted_at: now,
            ended_at: None,
            message: None,
            in_progress_due_at: now + crate::state::IMPORT_JOB_IN_PROGRESS_DELAY,
            completed_due_at: now + crate::state::IMPORT_JOB_COMPLETION_DELAY,
        };
        let job_status = job.job_status.as_wire().to_string();
        let submitted_at = crate::state::to_date_time(job.submitted_at);

        // Persist FIRST, and refuse rather than branch past the insert: answering 200
        // with a jobId that was never stored would dress absence as success.
        store
            .datastores
            .get_mut(&input.datastore_id)
            .ok_or_else(|| {
                StartDICOMImportJobErrorModeled::InternalServerException(InternalServerException {
                    message: "the data store could not be reached while submitting the import job"
                        .to_string(),
                })
            })?
            .import_jobs
            .insert(job_id.clone(), job);
        store.remember_client_token(token_key, job_id.clone());

        Ok(StartDICOMImportJobOutput {
            datastore_id: input.datastore_id,
            job_id,
            job_status,
            submitted_at,
        })
    }
}

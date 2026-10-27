// SPDX-License-Identifier: CC0-1.0

//! @op GetModelCopyJob
//!
//! Reports the model copy job a `jobArn` names, entirely out of the record
//! `ops/create_model_copy_job.rs` stored in [`crate::state`]: the source model and the
//! account it came from, the copied model's ARN, name, key and tags, the job's status and
//! the instant it was created. A `jobArn` naming no job of this account is 404
//! `ResourceNotFoundException`.
//!
//! A REPORTING operation (`@readonly`): the only write on this path is the one the CLOCK
//! already made due — the completion the job was owed
//! ([`crate::state::MODEL_COPY_JOB_COMPLETE_AFTER`] after it was submitted), which also
//! registers the copied model among the account's custom models. It was owed at that
//! instant whether or not anybody read the job, so a read of the copied model
//! materialises it just the same.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetModelCopyJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Retrieves information about a model copy job. For more information, see Copy models to
//! be used in other regions in the Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `jobArn` (ModelCopyJobArn, len ..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-copy-job/[a-z0-9]{12}$)
//!   **required** — The Amazon Resource Name (ARN) of the model copy job.
//!
//! ## Output
//!
//! * `creationTime` (Timestamp) **required** — The time at which the model copy job was
//!   created.
//! * `failureMessage` (ErrorMessage, len ..2048) — An error message for why the model copy
//!   job failed.
//! * `jobArn` (ModelCopyJobArn, len ..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-copy-job/[a-z0-9]{12}$)
//!   **required** — The Amazon Resource Name (ARN) of the model copy job.
//! * `sourceAccountId` (AccountId, pattern ^[0-9]{12}$) **required** — The unique identifier
//!   of the account that the model being copied originated from.
//! * `sourceModelArn` (ModelArn, len 20..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/((imported)|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}))((
//!   …) **required** — The Amazon Resource Name (ARN) of the original model being copied.
//! * `sourceModelName` (CustomModelName, len 1..63, pattern ^([0-9a-zA-Z][_-]?){1,63}$) — The
//!   name of the original model being copied.
//! * `status` (ModelCopyJobStatus, one of: Completed | Failed | InProgress) **required** —
//!   The status of the model copy job.
//! * `targetModelArn` (CustomModelArn, len 20..1011, pattern
//!   ^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model/(imported|[a-z0-9-]{1,63}[.]{1}[a-z0-9
//!   …) **required** — The Amazon Resource Name (ARN) of the copied model.
//! * `targetModelKmsKeyArn` (KmsKeyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$) — The Amazon
//!   Resource Name (ARN) of the KMS key encrypting the copied model.
//! * `targetModelName` (CustomModelName, len 1..63, pattern ^([0-9a-zA-Z][_-]?){1,63}$) — The
//!   name of the copied model.
//! * `targetModelTags` (TagList, len 0..200) — The tags associated with the copied model.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetModelCopyJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetModelCopyJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetModelCopyJob")]
#[derive(Debug)]
pub enum GetModelCopyJobErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_model_copy_job::GetModelCopyJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetModelCopyJobError = crate::scaffolded::Scaffolded<GetModelCopyJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetModelCopyJobErrorModeled> for GetModelCopyJobError {
    fn from(modeled: GetModelCopyJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetModelCopyJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetModelCopyJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetModelCopyJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetModelCopyJob<Eng>
{
    type Input = GetModelCopyJobInput;
    type Output = GetModelCopyJobOutput;
    type Error = GetModelCopyJobError;

    const NAME: &'static str = "GetModelCopyJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_MODEL_COPY_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_MODEL_COPY_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetModelCopyJobErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "model copy job state is not available on this engine".to_string(),
                    ),
                },
            )
            .into());
        };
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // The job as it stands at `now`: the completion the clock has already made due is
        // materialised by the lookup, so the status reported here is the status the job
        // holds whichever operation asked.
        let Some(job) = store
            .model_copy_job(&input.job_arn, now)
            .map(crate::state::model_copy_job_summary)
        else {
            return Err(GetModelCopyJobErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(format!(
                        "no model copy job {} exists in this account",
                        input.job_arn
                    )),
                },
            )
            .into());
        };

        Ok(GetModelCopyJobOutput {
            job_arn: job.job_arn,
            status: job.status,
            creation_time: job.creation_time,
            target_model_arn: job.target_model_arn,
            target_model_name: job.target_model_name,
            source_account_id: job.source_account_id,
            source_model_arn: job.source_model_arn,
            target_model_kms_key_arn: job.target_model_kms_key_arn,
            target_model_tags: job.target_model_tags,
            failure_message: job.failure_message,
            source_model_name: job.source_model_name,
        })
    }
}

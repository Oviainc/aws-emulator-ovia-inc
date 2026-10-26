// SPDX-License-Identifier: CC0-1.0

//! @op GetAdvancedPromptOptimizationJob
//! Reports one stored advanced prompt optimization job: the identity, configuration and
//! status `ops/create_advanced_prompt_optimization_job.rs` stored and
//! `ops/stop_advanced_prompt_optimization_job.rs` may since have moved. This is the ONLY
//! file to touch for this operation.
//!
//! A REPORTING operation (`@readonly`): it answers out of the stored record and the only
//! write it makes is materialising the status transition the clock had already made due
//! ([`crate::state::AdvancedPromptOptimizationJobRecord::settle`]), which was owed at that
//! instant even if nobody had called. An identifier no stored job answers to — the job ARN
//! or the bare minted id — is refused with 404 `ResourceNotFoundException`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetAdvancedPromptOptimizationJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Gets information about an advanced prompt optimization job.
//!
//! ## Input
//!
//! * `jobIdentifier` (AdvancedPromptOptimizationJobIdentifier, len ..1011, pattern
//!   ^((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})$)
//!   **required** — The ARN or ID of the advanced prompt optimization job.
//!
//! ## Output
//!
//! * `creationTime` (Timestamp) **required** — The time at which the advanced prompt
//!   optimization job was created.
//! * `encryptionKeyArn` (KmsKeyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$) — The Amazon
//!   Resource Name (ARN) of the KMS key used to encrypt the output data.
//! * `failureMessage` (ErrorMessage, len ..2048) — If the job failed, a message describing
//!   the reason for the failure.
//! * `inputConfig` (AdvancedPromptOptimizationInputConfig) **required** — The input data
//!   configuration for the optimization job.
//! * `jobArn` (AdvancedPromptOptimizationJobArn, len ..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/[a-z0-9]{12}$)
//!   **required** — The Amazon Resource Name (ARN) of the advanced prompt optimization job.
//! * `jobDescription` (AdvancedPromptOptimizationJobDescription, len 1..500) — The
//!   description of the advanced prompt optimization job.
//! * `jobName` (AdvancedPromptOptimizationJobName, len 1..100, pattern
//!   ^[a-zA-Z0-9][a-zA-Z0-9.+-]*$) **required** — The name of the advanced prompt
//!   optimization job.
//! * `jobStatus` (AdvancedPromptOptimizationJobStatus, one of: Completed | Deleting | Failed
//!   | InProgress | PartiallyCompleted | Stopped | Stopping) **required** — The status of the
//!   advanced prompt optimization job.
//! * `lastModifiedTime` (Timestamp) — The time at which the advanced prompt optimization job
//!   was last modified.
//! * `modelConfigurations` (ModelConfigurations, len 1..5) **required** — The model
//!   configurations used in the optimization job.
//! * `outputConfig` (AdvancedPromptOptimizationOutputConfig) **required** — The output data
//!   configuration for the optimization job.
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
// `GetAdvancedPromptOptimizationJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetAdvancedPromptOptimizationJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#GetAdvancedPromptOptimizationJob")]
#[derive(Debug)]
pub enum GetAdvancedPromptOptimizationJobErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_advanced_prompt_optimization_job::GetAdvancedPromptOptimizationJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetAdvancedPromptOptimizationJobError = crate::scaffolded::Scaffolded<GetAdvancedPromptOptimizationJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetAdvancedPromptOptimizationJobErrorModeled> for GetAdvancedPromptOptimizationJobError {
    fn from(modeled: GetAdvancedPromptOptimizationJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAdvancedPromptOptimizationJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetAdvancedPromptOptimizationJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAdvancedPromptOptimizationJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAdvancedPromptOptimizationJob<Eng>
{
    type Input = GetAdvancedPromptOptimizationJobInput;
    type Output = GetAdvancedPromptOptimizationJobOutput;
    type Error = GetAdvancedPromptOptimizationJobError;

    const NAME: &'static str = "GetAdvancedPromptOptimizationJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ADVANCED_PROMPT_OPTIMIZATION_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ADVANCED_PROMPT_OPTIMIZATION_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                GetAdvancedPromptOptimizationJobErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "advanced prompt optimization job state is not available on this \
                             engine"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };
        // An `AdvancedPromptOptimizationJobIdentifier` is the job's ARN or the bare minted
        // id, so the region the request addressed is what completes the bare spelling.
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        let Some(job) = store.advanced_prompt_optimization_job(&input.job_identifier, &region, now)
        else {
            return Err(
                GetAdvancedPromptOptimizationJobErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!(
                            "no advanced prompt optimization job answers to {} in this account \
                             in {region}",
                            input.job_identifier
                        )),
                    },
                )
                .into(),
            );
        };

        Ok(GetAdvancedPromptOptimizationJobOutput {
            job_arn: job.arn.clone(),
            job_name: job.name.clone(),
            job_description: job.description.clone(),
            job_status: job.status.to_string(),
            input_config: job.input_config.clone(),
            output_config: job.output_config.clone(),
            encryption_key_arn: job.encryption_key_arn.clone(),
            creation_time: crate::state::date_time(job.created_at),
            last_modified_time: Some(crate::state::date_time(job.last_modified_at)),
            // "The failure message if the job failed": a job this emulator mints runs or is
            // stopped, and neither carries one.
            failure_message: None,
            model_configurations: job.model_configurations.clone(),
        })
    }
}

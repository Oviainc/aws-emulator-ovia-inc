// SPDX-License-Identifier: CC0-1.0

//! @op CreateAdvancedPromptOptimizationJob
//! Creates one advanced prompt optimization job and persists it, so
//! `GetAdvancedPromptOptimizationJob` reads back what this call stored and
//! `StopAdvancedPromptOptimizationJob` winds down the same record. This is the ONLY file
//! to touch for this operation.
//!
//! What this handler decides, beyond storing the request:
//!
//! * the job is minted `InProgress`
//!   ([`crate::state::ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_IN_PROGRESS`]), because the
//!   optimization runs after the call returns;
//! * a replayed `clientToken` (`@idempotencyToken`) answers with the job the FIRST request
//!   minted and mints nothing;
//! * more tags than one resource may carry is refused with the modelled
//!   `TooManyTagsException`, and the account's job bound with
//!   `ServiceQuotaExceededException`.
//!
//! `jobName` is NOT an identifier of this resource — an
//! `AdvancedPromptOptimizationJobIdentifier` is the job ARN or the minted id alone
//! (`smithy_query(query="operation",operation="GetAdvancedPromptOptimizationJob")`) — and
//! the model documents no uniqueness for it, so a second create reusing a name is a second
//! job rather than a conflict.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateAdvancedPromptOptimizationJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Creates an advanced prompt optimization job. The job optimizes your prompt templates for
//! specific models using your evaluation dataset and criteria.
//!
//! ## Input
//!
//! * `clientToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the API request completes no more than one
//!   time. If this token matches a previous request, Amazon Bedrock ignores the request but
//!   does not return an error.
//! * `encryptionKeyArn` (KmsKeyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$) — The Amazon
//!   Resource Name (ARN) of the KMS key used for encrypting the output data. If not
//!   specified, the output is encrypted with an Amazon-owned KMS key.
//! * `inputConfig` (AdvancedPromptOptimizationInputConfig) **required** — Specifies the S3
//!   location of your JSONL input file containing prompt templates and evaluation samples.
//! * `jobDescription` (AdvancedPromptOptimizationJobDescription, len 1..500) — A description
//!   of the advanced prompt optimization job.
//! * `jobName` (AdvancedPromptOptimizationJobName, len 1..100, pattern
//!   ^[a-zA-Z0-9][a-zA-Z0-9.+-]*$) **required** — A name for the advanced prompt optimization
//!   job.
//! * `modelConfigurations` (ModelConfigurations, len 1..5) **required** — A list of model
//!   configurations specifying the target models for prompt optimization. You can specify up
//!   to 5 models.
//! * `outputConfig` (AdvancedPromptOptimizationOutputConfig) **required** — Specifies the S3
//!   location where optimization results will be stored.
//! * `tags` (TagList, len 0..200) — Tags to associate with the advanced prompt optimization
//!   job.
//!
//! ## Output
//!
//! * `jobArn` (AdvancedPromptOptimizationJobArn, len ..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/[a-z0-9]{12}$)
//!   **required** — The Amazon Resource Name (ARN) of the created advanced prompt
//!   optimization job.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `ConflictException` (HTTP 400) — Error occurred because of a conflict while performing
//!   an operation.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ServiceQuotaExceededException` (HTTP 400) — The number of requests exceeds the service
//!   quota. Resubmit your request later.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `TooManyTagsException` (HTTP 400) — The request contains more tags than can be
//!   associated with a resource (50 tags per resource). The maximum number of tags includes
//!   both existing tags and those included in your current request.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateAdvancedPromptOptimizationJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateAdvancedPromptOptimizationJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#CreateAdvancedPromptOptimizationJob")]
#[derive(Debug)]
pub enum CreateAdvancedPromptOptimizationJobErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    TooManyTagsException(TooManyTagsException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_advanced_prompt_optimization_job::CreateAdvancedPromptOptimizationJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateAdvancedPromptOptimizationJobError = crate::scaffolded::Scaffolded<CreateAdvancedPromptOptimizationJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateAdvancedPromptOptimizationJobErrorModeled> for CreateAdvancedPromptOptimizationJobError {
    fn from(modeled: CreateAdvancedPromptOptimizationJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateAdvancedPromptOptimizationJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateAdvancedPromptOptimizationJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateAdvancedPromptOptimizationJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateAdvancedPromptOptimizationJob<Eng>
{
    type Input = CreateAdvancedPromptOptimizationJobInput;
    type Output = CreateAdvancedPromptOptimizationJobOutput;
    type Error = CreateAdvancedPromptOptimizationJobError;

    const NAME: &'static str = "CreateAdvancedPromptOptimizationJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ADVANCED_PROMPT_OPTIMIZATION_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ADVANCED_PROMPT_OPTIMIZATION_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                CreateAdvancedPromptOptimizationJobErrorModeled::InternalServerException(
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
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();

        // `TooManyTagsException` documents the bound itself ("50 tags per resource"), and
        // the modelled `@length(0..200)` on `TagList` is enforced ahead of this handler, so
        // this is the tighter per-resource bound inside that span.
        let tags = input.tags.unwrap_or_default();
        if tags.len() > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(
                CreateAdvancedPromptOptimizationJobErrorModeled::TooManyTagsException(
                    TooManyTagsException {
                        message: Some(format!(
                            "a resource may carry at most {} tags, and this request carries {}",
                            crate::state::MAX_TAGS_PER_RESOURCE,
                            tags.len()
                        )),
                        resource_name: None,
                    },
                )
                .into(),
            );
        }

        let mut store = state.lock();

        // `@idempotencyToken`: a replay answers with what the first request minted.
        let replayed = input
            .client_token
            .as_ref()
            .and_then(|token| {
                store
                    .advanced_prompt_optimization_job_create_tokens
                    .get(&(region.clone(), token.clone()))
            })
            .cloned();
        if let Some(minted) = replayed {
            if store
                .advanced_prompt_optimization_jobs
                .contains_key(&minted)
            {
                return Ok(CreateAdvancedPromptOptimizationJobOutput { job_arn: minted });
            }
        }

        // The account's job bound. Enforcing it here is also what bounds this part of the
        // store: without a server-side cap a caller could grow it with nothing but fresh
        // creates.
        if store.advanced_prompt_optimization_jobs.len()
            >= crate::state::MAX_ADVANCED_PROMPT_OPTIMIZATION_JOBS_PER_ACCOUNT
        {
            return Err(
                CreateAdvancedPromptOptimizationJobErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "this account already holds the maximum of {} advanced prompt \
                             optimization jobs; request a quota increase",
                            crate::state::MAX_ADVANCED_PROMPT_OPTIMIZATION_JOBS_PER_ACCOUNT
                        )),
                    },
                )
                .into(),
            );
        }

        let job_id = store.mint_advanced_prompt_optimization_job_id(now);
        let arn = crate::state::advanced_prompt_optimization_job_arn(&region, &job_id);
        let job = crate::state::AdvancedPromptOptimizationJobRecord {
            arn: arn.clone(),
            name: input.job_name,
            region: region.clone(),
            description: input.job_description,
            input_config: input.input_config,
            output_config: input.output_config,
            encryption_key_arn: input.encryption_key_arn,
            model_configurations: input.model_configurations,
            tags,
            // The optimization runs after this call returns, so the job is reported in
            // progress from here onwards.
            status: crate::state::ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_IN_PROGRESS,
            // Nothing has asked for a stop yet, so the clock owes this job no transition.
            stopped_due_at: None,
            created_at: now,
            last_modified_at: now,
        };

        if let Some(token) = input.client_token {
            store
                .advanced_prompt_optimization_job_create_tokens
                .insert((region, token), arn.clone());
        }
        // The ARN the caller is told is the KEY the record is stored under, so a later
        // `GetAdvancedPromptOptimizationJob` addressed with it finds exactly this job.
        store
            .advanced_prompt_optimization_jobs
            .insert(arn.clone(), job);

        Ok(CreateAdvancedPromptOptimizationJobOutput { job_arn: arn })
    }
}

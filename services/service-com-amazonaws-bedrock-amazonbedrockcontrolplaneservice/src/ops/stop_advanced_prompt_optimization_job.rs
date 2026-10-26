// SPDX-License-Identifier: CC0-1.0

//! @op StopAdvancedPromptOptimizationJob
//! Stops one stored advanced prompt optimization job: it moves the job
//! `ops/create_advanced_prompt_optimization_job.rs` minted to `Stopping` and records when
//! the clock owes it `Stopped`, which
//! `ops/get_advanced_prompt_optimization_job.rs` then reports. This is the ONLY file to
//! touch for this operation.
//!
//! "Stops an advanced prompt optimization job that is in progress"
//! (`smithy_query(query="doc",shape="StopAdvancedPromptOptimizationJob")`), so a job in
//! any other status has nothing left to stop and the call is refused rather than silently
//! accepted; an identifier no stored job answers to is refused with 404
//! `ResourceNotFoundException`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `StopAdvancedPromptOptimizationJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Stops an advanced prompt optimization job that is in progress.
//!
//! ## Input
//!
//! * `jobIdentifier` (AdvancedPromptOptimizationJobIdentifier, len ..1011, pattern
//!   ^((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})$)
//!   **required** — The ARN or ID of the advanced prompt optimization job to stop.
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
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StopAdvancedPromptOptimizationJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StopAdvancedPromptOptimizationJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#StopAdvancedPromptOptimizationJob")]
#[derive(Debug)]
pub enum StopAdvancedPromptOptimizationJobErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::stop_advanced_prompt_optimization_job::StopAdvancedPromptOptimizationJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StopAdvancedPromptOptimizationJobError = crate::scaffolded::Scaffolded<StopAdvancedPromptOptimizationJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StopAdvancedPromptOptimizationJobErrorModeled> for StopAdvancedPromptOptimizationJobError {
    fn from(modeled: StopAdvancedPromptOptimizationJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StopAdvancedPromptOptimizationJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StopAdvancedPromptOptimizationJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StopAdvancedPromptOptimizationJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StopAdvancedPromptOptimizationJob<Eng>
{
    type Input = StopAdvancedPromptOptimizationJobInput;
    type Output = StopAdvancedPromptOptimizationJobOutput;
    type Error = StopAdvancedPromptOptimizationJobError;

    const NAME: &'static str = "StopAdvancedPromptOptimizationJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::STOP_ADVANCED_PROMPT_OPTIMIZATION_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::STOP_ADVANCED_PROMPT_OPTIMIZATION_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                StopAdvancedPromptOptimizationJobErrorModeled::InternalServerException(
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
        // id, so the region the request addressed is what completes the bare spelling —
        // exactly as it does for a read.
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        let Some(job) = store.advanced_prompt_optimization_job(&input.job_identifier, &region, now)
        else {
            return Err(
                StopAdvancedPromptOptimizationJobErrorModeled::ResourceNotFoundException(
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

        // "Stops an advanced prompt optimization job that is in progress": a job in any
        // other status has nothing left to stop, so asking again is refused.
        //
        // The refusal is the modelled `ValidationException` rather than the
        // `ConflictException` this operation also models, and its message reports that the
        // job is already stopped: that is what this service answers for a stop of a job of
        // its own that is already stopped (`ops/stop_evaluation_job.rs`, which the corpus
        // records). The two non-running statuses reachable here are the two a stop itself
        // produces (`Stopping` and `Stopped`), so that is what every refusal this branch
        // words is about.
        if !job.is_in_progress() {
            return Err(
                StopAdvancedPromptOptimizationJobErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(format!(
                            "Advanced prompt optimization job: {} is already in a stopped state.",
                            input.job_identifier
                        )),
                    },
                )
                .into(),
            );
        }

        // Stopping is asynchronous, so the job is reported `Stopping` from here onwards and
        // the clock owes it the terminal status.
        job.stop(now);
        Ok(StopAdvancedPromptOptimizationJobOutput {})
    }
}

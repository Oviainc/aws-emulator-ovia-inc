// SPDX-License-Identifier: CC0-1.0

//! @op StopTextTranslationJob
//!
//! Marks one submitted batch translation job for termination: a job that has
//! not reached a terminal status yet is put into `STOP_REQUESTED` and reaches
//! `STOPPED` a phase later, and a job that got there first keeps the terminal
//! status it reached — "if the job completes before it can be stopped, it is
//! put into the COMPLETED state".
//!
//! The stop is recorded on the job record
//! [`crate::ops::start_text_translation_job`] persisted, and the status this
//! operation answers with is the one every other reader of that record derives
//! ([`crate::state::batch_translation`]).
//!
//! ---
//!
//! # Modeled contract for `StopTextTranslationJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json")
//! ```
//!
//! Stops an asynchronous batch translation job that is in progress. If the job's state is
//! IN_PROGRESS, the job will be marked for termination and put into the STOP_REQUESTED
//! state. If the job completes before it can be stopped, it is put into the COMPLETED
//! state. Otherwise, the job is put into the STOPPED state. Asynchronous batch translation
//! jobs are started with the StartTextTranslationJob operation. You can use the
//! DescribeTextTranslationJob or ListTextTranslationJobs operations to get a batch
//! translation job's JobId.
//!
//! ## Input
//!
//! * `JobId` (JobId, len 1..32, pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$) **required** — The
//!   job ID of the job to be stopped.
//!
//! ## Output
//!
//! * `JobId` (JobId, len 1..32, pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$) — The job ID of the
//!   stopped batch translation job.
//! * `JobStatus` (JobStatus, one of: COMPLETED | COMPLETED_WITH_ERROR | FAILED | IN_PROGRESS
//!   | STOPPED | STOP_REQUESTED | SUBMITTED) — The status of the designated job. Upon
//!   successful completion, the job's status will be STOPPED.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The resource you are looking for has not been
//!   found. Review the resource you're looking for and see if a different resource will
//!   accomplish your needs before retrying the revised request.
//! * `TooManyRequestsException` (HTTP 429) — You have made too many requests within a short
//!   period of time. Wait for a short time and then try your request again.

#[allow(unused_imports)]
use super::types::*;

use std::time::SystemTime;

use crate::state::{resource_not_found, service_state};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StopTextTranslationJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StopTextTranslationJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.translate#StopTextTranslationJob")]
#[derive(Debug)]
pub enum StopTextTranslationJobErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::stop_text_translation_job::StopTextTranslationJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StopTextTranslationJobError = crate::scaffolded::Scaffolded<StopTextTranslationJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StopTextTranslationJobErrorModeled> for StopTextTranslationJobError {
    fn from(modeled: StopTextTranslationJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StopTextTranslationJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StopTextTranslationJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StopTextTranslationJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StopTextTranslationJob<Eng>
{
    type Input = StopTextTranslationJobInput;
    type Output = StopTextTranslationJobOutput;
    type Error = StopTextTranslationJobError;

    const NAME: &'static str = "StopTextTranslationJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::STOP_TEXT_TRANSLATION_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::STOP_TEXT_TRANSLATION_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = service_state(&*self.engine).ok_or_else(|| {
            StopTextTranslationJobErrorModeled::InternalServerException(InternalServerException {
                message: Some("the batch translation job store is unavailable".to_string()),
            })
        })?;
        let now = SystemTime::now();

        let mut store = state.lock();
        let Some(job) = store.jobs.get_mut(&input.job_id) else {
            return Err(StopTextTranslationJobErrorModeled::ResourceNotFoundException(
                resource_not_found(),
            )
            .into());
        };
        // A job that has already reached a status it never leaves keeps it: the
        // stop has nothing left to mark for termination. A job that has not is
        // marked once — a repeated stop does not move the instant the first one
        // recorded, so STOPPED stays due when the first request made it due.
        if !job.is_terminal(now) && job.stop_requested_at.is_none() {
            job.stop_requested_at = Some(now);
        }

        Ok(StopTextTranslationJobOutput {
            job_id: Some(job.job_id.clone()),
            job_status: Some(job.status(now).to_string()),
        })
    }
}
